//! The project data model. Everything the user edits lives here as plain
//! serializable data; the audio engine gets a compiled copy (see `engine::compile`).

use serde::{Deserialize, Serialize};

/// Ticks per quarter note.
pub const PPQ: u64 = 96;

fn one() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub name: String,
    /// Tempo at the start of the song.
    pub bpm: f64,
    /// Tempo changes after the start (see `TempoMap`).
    pub tempo_changes: Vec<TempoPoint>,
    pub time_sig_num: u32,
    pub time_sig_den: u32,
    /// 0 = C ... 11 = B. Used for scale highlighting in the editors.
    pub key_root: u8,
    pub scale: String,
    pub loop_start: u64,
    pub loop_end: u64,
    pub loop_enabled: bool,
    pub tracks: Vec<Track>,
    pub patterns: Vec<Pattern>,
    pub assets: Vec<AssetRef>,
    pub master: Master,
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            name: "Untitled".into(),
            bpm: 120.0,
            tempo_changes: vec![],
            time_sig_num: 4,
            time_sig_den: 4,
            key_root: 0,
            scale: "major".into(),
            loop_start: 0,
            loop_end: PPQ * 16,
            loop_enabled: false,
            tracks: vec![],
            patterns: vec![],
            assets: vec![],
            master: Master::default(),
            next_id: 1,
        }
    }
}

impl Project {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn beat_ticks(&self) -> u64 {
        PPQ * 4 / self.time_sig_den.max(1) as u64
    }

    pub fn bar_ticks(&self) -> u64 {
        self.beat_ticks() * self.time_sig_num.max(1) as u64
    }

    pub fn tempo_map(&self) -> TempoMap {
        TempoMap::new(self.bpm, &self.tempo_changes)
    }

    /// Length in ticks of `secs` of real time starting at `start`, following tempo changes.
    pub fn duration_ticks(&self, start: u64, secs: f64) -> u64 {
        let m = self.tempo_map();
        let end = m.secs_to_tick(m.tick_to_secs(start as f64) + secs);
        ((end - start as f64).round() as u64).max(1)
    }

    pub fn track(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: u64) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn pattern(&self, id: u64) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    pub fn pattern_mut(&mut self, id: u64) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|p| p.id == id)
    }

    pub fn asset(&self, id: u64) -> Option<&AssetRef> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// End of the last clip on any track (including video/images).
    pub fn end_tick_any(&self) -> u64 {
        self.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.start + c.length).max().unwrap_or(0)
    }

    /// End of the last clip on a track that makes sound.
    pub fn audio_end_tick(&self) -> u64 {
        self.tracks
            .iter()
            .filter(|t| !t.is_media())
            .flat_map(|t| t.clips.iter())
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(0)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub color: String,
    pub kind: TrackKind,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    /// Armed for recording (audio tracks record the input, instrument tracks record MIDI/typing keyboard).
    #[serde(default)]
    pub armed: bool,
    /// Hear the live input through this track (audio tracks only).
    #[serde(default)]
    pub monitor: bool,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
}

