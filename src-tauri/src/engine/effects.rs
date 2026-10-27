//! Insert effects. Each effect splits into parameters (computed off the audio
//! thread at compile time) and state (buffers). When a new compiled song arrives,
//! the audio thread swaps the old state into the new effect so tails continue.

use super::dsp::*;
use crate::model::{Effect, EffectKind, FilterMode, PPQ};
use std::f64::consts::TAU;

/// Per-block context passed to effects.
pub struct FxCtx {
    pub tick: f64,
    pub ticks_per_frame: f64,
    pub playing: bool,
}

pub struct EffectUnit {
    pub id: u64,
    pub enabled: bool,
    /// Wet/dry blend set by automation each block (1 = fully processed).
    pub wet: f32,
    pub fx: Fx,
}

pub enum Fx {
    Eq(EqFx),
    Filter(FilterFx),
    Delay(DelayFx),
    Reverb(Box<ReverbFx>),
    Comp(CompFx),
    Chorus(ChorusFx),
    Crush(CrushFx),
    Pump(PumpFx),
}

impl EffectUnit {
    pub fn new(e: &Effect, sr: f64, bpm: f64) -> Self {
        let fx = match &e.kind {
            EffectKind::Eq { low, mid, mid_freq, high } => Fx::Eq(EqFx {
                low: Biquad::low_shelf(sr, 200.0, *low as f64),
                mid: Biquad::peaking(sr, *mid_freq as f64, 0.9, *mid as f64),
                high: Biquad::high_shelf(sr, 5000.0, *high as f64),
                state: [[BiquadState::default(); 3]; 2],
            }),
            EffectKind::Filter { mode, cutoff, resonance } => Fx::Filter(FilterFx {
                mode: *mode,
                cutoff: *cutoff,
                resonance: *resonance,
                sr,
                c: SvfCoefs::new(*cutoff as f64, *resonance, sr),
                state: [SvfState::default(); 2],
            }),
            EffectKind::Delay { time, feedback, mix, ping_pong } => {
                let len = (sr * 4.0) as usize;
                let d = ((*time as f64) * 60.0 / bpm * sr).round() as usize;
                Fx::Delay(DelayFx {
                    delay: d.clamp(1, len - 1),
                    feedback: feedback.clamp(0.0, 0.95),
                    mix: mix.clamp(0.0, 1.0),
                    ping_pong: *ping_pong,
                    damp: (1.0 - (-TAU * 6000.0 / sr).exp()) as f32,
                    state: DelayState { l: vec![0.0; len], r: vec![0.0; len], w: 0, lp: [0.0; 2] },
                })
            }
            EffectKind::Reverb { size, damping, mix, width } => Fx::Reverb(Box::new(ReverbFx::new(
                sr,
                size.clamp(0.0, 1.0),
                damping.clamp(0.0, 1.0),
                mix.clamp(0.0, 1.0),
                width.clamp(0.0, 1.0),
            ))),
            EffectKind::Compressor { threshold, ratio, attack, release, makeup } => Fx::Comp(CompFx {
                thr: *threshold,
                ratio: ratio.max(1.0),
                att: exp_coef(*attack as f64 / 1000.0, sr),
                rel: exp_coef(*release as f64 / 1000.0, sr),
                makeup: *makeup,
                state: 0.0,
            }),
            EffectKind::Chorus { rate, depth, mix } => {
                let len = (sr * 0.06) as usize;
                Fx::Chorus(ChorusFx {
                    rate: *rate as f64 / sr,
                    depth: (*depth as f64 / 1000.0 * sr).min(sr * 0.02),
                    base: sr * 0.012,
                    mix: mix.clamp(0.0, 1.0),
                    state: ChorusState { l: vec![0.0; len], r: vec![0.0; len], w: 0, phase: 0.0 },
                })
            }
            EffectKind::Bitcrusher { bits, downsample, mix } => Fx::Crush(CrushFx {
                levels: (2f32.powf(bits.clamp(1.0, 16.0) - 1.0)),
                hold: downsample.clamp(1.0, 64.0),
                mix: mix.clamp(0.0, 1.0),
                state: CrushState::default(),
            }),
            EffectKind::Pump { depth, rate, release } => Fx::Pump(PumpFx {
                depth: depth.clamp(0.0, 1.0),
                rate: rate.max(0.0625) as f64,
                release: release.clamp(0.05, 1.0) as f64,
                smooth: (1.0 - (-1.0 / (0.002 * sr)).exp()) as f32,
                g: 1.0,
            }),
        };
        EffectUnit { id: e.id, enabled: e.enabled, wet: 1.0, fx }
    }

