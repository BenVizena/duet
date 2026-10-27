//! Voices for the built-in instruments: the synth, the sampler and the drum kits.
//! Everything here runs on the audio thread, so no allocation.

use super::compile::CInstrument;
use super::dsp::*;
use crate::model::{DrumParams, SamplerParams, SynthParams, Wave};
use std::f64::consts::TAU;

pub const MAX_UNISON: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Off,
}

#[derive(Clone, Copy)]
pub struct Env {
    pub stage: Stage,
    pub level: f32,
}

impl Env {
    #[inline]
    fn next(&mut self, att_inc: f32, dec_coef: f32, sus: f32, rel_coef: f32) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level += att_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = sus + (self.level - sus) * dec_coef;
                if (self.level - sus).abs() < 1e-4 {
                    self.level = sus;
                    self.stage = if sus <= 1e-4 { Stage::Off } else { Stage::Sustain };
                }
            }
            Stage::Sustain => self.level = sus,
            Stage::Release => {
                self.level *= rel_coef;
                if self.level < 1e-4 {
                    self.level = 0.0;
                    self.stage = Stage::Off;
                }
            }
            Stage::Off => self.level = 0.0,
        }
        self.level
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VoiceKind {
    Synth,
    Sampler,
    Drum,
}

#[derive(Clone, Copy, Default)]
pub struct Drum {
    /// 0 = sine, 1 = triangle, 2 = square
    tone_wave: u8,
    phase: f64,
    freq: f64,
    freq_end: f64,
    pcoef: f64,
    tamp: f32,
    tcoef: f32,
    namp: f32,
    ncoef: f32,
    nrate: f64,
    short: bool,
    white: bool,
    /// 808-style metallic noise from six square oscillators.
    metal: bool,
    metal_ph: [f64; 6],
    hp_coef: f32,
    hp_state: f32,
    clap: bool,
    crush: bool,
    gain: f32,
}

#[derive(Clone)]
pub struct Voice {
    pub active: bool,
    pub releasing: bool,
    pub live: bool,
    pub kind: VoiceKind,
    pub track_id: u64,
    pub track_index: usize,
    pub pitch: u8,
    pub vel: f32,
    pub end_tick: f64,
    pub serial: u64,
    fast: bool,
    note: f64,
    target: f64,
    t: f64,
    phases: [f64; MAX_UNISON],
    mphase: f64,
    env: Env,
    fenv: f32,
    svf: SvfState,
    lfsr: u32,
    nacc: f64,
    nval: f32,
    rng: u32,
    pos: f64,
    drum: Drum,
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            active: false,
            releasing: false,
            live: false,
            kind: VoiceKind::Synth,
            track_id: 0,
            track_index: 0,
            pitch: 60,
            vel: 1.0,
            end_tick: f64::INFINITY,
            serial: 0,
            fast: false,
            note: 60.0,
            target: 60.0,
            t: 0.0,
            phases: [0.0; MAX_UNISON],
            mphase: 0.0,
            env: Env { stage: Stage::Off, level: 0.0 },
            fenv: 0.0,
            svf: SvfState::default(),
            lfsr: 0x7fff,
            nacc: 0.0,
            nval: 1.0,
            rng: 0x1234_5678,
            pos: 0.0,
            drum: Drum::default(),
        }
    }
}

pub struct StartArgs {
    pub track_index: usize,
    pub track_id: u64,
    pub pitch: u8,
    pub vel: f32,
    pub end_tick: f64,
    pub live: bool,
    pub serial: u64,
    pub seed: u32,
}

impl Voice {
    pub fn start(&mut self, a: StartArgs, inst: &CInstrument, sr: f64) {
        *self = Voice {
            active: true,
            live: a.live,
            track_id: a.track_id,
            track_index: a.track_index,
            pitch: a.pitch,
            vel: a.vel,
            end_tick: a.end_tick,
            serial: a.serial,
            note: a.pitch as f64,
            target: a.pitch as f64,
            rng: a.seed | 1,
            env: Env { stage: Stage::Attack, level: 0.0 },
            fenv: 1.0,
            ..Voice::default()
        };
        for (i, p) in self.phases.iter_mut().enumerate() {
            *p = if i == 0 { 0.0 } else { ((a.seed.wrapping_mul(2654435761).wrapping_add(i as u32 * 40503)) % 1000) as f64 / 1000.0 };
        }
        match inst {
            CInstrument::Synth(_) => self.kind = VoiceKind::Synth,
            CInstrument::Sampler { .. } => self.kind = VoiceKind::Sampler,
            CInstrument::Drums(p) => {
                self.kind = VoiceKind::Drum;
                self.end_tick = f64::INFINITY; // one-shot: ignores note-off
                self.env = Env { stage: Stage::Sustain, level: 1.0 };
                self.drum = setup_drum(a.pitch, p, sr);
            }
        }
    }

