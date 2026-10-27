//! The real-time audio engine. `Engine::process` runs inside the audio
//! callback: it never allocates, locks or blocks. The main thread talks to it
//! only through lock-free ring buffers (`EngineMsg` in, `Garbage` out) and
//! reads its state back through atomics in `Shared`.

pub mod compile;
pub mod dsp;
pub mod effects;
pub mod synth;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer};

use crate::model::PPQ;
use compile::{CInstrument, CTarget, CompiledSong};
use dsp::hermite;
use effects::FxCtx;
use synth::{StartArgs, Voice, VoiceKind};

/// Frames per internal processing block (event timing resolution: ~0.33 ms at 48 kHz).
pub const SUB: usize = 16;
pub const MAX_TRACKS: usize = 128;
pub const MAX_VOICES: usize = 192;
pub const VOICES_PER_TRACK: usize = 24;
/// Meter slot for the master bus.
pub const MASTER_METER: usize = MAX_TRACKS;

pub enum EngineMsg {
    SetSong(Box<CompiledSong>),
    Play { record: bool },
    Stop,
    Seek(f64),
    SetMetronome(bool),
    NoteOn { track_id: u64, pitch: u8, vel: f32 },
    NoteOff { track_id: u64, pitch: u8 },
    AllNotesOff,
    SetMonitorInput(Option<Box<Consumer<f32>>>),
}

/// Things the audio thread hands back so they're freed on the main thread.
// The payloads are never read: they only exist to be dropped off the audio thread.
#[allow(dead_code)]
pub enum Garbage {
    Song(Box<CompiledSong>),
    Input(Box<Consumer<f32>>),
}

pub struct Shared {
    position: AtomicU64,
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    record_start: AtomicU64,
    #[allow(dead_code)]
    pub sample_rate: AtomicU32,
    pub latency_frames: AtomicU32,
    pub input_latency_frames: AtomicU32,
    pub num_tracks: AtomicU32,
    meters: Vec<AtomicU32>,
    input_meter: AtomicU32,
    load: AtomicU32,
}

impl Shared {
    pub fn new(sr: u32) -> Self {
        Shared {
            position: AtomicU64::new(0f64.to_bits()),
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            record_start: AtomicU64::new(0f64.to_bits()),
            sample_rate: AtomicU32::new(sr),
            latency_frames: AtomicU32::new(0),
            input_latency_frames: AtomicU32::new(0),
            num_tracks: AtomicU32::new(0),
            meters: (0..=MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            input_meter: AtomicU32::new(0),
            load: AtomicU32::new(0),
        }
    }
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }
    pub fn record_start(&self) -> f64 {
        f64::from_bits(self.record_start.load(Ordering::Relaxed))
    }
    pub fn meter(&self, i: usize) -> f32 {
        f32::from_bits(self.meters[i].load(Ordering::Relaxed))
    }
    pub fn input_meter(&self) -> f32 {
        f32::from_bits(self.input_meter.load(Ordering::Relaxed))
    }
    pub fn set_input_meter(&self, v: f32) {
        self.input_meter.store(v.to_bits(), Ordering::Relaxed)
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Default)]
struct TrackMods {
    volume: f32,
    pan: f32,
    cutoff: f32,
    pitch: f32,
}

#[derive(Clone, Copy)]
struct StereoBuf {
    l: [f32; SUB],
    r: [f32; SUB],
}

#[derive(Default)]
struct Click {
    phase: f64,
    freq: f64,
    amp: f32,
}

pub struct Engine {
    sr: f64,
    song: Box<CompiledSong>,
    shared: Arc<Shared>,
    rx: Consumer<EngineMsg>,
    garbage: Producer<Garbage>,
    voices: Vec<Voice>,
    playing: bool,
    recording: bool,
    position: f64,
    play_start: f64,
    metronome: bool,
    click: Click,
    bufs: Vec<StereoBuf>,
    mods: Vec<TrackMods>,
    meters: Vec<f32>,
    monitor: Option<Box<Consumer<f32>>>,
    mon_l: [f32; SUB],
    mon_r: [f32; SUB],
    master_l: [f32; SUB],
    master_r: [f32; SUB],
    limiter_gain: f32,
    serial: u64,
    seed: u32,
}