impl Track {
    pub fn instrument(&self) -> Option<&Instrument> {
        match &self.kind {
            TrackKind::Instrument { instrument } => Some(instrument),
            _ => None,
        }
    }
    pub fn is_instrument(&self) -> bool {
        matches!(self.kind, TrackKind::Instrument { .. })
    }
    pub fn is_audio(&self) -> bool {
        matches!(self.kind, TrackKind::Audio)
    }
    pub fn is_media(&self) -> bool {
        matches!(self.kind, TrackKind::Media)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TrackKind {
    Instrument { instrument: Instrument },
    Audio,
    Media,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Instrument {
    Synth(SynthParams),
    Sampler(SamplerParams),
    Drums(DrumParams),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Wave {
    Pulse,
    Triangle,
    Saw,
    Sine,
    Noise,
    /// 32-step, 4-bit wavetable like the Game Boy wave channel.
    Wavetable,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SynthParams {
    pub wave: Wave,
    /// Pulse width (0.125, 0.25, 0.5, 0.75 are the classic chip duty cycles).
    pub duty: f32,
    /// 32 values in 0..=15.
    pub wavetable: Vec<u8>,
    /// Noise: 7-bit LFSR mode (metallic/tonal), like the GB noise channel.
    pub noise_short: bool,
    pub unison: u8,
    /// Unison spread in cents.
    pub detune: f32,
    pub octave: i8,
    /// Fine tune in cents.
    pub fine: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub filter_on: bool,
    pub cutoff: f32,
    /// 0..1
    pub resonance: f32,
    /// Filter envelope amount in octaves.
    pub filter_env: f32,
    pub filter_decay: f32,
    /// Vibrato depth in semitones.
    pub vibrato_depth: f32,
    pub vibrato_rate: f32,
    /// Seconds before vibrato kicks in (the classic delayed-vibrato lead sound).
    pub vibrato_delay: f32,
    /// Portamento time in seconds (mono mode).
    pub glide: f32,
    pub mono: bool,
    /// Pitch sweep start offset in semitones, falling to 0 over `sweep_time`.
    pub sweep: f32,
    pub sweep_time: f32,
    /// Arpeggio offsets in semitones, cycled at `arp_speed` steps per second
    /// (or steps per beat when `arp_sync` is on).
    pub arp: Vec<i8>,
    pub arp_speed: f32,
    pub arp_sync: bool,
    /// LFO (sine) in Hz, or cycles per beat when `lfo_sync` is on.
    pub lfo_rate: f32,
    pub lfo_sync: bool,
    /// LFO → filter cutoff, in octaves.
    pub lfo_cutoff: f32,
    /// LFO → pulse width (0..0.45).
    pub lfo_pwm: f32,
    /// LFO → volume (tremolo, 0..1).
    pub lfo_amp: f32,
    /// FM (sine wave only): modulation index, modulator ratio, index decay time (0 = no decay).
    pub fm_amount: f32,
    pub fm_ratio: f32,
    pub fm_decay: f32,
    /// Output bit depth (0 = off). 4 gives the gritty GB DAC sound.
    pub bits: u8,
    pub gain: f32,
    pub vel_sens: f32,
}

pub fn default_wavetable() -> Vec<u8> {
    (0..32u8).map(|i| if i < 16 { i } else { 31 - i }).collect()
}

impl Default for SynthParams {
    fn default() -> Self {
        SynthParams {
            wave: Wave::Pulse,
            duty: 0.5,
            wavetable: default_wavetable(),
            noise_short: false,
            unison: 1,
            detune: 10.0,
            octave: 0,
            fine: 0.0,
            attack: 0.0,
            decay: 0.3,
            sustain: 0.7,
            release: 0.08,
            filter_on: false,
            cutoff: 4000.0,
            resonance: 0.1,
            filter_env: 0.0,
            filter_decay: 0.3,
            vibrato_depth: 0.0,
            vibrato_rate: 5.5,
            vibrato_delay: 0.25,
            glide: 0.0,
            mono: false,
            sweep: 0.0,
            sweep_time: 0.05,
            arp: vec![],
            arp_speed: 30.0,
            arp_sync: false,
            lfo_rate: 0.5,
            lfo_sync: false,
            lfo_cutoff: 0.0,
            lfo_pwm: 0.0,
            lfo_amp: 0.0,
            fm_amount: 0.0,
            fm_ratio: 2.0,
            fm_decay: 0.0,
            bits: 0,
            gain: 0.35,
            vel_sens: 0.5,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SamplerParams {
    pub asset_id: Option<u64>,
    /// MIDI note at which the sample plays at its original pitch.
    pub root: u8,
    pub fine: f32,
    pub attack: f32,
    pub release: f32,
    pub looped: bool,
    pub gain: f32,
}

impl Default for SamplerParams {
    fn default() -> Self {
        SamplerParams { asset_id: None, root: 60, fine: 0.0, attack: 0.002, release: 0.15, looped: false, gain: 0.8 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct DrumParams {
    /// "chip" (LFSR noise, 4-bit), "clean" (acoustic-ish) or "electro" (80s drum machine).
    pub style: String,
    pub tune: f32,
    pub decay: f32,
    pub gain: f32,
}

impl Default for DrumParams {
    fn default() -> Self {
        DrumParams { style: "chip".into(), tune: 0.0, decay: 1.0, gain: 0.6 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: u64,
    pub start: u64,
    pub length: u64,
    /// Ticks into the pattern for pattern clips; seconds into the file for audio/media clips.
    #[serde(default)]
    pub offset: f64,
    #[serde(default = "one")]
    pub gain: f32,
    pub content: ClipContent,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClipContent {
    Pattern { pattern_id: u64 },
    Audio { asset_id: u64 },
    Media { asset_id: u64 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Pattern {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub color: String,
    pub length: u64,
    #[serde(default)]
    pub notes: Vec<Note>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    #[serde(default)]
    pub id: u64,
    pub pitch: u8,
    pub start: u64,
    pub length: u64,
    #[serde(default = "default_velocity")]
    pub velocity: f32,
}

fn default_velocity() -> f32 {
    0.8
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Audio,
    Video,
    Image,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AssetRef {
    #[serde(default)]
    pub id: u64,
    pub name: String,
    pub path: String,
    pub kind: AssetKind,
    /// Seconds (0 for images).
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub sample_rate: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Master {
    pub volume: f32,
    pub effects: Vec<Effect>,
    pub automation: Vec<AutomationLane>,
}

impl Default for Master {
    fn default() -> Self {
        Master { volume: 0.9, effects: vec![], automation: vec![] }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TempoPoint {
    pub tick: u64,
    pub bpm: f64,
    /// Glide linearly from the previous tempo to this one instead of jumping.
    #[serde(default)]
    pub ramp: bool,
}

/// Automation target names:
/// "volume" (0..1.5), "pan" (-1..1), "cutoff" (synth filter offset, octaves),
/// "pitch" (pitch bend, semitones), "fx:<effectId>:mix" (0..1 wet),
/// "fx:<effectId>:cutoff" (Filter effect cutoff in Hz).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutomationLane {
    pub id: u64,
    pub target: String,
    #[serde(default)]
    pub points: Vec<AutoPoint>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AutoPoint {
    pub tick: u64,
    pub value: f32,
}

/// Value of an automation curve at a tick (linear between points, flat outside).
pub fn auto_value_at(points: &[(f64, f32)], tick: f64) -> f32 {
    let i = points.partition_point(|p| p.0 <= tick);
    if i == 0 {
        return points[0].1;
    }
    if i >= points.len() {
        return points[points.len() - 1].1;
    }
    let (t0, v0) = points[i - 1];
    let (t1, v1) = points[i];
    if t1 <= t0 {
        return v1;
    }
    v0 + (v1 - v0) * ((tick - t0) / (t1 - t0)) as f32
}

/// Piecewise-constant tempo map (ramps are split into short constant steps).
#[derive(Clone, Debug)]
pub struct TempoMap {
    segs: Vec<TempoSeg>,
}

#[derive(Clone, Copy, Debug)]
struct TempoSeg {
    tick: f64,
    bpm: f64,
    secs: f64,
}

impl TempoMap {
    pub fn new(base: f64, points: &[TempoPoint]) -> Self {
        let mut pts = points.to_vec();
        pts.sort_by_key(|p| p.tick);
        let base = base.clamp(20.0, 400.0);
        let mut raw: Vec<(f64, f64)> = vec![(0.0, base)];
        let (mut prev_t, mut prev_b) = (0.0, base);
        for p in pts {
            let t = p.tick as f64;
            let b = p.bpm.clamp(20.0, 400.0);
            if p.ramp && t > prev_t {
                let step = (PPQ / 8) as f64;
                let n = ((t - prev_t) / step).ceil().max(1.0) as usize;
                for k in 0..n {
                    let frac = (k as f64 + 0.5) / n as f64;
                    raw.push((prev_t + (t - prev_t) * k as f64 / n as f64, prev_b + (b - prev_b) * frac));
                }
            }
            raw.push((t, b));
            prev_t = t;
            prev_b = b;
        }
        let mut segs: Vec<TempoSeg> = Vec::with_capacity(raw.len());
        for (t, b) in raw {
            match segs.last_mut() {
                Some(last) if (last.tick - t).abs() < 1e-9 => last.bpm = b,
                Some(last) => {
                    let secs = last.secs + (t - last.tick) * 60.0 / (last.bpm * PPQ as f64);
                    segs.push(TempoSeg { tick: t, bpm: b, secs });
                }
                None => segs.push(TempoSeg { tick: t, bpm: b, secs: 0.0 }),
            }
        }
        TempoMap { segs }
    }

    fn seg_at_tick(&self, tick: f64) -> &TempoSeg {
        &self.segs[self.segs.partition_point(|s| s.tick <= tick).saturating_sub(1)]
    }

    pub fn bpm_at(&self, tick: f64) -> f64 {
        self.seg_at_tick(tick).bpm
    }

    pub fn tick_to_secs(&self, tick: f64) -> f64 {
        let s = self.seg_at_tick(tick);
        s.secs + (tick - s.tick) * 60.0 / (s.bpm * PPQ as f64)
    }

    pub fn secs_to_tick(&self, secs: f64) -> f64 {
        let s = &self.segs[self.segs.partition_point(|s| s.secs <= secs).saturating_sub(1)];
        s.tick + (secs - s.secs) * s.bpm * PPQ as f64 / 60.0
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub id: u64,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub kind: EffectKind,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FilterMode {
    Lowpass,
    Highpass,
    Bandpass,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EffectKind {
    /// Gains in dB.
    Eq { low: f32, mid: f32, mid_freq: f32, high: f32 },
    Filter { mode: FilterMode, cutoff: f32, resonance: f32 },
    /// `time` in beats.
    Delay { time: f32, feedback: f32, mix: f32, ping_pong: bool },
    Reverb { size: f32, damping: f32, mix: f32, width: f32 },
    /// threshold/makeup in dB, attack/release in ms.
    Compressor { threshold: f32, ratio: f32, attack: f32, release: f32, makeup: f32 },
    /// depth in ms.
    Chorus { rate: f32, depth: f32, mix: f32 },
    Bitcrusher { bits: f32, downsample: f32, mix: f32 },
    /// Tempo-synced sidechain-style ducking. `rate` in beats, `release` as a fraction of the period.
    Pump { depth: f32, rate: f32, release: f32 },
}

impl EffectKind {
    pub fn default_for(name: &str) -> Option<EffectKind> {
        Some(match name {
            "eq" => EffectKind::Eq { low: 0.0, mid: 0.0, mid_freq: 1000.0, high: 0.0 },
            "filter" => EffectKind::Filter { mode: FilterMode::Lowpass, cutoff: 2000.0, resonance: 0.2 },
            "delay" => EffectKind::Delay { time: 0.75, feedback: 0.35, mix: 0.3, ping_pong: false },
            "reverb" => EffectKind::Reverb { size: 0.6, damping: 0.5, mix: 0.25, width: 1.0 },
            "compressor" => {
                EffectKind::Compressor { threshold: -18.0, ratio: 4.0, attack: 5.0, release: 120.0, makeup: 4.0 }
            }
            "chorus" => EffectKind::Chorus { rate: 0.8, depth: 3.0, mix: 0.4 },
            "bitcrusher" => EffectKind::Bitcrusher { bits: 6.0, downsample: 2.0, mix: 1.0 },
            "pump" => EffectKind::Pump { depth: 0.7, rate: 1.0, release: 0.6 },
            _ => return None,
        })
    }
}
