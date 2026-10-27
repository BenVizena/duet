//! Declarative song building for AI/scripted composition. A `SongSpec` describes
//! tracks, patterns (in note notation), arrangement, effects, automation and tempo;
//! `apply` writes it into the project (as one undo step when run through `apply_with`).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::edit::{self, Edit};
use crate::model::*;
use crate::notation;
use crate::presets;

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SongSpec {
    pub name: Option<String>,
    pub bpm: Option<f64>,
    /// "4/4", "3/4", "6/8"...
    pub time_sig: Option<String>,
    /// Root note name: "C", "F#", "Bb".
    pub key: Option<String>,
    /// major, minor, harmonicMinor, dorian, phrygian, lydian, mixolydian, majorPent, minorPent, blues, chromatic
    pub scale: Option<String>,
    #[serde(default)]
    pub tempo_changes: Vec<TempoSpec>,
    /// true (default): replace all non-video tracks and patterns. false: add to the project.
    pub replace: Option<bool>,
    /// [startBar, endBar] (1-based, end exclusive) to loop; omit to leave the loop alone.
    pub loop_bars: Option<[f64; 2]>,
    #[serde(default)]
    pub tracks: Vec<TrackSpec>,
    pub master: Option<MasterSpec>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TempoSpec {
    pub bar: f64,
    pub bpm: f64,
    #[serde(default)]
    pub ramp: bool,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackSpec {
    pub name: String,
    /// Preset name (see list_presets). Defaults to "Pulse Lead 12.5%".
    pub preset: Option<String>,
    /// Partial instrument params merged over the preset (or a full instrument with "type").
    pub instrument: Option<Value>,
    pub volume: Option<f32>,
    pub pan: Option<f32>,
    pub mute: Option<bool>,
    /// e.g. [{"type":"reverb","mix":0.3}] — params merged over that effect's defaults.
    #[serde(default)]
    pub effects: Vec<Value>,
    /// Pattern name -> pattern. Names are local to this track.
    #[serde(default)]
    pub patterns: BTreeMap<String, PatternSpec>,
    /// Where patterns play.
    #[serde(default)]
    pub clips: Vec<ClipSpec>,
    #[serde(default)]
    pub automation: Vec<AutoSpec>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatternSpec {
    /// Length in bars; defaults to the notes rounded up to whole bars.
    pub bars: Option<f64>,
    pub notes: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipSpec {
    pub pattern: String,
    /// 1-based bar (fractions allowed).
    pub bar: f64,
    /// Clip length in bars (the pattern loops to fill it). Defaults to the pattern length.
    pub bars: Option<f64>,
    /// Place this many back-to-back copies.
    pub repeat: Option<u32>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoSpec {
    /// volume | pan | cutoff | pitch | fx:<index>:mix | fx:<index>:cutoff (index into this track's effects list)
    pub target: String,
    /// [[bar, value], ...]
    pub points: Vec<[f64; 2]>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MasterSpec {
    pub volume: Option<f32>,
    pub effects: Option<Vec<Value>>,
    #[serde(default)]
    pub automation: Vec<AutoSpec>,
}

fn merge(base: &mut Value, over: &Value) {
    if let (Value::Object(b), Value::Object(o)) = (base, over) {
        for (k, v) in o {
            b.insert(k.clone(), v.clone());
        }
    }
}

pub fn build_effect(spec: &Value) -> Result<EffectKind, String> {
    let ty = spec.get("type").and_then(|t| t.as_str()).ok_or("each effect needs a \"type\"")?;
    let def = EffectKind::default_for(ty).ok_or_else(|| {
        format!("unknown effect '{ty}' (eq, filter, delay, reverb, compressor, chorus, bitcrusher, pump)")
    })?;
    let mut v = serde_json::to_value(def).map_err(|e| e.to_string())?;
    merge(&mut v, spec);
    serde_json::from_value(v).map_err(|e| format!("effect '{ty}': {e}"))
}

pub fn build_instrument(preset: Option<&str>, over: Option<&Value>) -> Result<Instrument, String> {
    let has_type = over.and_then(|o| o.get("type")).is_some();
    let base = match preset {
        Some(name) => presets::preset_any(name).ok_or_else(|| format!("unknown preset '{name}' (see list_presets)"))?,
        None if has_type => return serde_json::from_value(over.unwrap().clone()).map_err(|e| format!("instrument: {e}")),
        None => presets::default_instrument(),
    };
    let Some(o) = over else { return Ok(base) };
    let mut v = serde_json::to_value(&base).map_err(|e| e.to_string())?;
    merge(&mut v, o);
    serde_json::from_value(v).map_err(|e| format!("instrument: {e}"))
}

fn bar_tick(p: &Project, bar: f64) -> u64 {
    ((bar - 1.0).max(0.0) * p.bar_ticks() as f64).round() as u64
}

fn bars_ticks(p: &Project, bars: f64) -> u64 {
    ((bars * p.bar_ticks() as f64).round() as u64).max(1)
}

fn auto_points(p: &Project, pts: &[[f64; 2]]) -> Vec<AutoPoint> {
    pts.iter().map(|[b, v]| AutoPoint { tick: bar_tick(p, *b), value: *v as f32 }).collect()
}

/// Turns an AutoSpec target (effects by index) into a model target (effects by id).
fn resolve_target(target: &str, effect_ids: &[u64]) -> Result<String, String> {
    if let Some(rest) = target.strip_prefix("fx:") {
        let (idx, param) = rest.split_once(':').ok_or_else(|| format!("bad target '{target}'"))?;
        let i: usize = idx.parse().map_err(|_| format!("bad effect index in '{target}'"))?;
        let id = effect_ids.get(i).ok_or_else(|| format!("'{target}': there is no effect #{i} on this track"))?;
        return Ok(format!("fx:{id}:{param}"));
    }
    Ok(target.to_string())
}

pub struct ComposeResult {
    pub tracks: Vec<(String, u64)>,
    pub warnings: Vec<String>,
}

pub fn apply(p: &mut Project, spec: SongSpec) -> Result<ComposeResult, String> {
    let mut warnings = vec![];
    if spec.replace.unwrap_or(true) {
        p.tracks.retain(|t| t.is_media());
        p.patterns.clear();
        p.tempo_changes.clear();
    }
    if let Some(n) = spec.name {
        p.name = n;
    }
    if let Some(b) = spec.bpm {
        p.bpm = b.clamp(20.0, 400.0);
    }
    if let Some(ts) = &spec.time_sig {
        let (n, d) = ts.split_once('/').ok_or("timeSig must look like \"4/4\"")?;
        edit::apply(
            p,
            Edit::SetTimeSig {
                num: n.trim().parse().map_err(|_| "bad timeSig")?,
                den: d.trim().parse().map_err(|_| "bad timeSig")?,
            },
        )?;
    }
    if spec.key.is_some() || spec.scale.is_some() {
        let root = match &spec.key {
            Some(k) => notation::pitch_of(&format!("{k}4")).ok_or_else(|| format!("unknown key '{k}'"))? % 12,
            None => p.key_root,
        };
        p.key_root = root;
        if let Some(s) = spec.scale {
            p.scale = s;
        }
    }
    if !spec.tempo_changes.is_empty() {
        let pts = spec
            .tempo_changes
            .iter()
            .map(|t| TempoPoint { tick: bar_tick(p, t.bar), bpm: t.bpm, ramp: t.ramp })
            .collect();
        edit::apply(p, Edit::SetTempo { points: pts })?;
    }

    let mut made = vec![];
    for ts in spec.tracks {
        let inst = build_instrument(ts.preset.as_deref(), ts.instrument.as_ref()).map_err(|e| format!("track '{}': {e}", ts.name))?;
        let tid = edit::apply(
            p,
            Edit::AddTrack { kind: "instrument".into(), name: Some(ts.name.clone()), instrument: Some(inst), index: None },
        )?
        .unwrap();
        let mut effect_ids = vec![];
        for e in &ts.effects {
            let kind = build_effect(e).map_err(|err| format!("track '{}': {err}", ts.name))?;
            let id = p.alloc_id();
            p.track_mut(tid).unwrap().effects.push(Effect { id, enabled: true, kind });
            effect_ids.push(id);
        }
        {
            let t = p.track_mut(tid).unwrap();
            if let Some(v) = ts.volume {
                t.volume = v.clamp(0.0, 2.0);
            }
            if let Some(v) = ts.pan {
                t.pan = v.clamp(-1.0, 1.0);
            }
            if let Some(v) = ts.mute {
                t.mute = v;
            }
        }
        let mut pats: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for (pname, ps) in &ts.patterns {
            let notes = notation::parse(&ps.notes).map_err(|e| format!("track '{}', pattern '{pname}': {e}", ts.name))?;
            let len = match ps.bars {
                Some(b) => bars_ticks(p, b),
                None => {
                    let bt = p.bar_ticks();
                    notation::end_tick(&notes).div_ceil(bt).max(1) * bt
                }
            };
            if notation::end_tick(&notes) > len {
                warnings.push(format!("track '{}', pattern '{pname}': notes run past its {} bars", ts.name, len as f64 / p.bar_ticks() as f64));
            }
            let pid = edit::apply(
                p,
                Edit::AddPattern { name: Some(format!("{} {pname}", ts.name)), length: Some(len), notes: Some(notes), text: None },
            )?
            .unwrap();
            pats.insert(pname.clone(), (pid, len));
        }
        for c in &ts.clips {
            let (pid, plen) = *pats
                .get(&c.pattern)
                .ok_or_else(|| format!("track '{}': clip uses unknown pattern '{}'", ts.name, c.pattern))?;
            let len = c.bars.map(|b| bars_ticks(p, b)).unwrap_or(plen);
            let start = bar_tick(p, c.bar);
            for k in 0..c.repeat.unwrap_or(1).max(1) as u64 {
                edit::apply(
                    p,
                    Edit::AddClip {
                        track_id: tid,
                        start: start + k * len,
                        length: len,
                        offset: None,
                        content: ClipContent::Pattern { pattern_id: pid },
                    },
                )?;
            }
        }
        if ts.clips.is_empty() && !pats.is_empty() {
            warnings.push(format!("track '{}' has patterns but no clips, so it won't play", ts.name));
        }
        for a in &ts.automation {
            let target = resolve_target(&a.target, &effect_ids).map_err(|e| format!("track '{}': {e}", ts.name))?;
            let points = auto_points(p, &a.points);
            edit::apply(p, Edit::AddAutomationLane { track_id: Some(tid), target, points: Some(points) })
                .map_err(|e| format!("track '{}': {e}", ts.name))?;
        }
        made.push((ts.name, tid));
    }

    if let Some(m) = spec.master {
        if let Some(v) = m.volume {
            p.master.volume = v.clamp(0.0, 2.0);
        }
        if let Some(fx) = m.effects {
            p.master.effects.clear();
            p.master.automation.clear();
            for e in &fx {
                let kind = build_effect(e).map_err(|err| format!("master: {err}"))?;
                let id = p.alloc_id();
                p.master.effects.push(Effect { id, enabled: true, kind });
            }
        }
        let ids: Vec<u64> = p.master.effects.iter().map(|e| e.id).collect();
        for a in &m.automation {
            let target = resolve_target(&a.target, &ids).map_err(|e| format!("master: {e}"))?;
            let points = auto_points(p, &a.points);
            edit::apply(p, Edit::AddAutomationLane { track_id: None, target, points: Some(points) })?;
        }
    }

    if let Some([a, b]) = spec.loop_bars {
        p.loop_start = bar_tick(p, a);
        p.loop_end = bar_tick(p, b).max(p.loop_start + 1);
        p.loop_enabled = true;
    }
    Ok(ComposeResult { tracks: made, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_a_small_song() {
        let spec: SongSpec = serde_json::from_value(serde_json::json!({
            "bpm": 110, "key": "A", "scale": "minor",
            "tempoChanges": [{"bar": 5, "bpm": 90, "ramp": true}],
            "tracks": [
                {"name": "Lead", "preset": "Supersaw Lead", "instrument": {"cutoff": 3000},
                 "effects": [{"type": "delay", "mix": 0.2}, {"type": "filter"}],
                 "patterns": {"A": {"notes": "A4:4 C5:4 E5:8"}},
                 "clips": [{"pattern": "A", "bar": 1, "repeat": 4}],
                 "automation": [{"target": "fx:1:cutoff", "points": [[1, 300], [5, 8000]]}, {"target": "volume", "points": [[1, 0.2], [2, 0.8]]}]},
                {"name": "Drums", "preset": "Electro Drum Machine",
                 "patterns": {"beat": {"bars": 1, "notes": "kick: x...x...x...x...\nsnare: ....x.......x..."}},
                 "clips": [{"pattern": "beat", "bar": 1, "bars": 4}]}
            ],
            "master": {"effects": [{"type": "reverb", "mix": 0.1}], "automation": [{"target": "volume", "points": [[4, 0.9], [5, 0]]}]},
            "loopBars": [1, 5]
        }))
        .unwrap();
        let mut p = presets::template("empty");
        let r = apply(&mut p, spec).unwrap();
        assert_eq!(r.tracks.len(), 2);
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[0].clips.len(), 4);
        assert_eq!(p.tracks[0].automation.len(), 2);
        assert_eq!(p.tracks[1].clips[0].length, 4 * 384);
        assert_eq!(p.patterns.iter().find(|x| x.name == "Drums beat").unwrap().notes.len(), 6);
        assert_eq!(p.master.automation.len(), 1);
        assert_eq!(p.tempo_changes.len(), 1);
        let (l, r) = crate::export::render_mix(&p, &crate::assets::AssetStore::default(), 44100, |_| {}).unwrap();
        // 4 bars at 110 bpm then ramping down: longer than 4 bars at 110.
        assert!(l.len() as f64 / 44100.0 > 4.0 * 4.0 * 60.0 / 110.0);
        assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()));
    }

    #[test]
    fn reports_helpful_errors() {
        let bad: SongSpec = serde_json::from_value(serde_json::json!({
            "tracks": [{"name": "X", "patterns": {"A": {"notes": "Q9:4"}}, "clips": [{"pattern": "A", "bar": 1}]}]
        }))
        .unwrap();
        let err = apply(&mut Project::default(), bad).err().unwrap();
        assert!(err.contains("track 'X', pattern 'A'"), "{err}");
    }
}