impl Engine {
    pub fn new(sr: f64, shared: Arc<Shared>, rx: Consumer<EngineMsg>, garbage: Producer<Garbage>) -> Self {
        Engine {
            sr,
            song: Box::new(CompiledSong::empty()),
            shared,
            rx,
            garbage,
            voices: vec![Voice::default(); MAX_VOICES],
            playing: false,
            recording: false,
            position: 0.0,
            play_start: 0.0,
            metronome: false,
            click: Click::default(),
            bufs: vec![StereoBuf { l: [0.0; SUB], r: [0.0; SUB] }; MAX_TRACKS],
            mods: vec![TrackMods::default(); MAX_TRACKS],
            meters: vec![0.0; MAX_TRACKS + 1],
            monitor: None,
            mon_l: [0.0; SUB],
            mon_r: [0.0; SUB],
            master_l: [0.0; SUB],
            master_r: [0.0; SUB],
            limiter_gain: 1.0,
            serial: 0,
            seed: 0x9e37_79b9,
        }
    }

    /// Fill an interleaved output buffer.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        let started = std::time::Instant::now();
        self.handle_messages();
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(SUB);
            self.render_block(n);
            for i in 0..n {
                let o = (done + i) * channels;
                out[o] = self.master_l[i];
                if channels > 1 {
                    out[o + 1] = self.master_r[i];
                    for c in 2..channels {
                        out[o + c] = 0.0;
                    }
                }
            }
            done += n;
        }
        let s = &self.shared;
        s.position.store(self.position.to_bits(), Ordering::Relaxed);
        s.playing.store(self.playing, Ordering::Relaxed);
        s.num_tracks.store(self.song.tracks.len().min(MAX_TRACKS) as u32, Ordering::Relaxed);
        for (i, m) in self.meters.iter().enumerate() {
            s.meters[i].store(m.to_bits(), Ordering::Relaxed);
        }
        if frames > 0 {
            let budget = frames as f64 / self.sr;
            let load = (started.elapsed().as_secs_f64() / budget) as f32;
            let prev = s.load();
            s.load.store((prev * 0.9 + load * 0.1).to_bits(), Ordering::Relaxed);
        }
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.pop() {
            match msg {
                EngineMsg::SetSong(mut song) => {
                    transfer_state(&mut song, &mut self.song);
                    let old = std::mem::replace(&mut self.song, song);
                    self.remap_voices();
                    // If the garbage queue is full this frees on the audio thread; rare and harmless.
                    let _ = self.garbage.push(Garbage::Song(old));
                }
                EngineMsg::Play { record } => {
                    if !self.playing {
                        self.playing = true;
                        self.play_start = self.position;
                    }
                    if record {
                        self.recording = true;
                        self.shared.record_start.store(self.position.to_bits(), Ordering::Relaxed);
                        self.shared.recording.store(true, Ordering::Release);
                    }
                }
                EngineMsg::Stop => {
                    if self.playing {
                        self.playing = false;
                        self.position = self.play_start;
                    } else {
                        self.position = 0.0;
                    }
                    self.recording = false;
                    self.shared.recording.store(false, Ordering::Release);
                    self.release_scheduled();
                }
                EngineMsg::Seek(t) => {
                    self.position = t.max(0.0);
                    if self.playing {
                        self.play_start = self.position;
                        self.release_scheduled();
                    }
                }
                EngineMsg::SetMetronome(on) => self.metronome = on,
                EngineMsg::NoteOn { track_id, pitch, vel } => {
                    if let Some(ti) = self.song.tracks.iter().position(|t| t.id == track_id) {
                        self.trigger(ti, pitch, vel, f64::INFINITY, true);
                    }
                }
                EngineMsg::NoteOff { track_id, pitch } => {
                    for v in self.voices.iter_mut() {
                        if v.active && v.live && v.track_id == track_id && v.pitch == pitch && !v.releasing {
                            v.release();
                        }
                    }
                }
                EngineMsg::AllNotesOff => {
                    for v in self.voices.iter_mut().filter(|v| v.active) {
                        v.release_fast();
                    }
                }
                EngineMsg::SetMonitorInput(c) => {
                    if let Some(old) = std::mem::replace(&mut self.monitor, c) {
                        let _ = self.garbage.push(Garbage::Input(old));
                    }
                }
            }
        }
    }

    fn remap_voices(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.active) {
            match self.song.tracks.iter().position(|t| t.id == v.track_id) {
                Some(ti) => {
                    let ok = matches!(
                        (&self.song.tracks[ti].instrument, v.kind),
                        (Some(CInstrument::Synth(_)), VoiceKind::Synth)
                            | (Some(CInstrument::Sampler { .. }), VoiceKind::Sampler)
                            | (Some(CInstrument::Drums(_)), VoiceKind::Drum)
                    );
                    if ok {
                        v.track_index = ti;
                    } else {
                        v.active = false;
                    }
                }
                None => v.active = false,
            }
        }
    }

    /// Release voices that came from the timeline (not live keyboard notes).
    fn release_scheduled(&mut self) {
        for v in self.voices.iter_mut() {
            if v.active && !v.live {
                v.release_fast();
            }
        }
    }

    fn trigger(&mut self, ti: usize, pitch: u8, vel: f32, end: f64, live: bool) {
        let tr = &self.song.tracks[ti];
        let Some(inst) = &tr.instrument else { return };
        let track_id = tr.id;

        match inst {
            CInstrument::Synth(p) if p.mono => {
                if let Some(v) =
                    self.voices.iter_mut().find(|v| v.active && v.track_id == track_id && !v.releasing)
                {
                    v.retarget(pitch, vel, end, live);
                    return;
                }
            }
            CInstrument::Drums(_) => {
                // Choke groups: same key retriggers, closed hat chokes open hat.
                for v in self.voices.iter_mut() {
                    if v.active
                        && v.track_id == track_id
                        && (v.pitch == pitch || (v.pitch == 46 && matches!(pitch, 42 | 44)))
                    {
                        v.release_fast();
                    }
                }
            }
            _ => {}
        }

        let count = self.voices.iter().filter(|v| v.active && v.track_id == track_id).count();
        let slot = if count >= VOICES_PER_TRACK {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.active && v.track_id == track_id)
                .min_by_key(|(_, v)| v.age_key())
                .map(|(i, _)| i)
        } else {
            self.voices.iter().position(|v| !v.active).or_else(|| {
                self.voices.iter().enumerate().min_by_key(|(_, v)| v.age_key()).map(|(i, _)| i)
            })
        };
        let Some(slot) = slot else { return };
        self.serial += 1;
        self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let args = StartArgs {
            track_index: ti,
            track_id,
            pitch,
            vel,
            end_tick: end,
            live,
            serial: self.serial,
            seed: self.seed,
        };
        self.voices[slot].start(args, inst, self.sr);
    }

    fn render_block(&mut self, n: usize) {
        let nt = self.song.tracks.len().min(MAX_TRACKS);
        for b in self.bufs[..nt].iter_mut() {
            b.l[..n].fill(0.0);
            b.r[..n].fill(0.0);
        }
        let t0 = self.position;
        let bpm = self.song.tempo.bpm_at(t0);
        let tps = bpm * PPQ as f64 / 60.0;
        let tpf = tps / self.sr;

        // Automation, evaluated once per block at the current position.
        for ti in 0..nt {
            let tr = &mut self.song.tracks[ti];
            let mut m = TrackMods { volume: tr.volume, pan: tr.pan, cutoff: 0.0, pitch: 0.0 };
            for fx in tr.effects.iter_mut() {
                fx.wet = 1.0;
            }
            for lane in &tr.auto {
                let v = lane.value_at(t0);
                match lane.target {
                    CTarget::Volume => m.volume = v.clamp(0.0, 2.0),
                    CTarget::Pan => m.pan = v.clamp(-1.0, 1.0),
                    CTarget::Cutoff => m.cutoff = v,
                    CTarget::Pitch => m.pitch = v,
                    CTarget::FxMix(i) => tr.effects[i].wet = v.clamp(0.0, 1.0),
                    CTarget::FxCutoff(i) => tr.effects[i].set_cutoff(v.clamp(20.0, 20000.0)),
                }
            }
            self.mods[ti] = m;
        }
        let mut master_volume = self.song.master_volume;
        for fx in self.song.master_effects.iter_mut() {
            fx.wet = 1.0;
        }
        for lane in &self.song.master_auto {
            let v = lane.value_at(t0);
            match lane.target {
                CTarget::Volume => master_volume = v.clamp(0.0, 2.0),
                CTarget::FxMix(i) => self.song.master_effects[i].wet = v.clamp(0.0, 1.0),
                CTarget::FxCutoff(i) => self.song.master_effects[i].set_cutoff(v.clamp(20.0, 20000.0)),
                _ => {}
            }
        }
        let mut t1 = t0 + tpf * n as f64;
        let mut wrap = false;

        if self.playing {
            let s = &self.song;
            if s.loop_enabled && !self.recording && s.loop_end > s.loop_start && t0 < s.loop_end && t1 >= s.loop_end {
                t1 = s.loop_end;
                wrap = true;
            }
            for ti in 0..nt {
                let notes_len = self.song.tracks[ti].notes.len();
                if notes_len == 0 {
                    continue;
                }
                let mut i = self.song.tracks[ti].notes.partition_point(|x| x.start < t0);
                while i < notes_len {
                    let note = self.song.tracks[ti].notes[i];
                    if note.start >= t1 {
                        break;
                    }
                    self.trigger(ti, note.pitch, note.vel, note.end, false);
                    i += 1;
                }
            }
            for v in self.voices.iter_mut() {
                if v.active && !v.live && !v.releasing && v.end_tick <= t1 {
                    v.release();
                }
            }
        }

        // Voices
        {
            let sr = self.sr;
            let song = &self.song;
            let bufs = &mut self.bufs;
            let mods = &self.mods;
            for v in self.voices.iter_mut().filter(|v| v.active) {
                let ti = v.track_index;
                if ti >= nt {
                    v.active = false;
                    continue;
                }
                match &song.tracks[ti].instrument {
                    Some(inst) => {
                        let b = &mut bufs[ti];
                        let m = &mods[ti];
                        synth::render_voice(v, inst, sr, bpm, m.cutoff, m.pitch, &mut b.l[..n], &mut b.r[..n]);
                    }
                    None => v.active = false,
                }
            }
        }

        // Audio clips
        if self.playing {
            let secs0 = self.song.tempo.tick_to_secs(t0);
            for ti in 0..nt {
                let tr = &self.song.tracks[ti];
                if tr.audio_clips.is_empty() {
                    continue;
                }
                let b = &mut self.bufs[ti];
                for clip in &tr.audio_clips {
                    if clip.end <= t0 || clip.start >= t1 {
                        continue;
                    }
                    let d = &clip.data;
                    let frames = d.frames() as f64;
                    let fade_ticks = tps * 0.003;
                    for i in 0..n {
                        let tick = t0 + tpf * i as f64;
                        if tick < clip.start || tick >= clip.end {
                            continue;
                        }
                        let pos = (secs0 + i as f64 / self.sr - clip.start_secs + clip.offset) * d.sr as f64;
                        if pos >= frames {
                            continue;
                        }
                        // 3 ms fades at clip edges avoid clicks.
                        let edge = (tick - clip.start).min(clip.end - tick);
                        let g = clip.gain * if edge < fade_ticks { (edge / fade_ticks) as f32 } else { 1.0 };
                        b.l[i] += hermite(&d.l, pos) * g;
                        b.r[i] += hermite(&d.r, pos) * g;
                    }
                }
            }
        }

        // Live input monitoring
        if let Some(mon) = self.monitor.as_mut() {
            let max_backlog = (self.sr * 0.03) as usize * 2;
            while mon.slots() > max_backlog + n * 2 {
                let _ = mon.pop();
            }
            let have = mon.slots() >= n * 2;
            for i in 0..n {
                if have {
                    self.mon_l[i] = mon.pop().unwrap_or(0.0);
                    self.mon_r[i] = mon.pop().unwrap_or(0.0);
                } else {
                    self.mon_l[i] = 0.0;
                    self.mon_r[i] = 0.0;
                }
            }
            for ti in 0..nt {
                if self.song.tracks[ti].monitor_input {
                    let b = &mut self.bufs[ti];
                    for i in 0..n {
                        b.l[i] += self.mon_l[i];
                        b.r[i] += self.mon_r[i];
                    }
                }
            }
        }

        // Mix
        let ctx = FxCtx { tick: t0, ticks_per_frame: tpf, playing: self.playing };
        self.master_l[..n].fill(0.0);
        self.master_r[..n].fill(0.0);
        let meter_decay = (-(n as f64) / (self.sr * 0.25)).exp() as f32;
        let any_solo = self.song.any_solo;
        for ti in 0..nt {
            let tr = &mut self.song.tracks[ti];
            let b = &mut self.bufs[ti];
            for fx in tr.effects.iter_mut() {
                if fx.enabled {
                    fx.process(&mut b.l[..n], &mut b.r[..n], &ctx);
                }
            }
            let audible = !tr.mute && (!any_solo || tr.solo);
            let m = self.mods[ti];
            let gl = m.volume * (1.0 - m.pan).min(1.0);
            let gr = m.volume * (1.0 + m.pan).min(1.0);
            let mut peak = 0.0f32;
            for i in 0..n {
                let l = b.l[i] * gl;
                let r = b.r[i] * gr;
                peak = peak.max(l.abs()).max(r.abs());
                if audible {
                    self.master_l[i] += l;
                    self.master_r[i] += r;
                }
            }
            if !audible {
                peak = 0.0;
            }
            self.meters[ti] = (self.meters[ti] * meter_decay).max(peak);
        }
        for m in self.meters[nt..MAX_TRACKS].iter_mut() {
            *m = 0.0;
        }

        for fx in self.song.master_effects.iter_mut() {
            if fx.enabled {
                fx.process(&mut self.master_l[..n], &mut self.master_r[..n], &ctx);
            }
        }
        let mv = master_volume;
        let rel = (1.0 / (self.sr * 0.15)) as f32;
        let mut peak = 0.0f32;
        for i in 0..n {
            let mut l = self.master_l[i] * mv;
            let mut r = self.master_r[i] * mv;
            if !l.is_finite() || !r.is_finite() {
                l = 0.0;
                r = 0.0;
            }
            // Simple peak limiter: instant attack, smooth release.
            let p = l.abs().max(r.abs());
            if p * self.limiter_gain > 0.98 {
                self.limiter_gain = 0.98 / p;
            } else {
                self.limiter_gain += (1.0 - self.limiter_gain) * rel;
            }
            l *= self.limiter_gain;
            r *= self.limiter_gain;
            peak = peak.max(l.abs()).max(r.abs());
            self.master_l[i] = l;
            self.master_r[i] = r;
        }
        self.meters[MASTER_METER] = (self.meters[MASTER_METER] * meter_decay).max(peak);

        // Metronome (after the limiter, never exported)
        if self.playing && self.metronome {
            let beat = self.song.beat_ticks;
            let first = (t0 / beat).ceil() * beat;
            if first < t1 {
                let idx = (first / beat).round() as i64;
                let accent = idx % self.song.beats_per_bar as i64 == 0;
                self.click = Click { phase: 0.0, freq: if accent { 1760.0 } else { 1175.0 }, amp: 0.35 };
            }
        }
        if self.click.amp > 1e-4 {
            let coef = (-1.0 / (self.sr * 0.012)).exp() as f32;
            for i in 0..n {
                let s = (std::f64::consts::TAU * self.click.phase).sin() as f32 * self.click.amp;
                self.click.phase += self.click.freq / self.sr;
                self.click.amp *= coef;
                self.master_l[i] += s;
                self.master_r[i] += s;
            }
        }

        if self.playing {
            if wrap {
                self.position = self.song.loop_start;
                self.release_scheduled();
            } else {
                self.position = t1;
            }
        }
    }
}

/// Move effect state from the old song into matching effects of the new one.
fn transfer_state(new: &mut CompiledSong, old: &mut CompiledSong) {
    for nt in new.tracks.iter_mut() {
        if let Some(ot) = old.tracks.iter_mut().find(|t| t.id == nt.id) {
            for ne in nt.effects.iter_mut() {
                if let Some(oe) = ot.effects.iter_mut().find(|e| e.id == ne.id) {
                    ne.inherit(oe);
                }
            }
        }
    }
    for ne in new.master_effects.iter_mut() {
        if let Some(oe) = old.master_effects.iter_mut().find(|e| e.id == ne.id) {
            ne.inherit(oe);
        }
    }
}
