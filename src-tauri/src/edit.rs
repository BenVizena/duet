//! The command layer: every change to the project goes through an `Edit`.
//! Undo/redo (see `history`) snapshots the project around each applied edit.

use crate::model::*;
use crate::presets;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Edit {
    /// Several edits applied as one undo step.
    Batch { edits: Vec<Edit> },
    SetBpm { bpm: f64 },
    SetTimeSig { num: u32, den: u32 },
    SetKeyScale { root: u8, scale: String },
    SetLoop { start: u64, end: u64, enabled: bool },
    RenameProject { name: String },
    AddTrack {
        /// "instrument" | "audio" | "media"
        kind: String,
        name: Option<String>,
        instrument: Option<Instrument>,
        index: Option<usize>,
    },
    RemoveTrack { track_id: u64 },
    MoveTrack { track_id: u64, index: usize },
    UpdateTrack {
        track_id: u64,
        name: Option<String>,
        color: Option<String>,
        volume: Option<f32>,
        pan: Option<f32>,
        mute: Option<bool>,
        solo: Option<bool>,
        armed: Option<bool>,
        monitor: Option<bool>,
    },
    SetInstrument { track_id: u64, instrument: Instrument },
    AddClip { track_id: u64, start: u64, length: u64, offset: Option<f64>, content: ClipContent },
    UpdateClip {
        track_id: u64,
        clip_id: u64,
        start: Option<u64>,
        length: Option<u64>,
        offset: Option<f64>,
        gain: Option<f32>,
        to_track_id: Option<u64>,
    },
    RemoveClip { track_id: u64, clip_id: u64 },
    /// `text` is note notation (see `notation`); used instead of `notes` when given.
    AddPattern { name: Option<String>, length: Option<u64>, notes: Option<Vec<Note>>, text: Option<String> },
    UpdatePattern { pattern_id: u64, name: Option<String>, length: Option<u64>, color: Option<String> },
    RemovePattern { pattern_id: u64 },
    ClonePattern { pattern_id: u64 },
    SetNotes { pattern_id: u64, notes: Vec<Note> },
    SetNotesText { pattern_id: u64, text: String },
    SetTempo { points: Vec<TempoPoint> },
    /// `track_id: None` = master bus.
    AddAutomationLane { track_id: Option<u64>, target: String, points: Option<Vec<AutoPoint>> },
    SetAutomation { track_id: Option<u64>, lane_id: u64, points: Option<Vec<AutoPoint>>, enabled: Option<bool> },
    RemoveAutomationLane { track_id: Option<u64>, lane_id: u64 },
    AddEffect { track_id: Option<u64>, effect: String },
    UpdateEffect { track_id: Option<u64>, effect_id: u64, kind: Option<EffectKind>, enabled: Option<bool> },
    RemoveEffect { track_id: Option<u64>, effect_id: u64 },
    MoveEffect { track_id: Option<u64>, effect_id: u64, index: usize },
    SetMasterVolume { volume: f32 },
    AddAsset { asset: AssetRef },
}

pub const TRACK_COLORS: [&str; 10] = [
    "#e8a33d", "#4fb3d9", "#8bc34a", "#e57373", "#ba68c8", "#4db6ac", "#ffb74d", "#7986cb", "#f06292", "#aed581",
];

/// Edits with the same key in quick succession merge into one undo step
/// (e.g. dragging a volume fader).
pub fn merge_key(e: &Edit) -> Option<String> {
    match e {
        Edit::SetBpm { .. } => Some("bpm".into()),
        Edit::SetLoop { .. } => Some("loop".into()),
        Edit::UpdateTrack { track_id, volume, pan, .. } if volume.is_some() || pan.is_some() => {
            Some(format!("mix:{track_id}"))
        }
        Edit::SetInstrument { track_id, .. } => Some(format!("inst:{track_id}")),
        Edit::UpdateEffect { effect_id, kind: Some(_), .. } => Some(format!("fx:{effect_id}")),
        Edit::SetMasterVolume { .. } => Some("master".into()),
        _ => None,
    }
}

fn effects_mut<'a>(p: &'a mut Project, track_id: Option<u64>) -> Result<&'a mut Vec<Effect>, String> {
    match track_id {
        None => Ok(&mut p.master.effects),
        Some(id) => p.track_mut(id).map(|t| &mut t.effects).ok_or_else(|| "Track not found".to_string()),
    }
}