    /// Mono/legato: glide the existing voice to a new note.
    pub fn retarget(&mut self, pitch: u8, vel: f32, end_tick: f64, live: bool) {
        self.pitch = pitch;
        self.target = pitch as f64;
        self.vel = vel;
        self.end_tick = end_tick;
        self.live = live;
        self.releasing = false;
        if self.env.stage == Stage::Release || self.env.stage == Stage::Off {
            self.env.stage = Stage::Attack;
        }
    }

    pub fn release(&mut self) {
        if self.kind == VoiceKind::Drum && !self.fast {
            return;
        }
        self.releasing = true;
        self.env.stage = Stage::Release;
    }

    /// Quick fade-out (voice stealing, choke groups, loop wrap).
    pub fn release_fast(&mut self) {
        self.fast = true;
        self.releasing = true;
        self.env.stage = Stage::Release;
    }

    pub fn age_key(&self) -> (bool, u64) {
        (!self.releasing, self.serial)
    }
}

/// Renders one voice, adding into the track buffers.
/// `cutoff_oct` and `pitch` come from automation (filter offset in octaves, pitch bend in semitones).
#[allow(clippy::too_many_arguments)]
pub fn render_voice(
    v: &mut Voice,
    inst: &CInstrument,
    sr: f64,
    bpm: f64,
    cutoff_oct: f32,
    pitch: f32,
    l: &mut [f32],
    r: &mut [f32],
) {
    match (v.kind, inst) {
        (VoiceKind::Synth, CInstrument::Synth(p)) => render_synth(v, p, sr, bpm, cutoff_oct, pitch, l, r),
        (VoiceKind::Sampler, CInstrument::Sampler { params, data }) => match data {
            Some(d) => render_sampler(v, params, d, sr, pitch, l, r),
            None => v.active = false,
        },
        (VoiceKind::Drum, CInstrument::Drums(p)) => render_drum(v, p, sr, l, r),
        _ => v.active = false,
    }
}

#[inline]
fn tri(ph: f64) -> f64 {
    if ph < 0.5 {
        4.0 * ph - 1.0
    } else {
        3.0 - 4.0 * ph
    }
}