    /// Take over the running state (buffers, filter memory) of the previous version of this effect.
    pub fn inherit(&mut self, old: &mut EffectUnit) {
        use std::mem::swap;
        match (&mut self.fx, &mut old.fx) {
            (Fx::Eq(a), Fx::Eq(b)) => swap(&mut a.state, &mut b.state),
            (Fx::Filter(a), Fx::Filter(b)) => swap(&mut a.state, &mut b.state),
            (Fx::Delay(a), Fx::Delay(b)) => {
                if a.state.l.len() == b.state.l.len() {
                    swap(&mut a.state, &mut b.state)
                }
            }
            (Fx::Reverb(a), Fx::Reverb(b)) => swap(&mut a.state, &mut b.state),
            (Fx::Comp(a), Fx::Comp(b)) => swap(&mut a.state, &mut b.state),
            (Fx::Chorus(a), Fx::Chorus(b)) => {
                if a.state.l.len() == b.state.l.len() {
                    swap(&mut a.state, &mut b.state)
                }
            }
            (Fx::Crush(a), Fx::Crush(b)) => swap(&mut a.state, &mut b.state),
            _ => {}
        }
    }

    /// Automated cutoff for the Filter effect (other effects ignore it).
    pub fn set_cutoff(&mut self, hz: f32) {
        if let Fx::Filter(f) = &mut self.fx {
            if (f.cutoff - hz).abs() > 0.01 {
                f.cutoff = hz;
                f.c = SvfCoefs::new(hz as f64, f.resonance, f.sr);
            }
        }
    }

    /// Processes in place, blending with the dry signal when `wet` < 1.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        if self.wet >= 0.999 {
            return self.process_raw(l, r, ctx);
        }
        let n = l.len().min(super::SUB);
        let mut dl = [0f32; super::SUB];
        let mut dr = [0f32; super::SUB];
        dl[..n].copy_from_slice(&l[..n]);
        dr[..n].copy_from_slice(&r[..n]);
        self.process_raw(l, r, ctx);
        let w = self.wet.max(0.0);
        for i in 0..n {
            l[i] = dl[i] + (l[i] - dl[i]) * w;
            r[i] = dr[i] + (r[i] - dr[i]) * w;
        }
    }

    fn process_raw(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        match &mut self.fx {
            Fx::Eq(f) => f.process(l, r),
            Fx::Filter(f) => f.process(l, r),
            Fx::Delay(f) => f.process(l, r),
            Fx::Reverb(f) => f.process(l, r),
            Fx::Comp(f) => f.process(l, r),
            Fx::Chorus(f) => f.process(l, r),
            Fx::Crush(f) => f.process(l, r),
            Fx::Pump(f) => f.process(l, r, ctx),
        }
    }
}

pub struct EqFx {
    low: Biquad,
    mid: Biquad,
    high: Biquad,
    state: [[BiquadState; 3]; 2],
}

impl EqFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (ch, buf) in [l, r].into_iter().enumerate() {
            let s = &mut self.state[ch];
            for x in buf.iter_mut() {
                let mut y = self.low.tick(&mut s[0], *x);
                y = self.mid.tick(&mut s[1], y);
                *x = self.high.tick(&mut s[2], y);
            }
        }
    }
}

pub struct FilterFx {
    mode: FilterMode,
    cutoff: f32,
    resonance: f32,
    sr: f64,
    c: SvfCoefs,
    state: [SvfState; 2],
}