fn lanes_mut<'a>(p: &'a mut Project, track_id: Option<u64>) -> Result<&'a mut Vec<AutomationLane>, String> {
    match track_id {
        None => Ok(&mut p.master.automation),
        Some(id) => p.track_mut(id).map(|t| &mut t.automation).ok_or_else(|| "Track not found".to_string()),
    }
}

/// Checks an automation target name against what the track actually has.
pub fn validate_target(p: &Project, track_id: Option<u64>, target: &str) -> Result<(), String> {
    let effects = match track_id {
        None => &p.master.effects,
        Some(id) => &p.track(id).ok_or("Track not found")?.effects,
    };
    let ok = match target {
        "volume" => true,
        "pan" | "cutoff" | "pitch" => track_id.is_some(),
        t => match t.strip_prefix("fx:").and_then(|r| r.split_once(':')) {
            Some((id, param)) => {
                let fx = id.parse::<u64>().ok().and_then(|id| effects.iter().find(|e| e.id == id));
                match (fx, param) {
                    (Some(_), "mix") => true,
                    (Some(e), "cutoff") => matches!(e.kind, EffectKind::Filter { .. }),
                    _ => false,
                }
            }
            None => false,
        },
    };
    if ok {
        Ok(())
    } else {
        Err(format!("Can't automate '{target}' here. Use volume, pan, cutoff, pitch, fx:<effectId>:mix or fx:<filterEffectId>:cutoff."))
    }
}

fn content_fits(track: &Track, content: &ClipContent) -> bool {
    match content {
        ClipContent::Pattern { .. } => track.is_instrument(),
        ClipContent::Audio { .. } => track.is_audio(),
        ClipContent::Media { .. } => track.is_media(),
    }
}

