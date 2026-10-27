//! Turns the editable `Project` into a flat, play-ready `CompiledSong`.
//! Runs on the main thread; the result is handed to the audio thread.

use std::sync::Arc;

use super::effects::EffectUnit;
use crate::assets::{AssetStore, AudioData};
use crate::model::*;

#[derive(Clone, Copy)]
pub struct CNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub vel: f32,
}

pub struct CAudioClip {
    pub start: f64,
    /// Song time (seconds) at `start`, from the tempo map.
    pub start_secs: f64,
    pub end: f64,
    /// Seconds into the file.
    pub offset: f64,
    pub gain: f32,
    pub data: Arc<AudioData>,
}

pub enum CInstrument {
    Synth(SynthParams),
    Sampler { params: SamplerParams, data: Option<Arc<AudioData>> },
    Drums(DrumParams),
}

#[derive(Clone, Copy, PartialEq)]
pub enum CTarget {
    Volume,
    Pan,
    Cutoff,
    Pitch,
    FxMix(usize),
    FxCutoff(usize),
}

pub struct CLane {
    pub target: CTarget,
    /// (tick, value), sorted, never empty.
    pub points: Vec<(f64, f32)>,
}

impl CLane {
    pub fn value_at(&self, tick: f64) -> f32 {
        auto_value_at(&self.points, tick)
    }
}

fn compile_lanes(lanes: &[AutomationLane], effects: &[Effect]) -> Vec<CLane> {
    lanes
        .iter()
        .filter(|l| l.enabled && !l.points.is_empty())
        .filter_map(|l| {
            let target = match l.target.as_str() {
                "volume" => CTarget::Volume,
                "pan" => CTarget::Pan,
                "cutoff" => CTarget::Cutoff,
                "pitch" => CTarget::Pitch,
                t => {
                    let (id, param) = t.strip_prefix("fx:")?.split_once(':')?;
                    let id: u64 = id.parse().ok()?;
                    let idx = effects.iter().position(|e| e.id == id)?;
                    match param {
                        "mix" => CTarget::FxMix(idx),
                        "cutoff" if matches!(effects[idx].kind, EffectKind::Filter { .. }) => CTarget::FxCutoff(idx),
                        _ => return None,
                    }
                }
            };
            let mut points: Vec<(f64, f32)> = l.points.iter().map(|p| (p.tick as f64, p.value)).collect();
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            Some(CLane { target, points })
        })
        .collect()
}

pub struct CTrack {
    pub id: u64,
    pub auto: Vec<CLane>,
    pub instrument: Option<CInstrument>,
    /// Sorted by start.
    pub notes: Vec<CNote>,
    pub audio_clips: Vec<CAudioClip>,
    pub effects: Vec<EffectUnit>,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub monitor_input: bool,
}

pub struct CompiledSong {
    /// Tempo at the start (effects like delay sync to this).
    #[allow(dead_code)]
    pub bpm: f64,
    pub tempo: TempoMap,
    pub master_auto: Vec<CLane>,
    pub beat_ticks: f64,
    pub beats_per_bar: u32,
    pub loop_enabled: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub tracks: Vec<CTrack>,
    pub master_effects: Vec<EffectUnit>,
    pub master_volume: f32,
    pub any_solo: bool,
}

impl CompiledSong {
    pub fn empty() -> Self {
        CompiledSong {
            bpm: 120.0,
            tempo: TempoMap::new(120.0, &[]),
            master_auto: vec![],
            beat_ticks: PPQ as f64,
            beats_per_bar: 4,
            loop_enabled: false,
            loop_start: 0.0,
            loop_end: 0.0,
            tracks: vec![],
            master_effects: vec![],
            master_volume: 1.0,
            any_solo: false,
        }
    }
}

fn compile_instrument(i: &Instrument, assets: &AssetStore) -> CInstrument {
    match i {
        Instrument::Synth(p) => {
            let mut p = p.clone();
            if p.wavetable.len() != 32 {
                p.wavetable.resize(32, 8);
            }
            for v in p.wavetable.iter_mut() {
                *v = (*v).min(15);
            }
            CInstrument::Synth(p)
        }
        Instrument::Sampler(p) => CInstrument::Sampler {
            params: p.clone(),
            data: p.asset_id.and_then(|id| assets.get(id).cloned()),
        },
        Instrument::Drums(p) => CInstrument::Drums(p.clone()),
    }
}

/// Expands pattern clips (patterns repeat to fill their clip) into absolute notes.
fn flatten_notes(p: &Project, track: &Track) -> Vec<CNote> {
    let mut out = Vec::new();
    for clip in &track.clips {
        let ClipContent::Pattern { pattern_id } = clip.content else { continue };
        let Some(pat) = p.pattern(pattern_id) else { continue };
        if pat.length == 0 {
            continue;
        }
        let len = pat.length as f64;
        let cs = clip.start as f64;
        let ce = cs + clip.length as f64;
        let off = clip.offset.max(0.0);
        let mut k = (off / len).floor();
        loop {
            let base = cs - off + k * len;
            if base >= ce {
                break;
            }
            for n in &pat.notes {
                if n.start >= pat.length {
                    continue;
                }
                let s = base + n.start as f64;
                if s < cs || s >= ce {
                    continue;
                }
                let e = (s + n.length as f64).min(ce);
                out.push(CNote { start: s, end: e, pitch: n.pitch, vel: n.velocity });
            }
            k += 1.0;
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

pub fn compile(p: &Project, assets: &AssetStore, sr: f64) -> CompiledSong {
    let tempo = p.tempo_map();
    let mut tracks = Vec::with_capacity(p.tracks.len());
    for t in &p.tracks {
        let instrument = t.instrument().map(|i| compile_instrument(i, assets));
        let notes = if t.is_instrument() { flatten_notes(p, t) } else { vec![] };
        let mut audio_clips = vec![];
        if t.is_audio() {
            for c in &t.clips {
                if let ClipContent::Audio { asset_id } = c.content {
                    if let Some(data) = assets.get(asset_id) {
                        audio_clips.push(CAudioClip {
                            start: c.start as f64,
                            start_secs: tempo.tick_to_secs(c.start as f64),
                            end: (c.start + c.length) as f64,
                            offset: c.offset,
                            gain: c.gain,
                            data: data.clone(),
                        });
                    }
                }
            }
        }
        tracks.push(CTrack {
            id: t.id,
            auto: compile_lanes(&t.automation, &t.effects),
            instrument,
            notes,
            audio_clips,
            effects: t.effects.iter().map(|e| EffectUnit::new(e, sr, p.bpm)).collect(),
            volume: t.volume,
            pan: t.pan,
            mute: t.mute,
            solo: t.solo,
            monitor_input: t.is_audio() && t.armed && t.monitor,
        });
    }
    CompiledSong {
        bpm: p.bpm,
        master_auto: compile_lanes(&p.master.automation, &p.master.effects),
        tempo,
        beat_ticks: p.beat_ticks() as f64,
        beats_per_bar: p.time_sig_num.max(1),
        loop_enabled: p.loop_enabled,
        loop_start: p.loop_start as f64,
        loop_end: p.loop_end as f64,
        any_solo: p.tracks.iter().any(|t| t.solo),
        tracks,
        master_effects: p.master.effects.iter().map(|e| EffectUnit::new(e, sr, p.bpm)).collect(),
        master_volume: p.master.volume,
    }
}