impl FilterFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (ch, buf) in [l, r].into_iter().enumerate() {
            let s = &mut self.state[ch];
            for x in buf.iter_mut() {
                let (lp, bp, hp) = s.tick(&self.c, *x);
                *x = match self.mode {
                    FilterMode::Lowpass => lp,
                    FilterMode::Bandpass => bp,
                    FilterMode::Highpass => hp,
                };
            }
        }
    }
}

pub struct DelayState {
    l: Vec<f32>,
    r: Vec<f32>,
    w: usize,
    lp: [f32; 2],
}

pub struct DelayFx {
    delay: usize,
    feedback: f32,
    mix: f32,
    ping_pong: bool,
    damp: f32,
    state: DelayState,
}

impl DelayFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut self.state;
        let len = s.l.len();
        for i in 0..l.len() {
            let ri = (s.w + len - self.delay) % len;
            let dl = s.l[ri];
            let dr = s.r[ri];
            s.lp[0] += (dl - s.lp[0]) * self.damp;
            s.lp[1] += (dr - s.lp[1]) * self.damp;
            if self.ping_pong {
                s.l[s.w] = 0.5 * (l[i] + r[i]) + s.lp[1] * self.feedback;
                s.r[s.w] = s.lp[0] * self.feedback;
            } else {
                s.l[s.w] = l[i] + s.lp[0] * self.feedback;
                s.r[s.w] = r[i] + s.lp[1] * self.feedback;
            }
            l[i] += dl * self.mix;
            r[i] += dr * self.mix;
            s.w = (s.w + 1) % len;
        }
    }
}

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

pub struct ReverbState {
    combs: [Vec<Comb>; 2],
    aps: [Vec<Allpass>; 2],
}

/// Freeverb (Jezar's public-domain design).
pub struct ReverbFx {
    feedback: f32,
    damp: f32,
    wet1: f32,
    wet2: f32,
    dry: f32,
    state: ReverbState,
}

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const APS: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;

impl ReverbFx {
    fn new(sr: f64, size: f32, damping: f32, mix: f32, width: f32) -> Self {
        let scale = sr / 44100.0;
        let mk = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (
                COMBS
                    .iter()
                    .map(|&n| Comb { buf: vec![0.0; ((n + spread) as f64 * scale) as usize], idx: 0, store: 0.0 })
                    .collect(),
                APS.iter().map(|&n| Allpass { buf: vec![0.0; ((n + spread) as f64 * scale) as usize], idx: 0 }).collect(),
            )
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(SPREAD);
        let wet = mix * 3.0;
        ReverbFx {
            feedback: 0.7 + size * 0.28,
            damp: damping * 0.4,
            wet1: wet * (width / 2.0 + 0.5),
            wet2: wet * ((1.0 - width) / 2.0),
            dry: 1.0 - mix * 0.5,
            state: ReverbState { combs: [cl, cr], aps: [al, ar] },
        }
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let st = &mut self.state;
        for i in 0..l.len() {
            let input = (l[i] + r[i]) * 0.015;
            let mut out = [0.0f32; 2];
            for ch in 0..2 {
                let mut acc = 0.0;
                for c in st.combs[ch].iter_mut() {
                    let y = c.buf[c.idx];
                    c.store = y * (1.0 - self.damp) + c.store * self.damp;
                    c.buf[c.idx] = input + c.store * self.feedback;
                    c.idx += 1;
                    if c.idx >= c.buf.len() {
                        c.idx = 0;
                    }
                    acc += y;
                }
                for a in st.aps[ch].iter_mut() {
                    let b = a.buf[a.idx];
                    let y = b - acc;
                    a.buf[a.idx] = acc + b * 0.5;
                    a.idx += 1;
                    if a.idx >= a.buf.len() {
                        a.idx = 0;
                    }
                    acc = y;
                }
                out[ch] = acc;
            }
            let (dl, dr) = (l[i], r[i]);
            l[i] = dl * self.dry + out[0] * self.wet1 + out[1] * self.wet2;
            r[i] = dr * self.dry + out[1] * self.wet1 + out[0] * self.wet2;
        }
    }
}