#[allow(clippy::too_many_arguments)]
fn render_synth(v: &mut Voice, p: &SynthParams, sr: f64, bpm: f64, cutoff_oct: f32, bend: f32, l: &mut [f32], r: &mut [f32]) {
    let n = l.len();
    let block_secs = n as f64 / sr;

    // Pitch, updated once per block (blocks are 16 frames, ~3 kHz control rate).
    if p.mono && p.glide > 0.0005 {
        let coef = (-block_secs * 4.0 / p.glide as f64).exp();
        v.note = v.target + (v.note - v.target) * coef;
    } else {
        v.note = v.target;
    }
    let mut semis = v.note + p.octave as f64 * 12.0 + p.fine as f64 / 100.0 + bend as f64;
    if p.vibrato_depth > 0.0 && v.t > p.vibrato_delay as f64 {
        let tv = v.t - p.vibrato_delay as f64;
        let fade = (tv / 0.15).min(1.0);
        semis += p.vibrato_depth as f64 * fade * (TAU * p.vibrato_rate as f64 * tv).sin();
    }
    if p.sweep != 0.0 && p.sweep_time > 0.0 && v.t < p.sweep_time as f64 {
        semis += p.sweep as f64 * (1.0 - v.t / p.sweep_time as f64);
    }
    let beats = v.t * bpm / 60.0;
    if p.arp.len() > 1 && p.arp_speed > 0.0 {
        let steps = if p.arp_sync { beats * p.arp_speed as f64 } else { v.t * p.arp_speed as f64 };
        semis += p.arp[(steps as usize) % p.arp.len()] as f64;
    }
    let lfo = if p.lfo_cutoff != 0.0 || p.lfo_pwm != 0.0 || p.lfo_amp != 0.0 {
        let cycles = if p.lfo_sync { beats * p.lfo_rate as f64 } else { v.t * p.lfo_rate as f64 };
        (TAU * cycles).sin() as f32
    } else {
        0.0
    };
    let freq = midi_to_hz(semis);
    let dt = (freq / sr).min(0.45);

    let uni = (p.unison as usize).clamp(1, MAX_UNISON);
    let mut dts = [0.0f64; MAX_UNISON];
    for (u, d) in dts.iter_mut().enumerate().take(uni) {
        let cents = if uni == 1 { 0.0 } else { p.detune as f64 * ((u as f64 / (uni - 1) as f64) * 2.0 - 1.0) };
        *d = (dt * (cents / 1200.0).exp2()).min(0.45);
    }
    let uni_norm = 1.0 / (uni as f64).sqrt();
    let duty = (p.duty + lfo * p.lfo_pwm).clamp(0.02, 0.98) as f64;

    let filt = if p.filter_on {
        let oct = p.filter_env * v.fenv + p.lfo_cutoff * lfo + cutoff_oct;
        Some(SvfCoefs::new(p.cutoff as f64 * (oct as f64).exp2(), p.resonance, sr))
    } else {
        None
    };
    let fenv_coef = exp_coef(p.filter_decay as f64, sr);

    let rel = if v.fast { 0.004 } else { p.release.max(0.002) as f64 };
    let att_inc = (1.0 / (p.attack.max(0.0008) as f64 * sr)) as f32;
    let dec_coef = exp_coef(p.decay.max(0.001) as f64, sr);
    let rel_coef = exp_coef(rel, sr);
    let sus = p.sustain.clamp(0.0, 1.0);
    let amp = p.gain * (1.0 - p.vel_sens + p.vel_sens * v.vel) * (1.0 - p.lfo_amp * 0.5 * (1.0 + lfo));

    let fm_index = if p.fm_amount > 0.0 {
        if p.fm_decay > 0.0 {
            p.fm_amount as f64 * (-v.t / p.fm_decay as f64).exp()
        } else {
            p.fm_amount as f64
        }
    } else {
        0.0
    };
    let mdt = dt * p.fm_ratio as f64;
    let noise_rate = freq * 16.0 / sr;
    let table_ok = p.wavetable.len() == 32;

    for i in 0..n {
        let e = v.env.next(att_inc, dec_coef, sus, rel_coef);
        let mut s = 0.0f64;
        match p.wave {
            Wave::Noise => {
                v.nacc += noise_rate;
                while v.nacc >= 1.0 {
                    v.nacc -= 1.0;
                    v.nval = lfsr_step(&mut v.lfsr, p.noise_short);
                }
                s = v.nval as f64;
            }
            _ => {
                for u in 0..uni {
                    let ph = v.phases[u];
                    let d = dts[u];
                    s += match p.wave {
                        Wave::Pulse => {
                            let mut x = if ph < duty { 1.0 } else { -1.0 };
                            x += poly_blep(ph, d);
                            x -= poly_blep((ph + 1.0 - duty) % 1.0, d);
                            x - (2.0 * duty - 1.0)
                        }
                        Wave::Saw => 2.0 * ph - 1.0 - poly_blep(ph, d),
                        Wave::Triangle => tri(ph),
                        Wave::Sine => {
                            if fm_index > 0.0 {
                                (TAU * ph + fm_index * (TAU * v.mphase).sin()).sin()
                            } else {
                                (TAU * ph).sin()
                            }
                        }
                        Wave::Wavetable => {
                            if table_ok {
                                p.wavetable[((ph * 32.0) as usize).min(31)] as f64 / 7.5 - 1.0
                            } else {
                                0.0
                            }
                        }
                        Wave::Noise => 0.0,
                    };
                    let mut nph = ph + d;
                    if nph >= 1.0 {
                        nph -= 1.0;
                    }
                    v.phases[u] = nph;
                }
                s *= uni_norm;
                if fm_index > 0.0 {
                    v.mphase += mdt;
                    if v.mphase >= 1.0 {
                        v.mphase -= v.mphase.floor();
                    }
                }
            }
        }
        let mut x = s as f32;
        if let Some(c) = &filt {
            x = v.svf.tick(c, x).0;
        }
        x = quantize(x * e, p.bits) * amp;
        l[i] += x;
        r[i] += x;
        v.fenv *= fenv_coef;
    }
    v.t += block_secs;
    if v.env.stage == Stage::Off {
        v.active = false;
    }
}