/// Applies an edit. Returns the id of a newly created object, if any.
pub fn apply(p: &mut Project, e: Edit) -> Result<Option<u64>, String> {
    match e {
        Edit::Batch { edits } => {
            let mut last = None;
            for e in edits {
                if let Some(id) = apply(p, e)? {
                    last = Some(id);
                }
            }
            Ok(last)
        }
        Edit::SetBpm { bpm } => {
            p.bpm = bpm.clamp(20.0, 400.0);
            Ok(None)
        }
        Edit::SetTimeSig { num, den } => {
            if !matches!(den, 2 | 4 | 8 | 16) {
                return Err("Time signature denominator must be 2, 4, 8 or 16".into());
            }
            p.time_sig_num = num.clamp(1, 32);
            p.time_sig_den = den;
            Ok(None)
        }
        Edit::SetKeyScale { root, scale } => {
            p.key_root = root % 12;
            p.scale = scale;
            Ok(None)
        }
        Edit::SetLoop { start, end, enabled } => {
            p.loop_start = start.min(end);
            p.loop_end = end.max(start);
            p.loop_enabled = enabled && p.loop_end > p.loop_start;
            Ok(None)
        }
        Edit::RenameProject { name } => {
            p.name = name;
            Ok(None)
        }
        Edit::AddTrack { kind, name, instrument, index } => {
            let id = p.alloc_id();
            let count = p.tracks.len();
            let (kind, default_name) = match kind.as_str() {
                "instrument" => (
                    TrackKind::Instrument { instrument: instrument.unwrap_or_else(presets::default_instrument) },
                    format!("Instrument {}", count + 1),
                ),
                "audio" => (TrackKind::Audio, format!("Audio {}", count + 1)),
                "media" => (TrackKind::Media, "Video / Images".to_string()),
                other => return Err(format!("Unknown track kind '{other}'")),
            };
            let track = Track {
                id,
                name: name.unwrap_or(default_name),
                color: TRACK_COLORS[count % TRACK_COLORS.len()].into(),
                kind,
                volume: 0.8,
                pan: 0.0,
                mute: false,
                solo: false,
                armed: false,
                monitor: false,
                effects: vec![],
                clips: vec![],
                automation: vec![],
            };
            let index = index.unwrap_or(p.tracks.len()).min(p.tracks.len());
            p.tracks.insert(index, track);
            Ok(Some(id))
        }
        Edit::RemoveTrack { track_id } => {
            let before = p.tracks.len();
            p.tracks.retain(|t| t.id != track_id);
            if p.tracks.len() == before {
                return Err("Track not found".into());
            }
            Ok(None)
        }
        Edit::MoveTrack { track_id, index } => {
            let from = p.tracks.iter().position(|t| t.id == track_id).ok_or("Track not found")?;
            let t = p.tracks.remove(from);
            let index = index.min(p.tracks.len());
            p.tracks.insert(index, t);
            Ok(None)
        }
        Edit::UpdateTrack { track_id, name, color, volume, pan, mute, solo, armed, monitor } => {
            let t = p.track_mut(track_id).ok_or("Track not found")?;
            if let Some(v) = name {
                t.name = v;
            }
            if let Some(v) = color {
                t.color = v;
            }
            if let Some(v) = volume {
                t.volume = v.clamp(0.0, 2.0);
            }
            if let Some(v) = pan {
                t.pan = v.clamp(-1.0, 1.0);
            }
            if let Some(v) = mute {
                t.mute = v;
            }
            if let Some(v) = solo {
                t.solo = v;
            }
            if let Some(v) = armed {
                t.armed = v && !t.is_media();
            }
            if let Some(v) = monitor {
                t.monitor = v && t.is_audio();
            }
            Ok(None)
        }
        Edit::SetInstrument { track_id, instrument } => {
            let t = p.track_mut(track_id).ok_or("Track not found")?;
            match &mut t.kind {
                TrackKind::Instrument { instrument: i } => *i = instrument,
                _ => return Err("Not an instrument track".into()),
            }
            Ok(None)
        }
        Edit::AddClip { track_id, start, length, offset, content } => {
            let id = p.alloc_id();
            let t = p.track_mut(track_id).ok_or("Track not found")?;
            if !content_fits(t, &content) {
                return Err("That kind of clip can't go on this track".into());
            }
            t.clips.push(Clip { id, start, length: length.max(1), offset: offset.unwrap_or(0.0), gain: 1.0, content });
            Ok(Some(id))
        }
        Edit::UpdateClip { track_id, clip_id, start, length, offset, gain, to_track_id } => {
            let t = p.track_mut(track_id).ok_or("Track not found")?;
            let idx = t.clips.iter().position(|c| c.id == clip_id).ok_or("Clip not found")?;
            {
                let c = &mut t.clips[idx];
                if let Some(v) = start {
                    c.start = v;
                }
                if let Some(v) = length {
                    c.length = v.max(1);
                }
                if let Some(v) = offset {
                    c.offset = v.max(0.0);
                }
                if let Some(v) = gain {
                    c.gain = v.clamp(0.0, 4.0);
                }
            }
            if let Some(dest) = to_track_id {
                if dest != track_id {
                    let clip = t.clips[idx].clone();
                    let d = p.track(dest).ok_or("Destination track not found")?;
                    if !content_fits(d, &clip.content) {
                        return Err("That kind of clip can't go on this track".into());
                    }
                    p.track_mut(track_id).unwrap().clips.remove(idx);
                    p.track_mut(dest).unwrap().clips.push(clip);
                }
            }
            Ok(None)
        }
        Edit::RemoveClip { track_id, clip_id } => {
            let t = p.track_mut(track_id).ok_or("Track not found")?;
            t.clips.retain(|c| c.id != clip_id);
            Ok(None)
        }
        Edit::AddPattern { name, length, notes, text } => {
            let notes = match text {
                Some(t) => Some(crate::notation::parse(&t)?),
                None => notes,
            };
            let id = p.alloc_id();
            let n = p.patterns.len();
            let bar = p.bar_ticks();
            let mut pat = Pattern {
                id,
                name: name.unwrap_or(format!("Pattern {}", n + 1)),
                color: TRACK_COLORS[(n + 3) % TRACK_COLORS.len()].into(),
                length: length.unwrap_or(bar).max(1),
                notes: vec![],
            };
            if let Some(notes) = notes {
                pat.notes = normalize_notes(p, notes);
            }
            p.patterns.push(pat);
            Ok(Some(id))
        }
        Edit::UpdatePattern { pattern_id, name, length, color } => {
            let pat = p.pattern_mut(pattern_id).ok_or("Pattern not found")?;
            if let Some(v) = name {
                pat.name = v;
            }
            if let Some(v) = length {
                pat.length = v.max(1);
            }
            if let Some(v) = color {
                pat.color = v;
            }
            Ok(None)
        }
        Edit::RemovePattern { pattern_id } => {
            p.patterns.retain(|x| x.id != pattern_id);
            for t in &mut p.tracks {
                t.clips.retain(|c| !matches!(c.content, ClipContent::Pattern { pattern_id: pid } if pid == pattern_id));
            }
            Ok(None)
        }
        Edit::ClonePattern { pattern_id } => {
            let mut pat = p.pattern(pattern_id).ok_or("Pattern not found")?.clone();
            let id = p.alloc_id();
            pat.id = id;
            pat.name = format!("{} copy", pat.name);
            for n in &mut pat.notes {
                n.id = 0;
            }
            pat.notes = normalize_notes(p, pat.notes);
            p.patterns.push(pat);
            Ok(Some(id))
        }
        Edit::SetNotes { pattern_id, notes } => {
            let notes = normalize_notes(p, notes);
            let pat = p.pattern_mut(pattern_id).ok_or("Pattern not found")?;
            pat.notes = notes;
            Ok(None)
        }
        Edit::SetNotesText { pattern_id, text } => {
            let notes = normalize_notes(p, crate::notation::parse(&text)?);
            let pat = p.pattern_mut(pattern_id).ok_or("Pattern not found")?;
            pat.notes = notes;
            Ok(None)
        }
        Edit::SetTempo { mut points } => {
            points.retain(|x| x.tick > 0);
            points.sort_by_key(|x| x.tick);
            points.dedup_by_key(|x| x.tick);
            for x in points.iter_mut() {
                x.bpm = x.bpm.clamp(20.0, 400.0);
            }
            p.tempo_changes = points;
            Ok(None)
        }
        Edit::AddAutomationLane { track_id, target, points } => {
            validate_target(p, track_id, &target)?;
            let id = p.alloc_id();
            let mut points = points.unwrap_or_default();
            points.sort_by_key(|x| x.tick);
            lanes_mut(p, track_id)?.push(AutomationLane { id, target, points, enabled: true });
            Ok(Some(id))
        }
        Edit::SetAutomation { track_id, lane_id, points, enabled } => {
            let lane = lanes_mut(p, track_id)?.iter_mut().find(|l| l.id == lane_id).ok_or("Automation lane not found")?;
            if let Some(mut pts) = points {
                pts.sort_by_key(|x| x.tick);
                lane.points = pts;
            }
            if let Some(e) = enabled {
                lane.enabled = e;
            }
            Ok(None)
        }
        Edit::RemoveAutomationLane { track_id, lane_id } => {
            lanes_mut(p, track_id)?.retain(|l| l.id != lane_id);
            Ok(None)
        }
        Edit::AddEffect { track_id, effect } => {
            let kind = EffectKind::default_for(&effect).ok_or_else(|| format!("Unknown effect '{effect}'"))?;
            let id = p.alloc_id();
            effects_mut(p, track_id)?.push(Effect { id, enabled: true, kind });
            Ok(Some(id))
        }
        Edit::UpdateEffect { track_id, effect_id, kind, enabled } => {
            let fx = effects_mut(p, track_id)?;
            let e = fx.iter_mut().find(|e| e.id == effect_id).ok_or("Effect not found")?;
            if let Some(k) = kind {
                if std::mem::discriminant(&k) != std::mem::discriminant(&e.kind) {
                    return Err("Effect type can't change".into());
                }
                e.kind = k;
            }
            if let Some(v) = enabled {
                e.enabled = v;
            }
            Ok(None)
        }
        Edit::RemoveEffect { track_id, effect_id } => {
            effects_mut(p, track_id)?.retain(|e| e.id != effect_id);
            Ok(None)
        }
        Edit::MoveEffect { track_id, effect_id, index } => {
            let fx = effects_mut(p, track_id)?;
            let from = fx.iter().position(|e| e.id == effect_id).ok_or("Effect not found")?;
            let e = fx.remove(from);
            let index = index.min(fx.len());
            fx.insert(index, e);
            Ok(None)
        }
        Edit::SetMasterVolume { volume } => {
            p.master.volume = volume.clamp(0.0, 2.0);
            Ok(None)
        }
        Edit::AddAsset { mut asset } => {
            if asset.id == 0 {
                asset.id = p.alloc_id();
            }
            let id = asset.id;
            p.assets.retain(|a| a.id != id);
            p.assets.push(asset);
            Ok(Some(id))
        }
    }
}

fn normalize_notes(p: &mut Project, notes: Vec<Note>) -> Vec<Note> {
    let mut out: Vec<Note> = notes
        .into_iter()
        .map(|mut n| {
            if n.id == 0 {
                n.id = p.alloc_id();
            }
            n.pitch = n.pitch.min(127);
            n.length = n.length.max(1);
            n.velocity = n.velocity.clamp(0.0, 1.0);
            n
        })
        .collect();
    out.sort_by_key(|n| (n.start, n.pitch));
    out
}