pub struct CompFx {
    thr: f32,
    ratio: f32,
    att: f32,
    rel: f32,
    makeup: f32,
    /// Current gain reduction in dB.
    state: f32,
}

impl CompFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let mk = db_to_gain(self.makeup);
        for i in 0..l.len() {
            let peak = l[i].abs().max(r[i].abs()) + 1e-9;
            let level = 20.0 * peak.log10();
            let over = level - self.thr;
            let target = if over > 0.0 { over * (1.0 - 1.0 / self.ratio) } else { 0.0 };
            let c = if target > self.state { self.att } else { self.rel };
            self.state = target + (self.state - target) * c;
            let g = db_to_gain(-self.state) * mk;
            l[i] *= g;
            r[i] *= g;
        }
    }
}

pub struct ChorusState {
    l: Vec<f32>,
    r: Vec<f32>,
    w: usize,
    phase: f64,
}

pub struct ChorusFx {
    rate: f64,
    depth: f64,
    base: f64,
    mix: f32,
    state: ChorusState,
}

impl ChorusFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut self.state;
        let len = s.l.len();
        let read = |buf: &[f32], w: usize, d: f64| -> f32 {
            let pos = w as f64 + len as f64 - d;
            let i = pos.floor() as usize;
            let f = (pos - pos.floor()) as f32;
            let a = buf[i % len];
            let b = buf[(i + 1) % len];
            a + (b - a) * f
        };
        for i in 0..l.len() {
            s.l[s.w] = l[i];
            s.r[s.w] = r[i];
            let dl = self.base + self.depth * (0.5 + 0.5 * (TAU * s.phase).sin());
            let dr = self.base + self.depth * (0.5 + 0.5 * (TAU * (s.phase + 0.25)).sin());
            let wl = read(&s.l, s.w, dl);
            let wr = read(&s.r, s.w, dr);
            l[i] = l[i] * (1.0 - self.mix * 0.5) + wl * self.mix;
            r[i] = r[i] * (1.0 - self.mix * 0.5) + wr * self.mix;
            s.w = (s.w + 1) % len;
            s.phase += self.rate;
            if s.phase >= 1.0 {
                s.phase -= 1.0;
            }
        }
    }
}

#[derive(Default)]
pub struct CrushState {
    count: f32,
    hold: [f32; 2],
}

pub struct CrushFx {
    levels: f32,
    hold: f32,
    mix: f32,
    state: CrushState,
}

impl CrushFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let s = &mut self.state;
        for i in 0..l.len() {
            s.count -= 1.0;
            if s.count <= 0.0 {
                s.count += self.hold;
                s.hold[0] = (l[i] * self.levels).round() / self.levels;
                s.hold[1] = (r[i] * self.levels).round() / self.levels;
            }
            l[i] += (s.hold[0] - l[i]) * self.mix;
            r[i] += (s.hold[1] - r[i]) * self.mix;
        }
    }
}

pub struct PumpFx {
    depth: f32,
    rate: f64,
    release: f64,
    smooth: f32,
    g: f32,
}

impl PumpFx {
    fn process(&mut self, l: &mut [f32], r: &mut [f32], ctx: &FxCtx) {
        if !ctx.playing {
            return;
        }
        for i in 0..l.len() {
            let beats = (ctx.tick + ctx.ticks_per_frame * i as f64) / PPQ as f64;
            let ph = (beats / self.rate).fract();
            let x = (ph / self.release).min(1.0);
            // Fast duck at the beat, smooth recovery.
            let target = 1.0 - self.depth * (1.0 - x * x * (3.0 - 2.0 * x)) as f32;
            // ~2 ms smoothing so the duck doesn't click.
            self.g += (target - self.g) * self.smooth;
            l[i] *= self.g;
            r[i] *= self.g;
        }
    }
}