fn render_sampler(
    v: &mut Voice,
    p: &SamplerParams,
    d: &crate::assets::AudioData,
    sr: f64,
    bend: f32,
    l: &mut [f32],
    r: &mut [f32],
) {
    let rate = d.sr as f64 / sr * ((v.pitch as f64 - p.root as f64 + p.fine as f64 / 100.0 + bend as f64) / 12.0).exp2();
    let len = d.frames() as f64;
    let att_inc = (1.0 / (p.attack.max(0.0005) as f64 * sr)) as f32;
    let rel = if v.fast { 0.004 } else { p.release.max(0.002) as f64 };
    let rel_coef = exp_coef(rel, sr);
    let amp = p.gain * (0.4 + 0.6 * v.vel);
    for i in 0..l.len() {
        if v.pos >= len {
            if p.looped && len > 4.0 {
                v.pos -= len;
            } else {
                v.active = false;
                return;
            }
        }
        let e = v.env.next(att_inc, 1.0, 1.0, rel_coef);
        l[i] += hermite(&d.l, v.pos) * e * amp;
        r[i] += hermite(&d.r, v.pos) * e * amp;
        v.pos += rate;
    }
    if v.env.stage == Stage::Off {
        v.active = false;
    }
}

/// General MIDI-ish drum map.
fn setup_drum(pitch: u8, p: &DrumParams, sr: f64) -> Drum {
    let chip = p.style == "chip";
    let electro = p.style == "electro";
    let tm = (p.tune as f64 / 12.0).exp2();
    let dm = p.decay.clamp(0.1, 4.0) as f64;
    let hp = |hz: f64| (1.0 - (-TAU * hz / sr).exp()) as f32;
    let mut d = Drum { gain: p.gain, crush: chip, white: !chip, ..Drum::default() };
    let pc = |secs: f64| (-1.0 / (secs * sr)).exp();
    match pitch {
        35 | 36 => {
            d.tone_wave = if chip { 1 } else { 0 };
            let (f0, f1, dec) = if electro { (130.0, 42.0, 0.55) } else { (160.0, 48.0, 0.28) };
            d.freq = f0 * tm;
            d.freq_end = f1 * tm;
            d.pcoef = pc(if electro { 0.045 } else { 0.03 });
            d.tamp = 1.0;
            d.tcoef = exp_coef(dec * dm, sr);
            d.namp = if electro { 0.15 } else { 0.3 };
            d.ncoef = exp_coef(0.012, sr);
            d.nrate = 9000.0;
        }
        37 => {
            d.tone_wave = 2;
            d.freq = 700.0 * tm;
            d.freq_end = d.freq;
            d.pcoef = 1.0;
            d.tamp = 0.5;
            d.tcoef = exp_coef(0.03 * dm, sr);
            d.namp = 0.4;
            d.ncoef = exp_coef(0.025 * dm, sr);
            d.nrate = 14000.0;
        }
        38 | 40 => {
            d.tone_wave = if chip { 1 } else { 0 };
            d.freq = if electro { 200.0 } else { 230.0 } * tm;
            d.freq_end = 170.0 * tm;
            d.pcoef = pc(0.02);
            d.tamp = if electro { 0.6 } else { 0.5 };
            d.tcoef = exp_coef(0.08 * dm, sr);
            d.namp = 0.8;
            d.ncoef = exp_coef(if electro { 0.25 } else { 0.17 } * dm, sr);
            d.nrate = 12000.0;
            d.hp_coef = hp(if electro { 900.0 } else { 400.0 });
        }
        39 => {
            d.namp = 0.9;
            d.ncoef = exp_coef(0.22 * dm, sr);
            d.nrate = 11000.0;
            d.clap = true;
            d.hp_coef = hp(800.0);
        }
        42 | 44 | 46 => {
            d.namp = if pitch == 46 { 0.45 } else { 0.5 };
            d.ncoef = exp_coef(if pitch == 46 { 0.35 } else { 0.05 } * dm, sr);
            d.nrate = 32000.0;
            d.short = chip;
            d.metal = electro;
            d.hp_coef = hp(if chip { 2000.0 } else { 6000.0 });
        }
        41 | 43 | 45 | 47 | 48 | 50 => {
            let base = match pitch {
                41 => 90.0,
                43 => 110.0,
                45 => 135.0,
                47 => 160.0,
                48 => 190.0,
                _ => 230.0,
            };
            d.tone_wave = if chip { 1 } else { 0 };
            d.freq = base * 1.6 * tm;
            d.freq_end = base * tm;
            d.pcoef = pc(0.05);
            d.tamp = 0.9;
            d.tcoef = exp_coef(0.35 * dm, sr);
            d.namp = 0.15;
            d.ncoef = exp_coef(0.02, sr);
            d.nrate = 8000.0;
        }
        49 | 52 | 55 | 57 => {
            d.namp = 0.55;
            d.ncoef = exp_coef(1.4 * dm, sr);
            d.nrate = 26000.0;
            d.metal = electro;
            d.hp_coef = hp(3000.0);
        }
        51 | 53 | 59 => {
            d.namp = 0.3;
            d.ncoef = exp_coef(0.7 * dm, sr);
            d.nrate = 20000.0;
            d.short = chip;
            d.metal = true;
            d.hp_coef = hp(4000.0);
        }
        _ => {
            // Pitched percussion blip for any other key.
            d.tone_wave = if chip { 2 } else { 0 };
            d.freq = midi_to_hz(pitch as f64) * tm;
            d.freq_end = d.freq;
            d.pcoef = 1.0;
            d.tamp = 0.6;
            d.tcoef = exp_coef(0.18 * dm, sr);
        }
    }
    d
}

const METAL_HZ: [f64; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

fn render_drum(v: &mut Voice, _p: &DrumParams, sr: f64, l: &mut [f32], r: &mut [f32]) {
    let d = &mut v.drum;
    let rel_coef = exp_coef(0.004, sr);
    for i in 0..l.len() {
        let mut s = 0.0f32;
        if d.tamp > 1e-4 {
            let w = match d.tone_wave {
                0 => (TAU * d.phase).sin(),
                1 => tri(d.phase),
                _ => {
                    if d.phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            s += w as f32 * d.tamp;
            d.phase += d.freq / sr;
            if d.phase >= 1.0 {
                d.phase -= 1.0;
            }
            d.freq = d.freq_end + (d.freq - d.freq_end) * d.pcoef;
            d.tamp *= d.tcoef;
        }
        if d.namp > 1e-4 {
            let mut x = if d.metal {
                let mut m = 0.0;
                for (k, ph) in d.metal_ph.iter_mut().enumerate() {
                    *ph += METAL_HZ[k] * 1.7 / sr;
                    if *ph >= 1.0 {
                        *ph -= 1.0;
                    }
                    m += if *ph < 0.5 { 1.0 } else { -1.0 };
                }
                m as f32 / 6.0
            } else if d.white {
                xorshift(&mut v.rng)
            } else {
                v.nacc += d.nrate / sr;
                while v.nacc >= 1.0 {
                    v.nacc -= 1.0;
                    v.nval = lfsr_step(&mut v.lfsr, d.short);
                }
                v.nval
            };
            if d.hp_coef > 0.0 {
                d.hp_state += (x - d.hp_state) * d.hp_coef;
                x -= d.hp_state;
            }
            let mut a = d.namp;
            if d.clap && v.t < 0.026 {
                a *= 1.0 - ((v.t / 0.0087).fract()) as f32;
            }
            s += x * a;
            d.namp *= d.ncoef;
        }
        if d.crush {
            s = quantize(s, 4);
        }
        let e = v.env.next(0.0, 1.0, 1.0, rel_coef);
        let out = s * d.gain * (0.35 + 0.65 * v.vel) * e;
        l[i] += out;
        r[i] += out;
        v.t += 1.0 / sr;
    }
    if (d.tamp <= 1e-4 && d.namp <= 1e-4) || v.env.stage == Stage::Off {
        v.active = false;
    }
}
