use crate::assets::AssetStore;
use crate::edit::{apply, Edit};
use crate::export::render_mix;
use crate::model::*;
use crate::presets;

fn stats(l: &[f32], r: &[f32]) -> (f32, f32) {
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    for i in 0..l.len() {
        assert!(l[i].is_finite() && r[i].is_finite(), "non-finite sample at {i}");
        peak = peak.max(l[i].abs()).max(r[i].abs());
        sum += (l[i] as f64).powi(2);
    }
    (peak, (sum / l.len().max(1) as f64).sqrt() as f32)
}

#[test]
fn templates_render_audibly() {
    let assets = AssetStore::default();
    for t in presets::templates() {
        let p = presets::template(&t.id);
        let (l, r) = render_mix(&p, &assets, 48000, |_| {}).unwrap_or_else(|e| panic!("{}: {e}", t.id));
        let secs = p.tempo_map().tick_to_secs(p.audio_end_tick() as f64);
        assert!(l.len() as f64 >= secs * 48000.0 * 0.99, "{}: too short", t.id);
        let (peak, rms) = stats(&l, &r);
        println!("{}: {:.1}s peak {peak:.3} rms {rms:.3}", t.id, l.len() as f64 / 48000.0);
        assert!(peak <= 1.0, "{}: clipped ({peak})", t.id);
        if t.id != "empty" {
            assert!(rms > 0.02, "{}: too quiet (rms {rms})", t.id);
        }
    }
}

#[test]
fn every_preset_makes_sound() {
    let assets = AssetStore::default();
    for preset in presets::presets() {
        if matches!(preset.instrument, Instrument::Sampler(_)) {
            continue;
        }
        let mut p = Project::default();
        let tid = apply(
            &mut p,
            Edit::AddTrack { kind: "instrument".into(), name: None, instrument: Some(preset.instrument.clone()), index: None },
        )
        .unwrap()
        .unwrap();
        let notes = vec![
            Note { id: 0, pitch: 36, start: 0, length: 96, velocity: 0.9 },
            Note { id: 0, pitch: 60, start: 96, length: 96, velocity: 0.9 },
            Note { id: 0, pitch: 67, start: 192, length: 96, velocity: 0.9 },
        ];
        let pid = apply(&mut p, Edit::AddPattern { name: None, length: Some(384), notes: Some(notes), text: None }).unwrap().unwrap();
        apply(
            &mut p,
            Edit::AddClip { track_id: tid, start: 0, length: 384, offset: None, content: ClipContent::Pattern { pattern_id: pid } },
        )
        .unwrap();
        let (l, r) = render_mix(&p, &assets, 44100, |_| {}).unwrap();
        let (peak, rms) = stats(&l, &r);
        assert!(rms > 0.003 && peak > 0.02, "preset '{}' is silent (rms {rms}, peak {peak})", preset.name);
    }
}

#[test]
fn every_effect_is_stable() {
    let assets = AssetStore::default();
    for name in ["eq", "filter", "delay", "reverb", "compressor", "chorus", "bitcrusher", "pump"] {
        let mut p = presets::template("chip-route");
        let tid = p.tracks[0].id;
        apply(&mut p, Edit::AddEffect { track_id: Some(tid), effect: name.into() }).unwrap();
        apply(&mut p, Edit::AddEffect { track_id: None, effect: name.into() }).unwrap();
        let (l, r) = render_mix(&p, &assets, 48000, |_| {}).unwrap();
        let (peak, rms) = stats(&l, &r);
        assert!(rms > 0.01 && peak <= 1.0, "{name}: rms {rms} peak {peak}");
    }
}

#[test]
fn project_roundtrips_through_json() {
    for t in presets::templates() {
        let p = presets::template(&t.id);
        let s = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&s).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), s);
    }
}

#[test]
fn edits_from_ui_json_parse() {
    let mut p = presets::template("empty");
    let tid = p.tracks[0].id;
    let e: Edit = serde_json::from_str(&format!(r#"{{"type":"updateTrack","trackId":{tid},"volume":0.5,"mute":true}}"#)).unwrap();
    apply(&mut p, e).unwrap();
    assert_eq!(p.tracks[0].volume, 0.5);
    assert!(p.tracks[0].mute);
    let e: Edit = serde_json::from_str(r#"{"type":"addTrack","kind":"audio"}"#).unwrap();
    assert!(apply(&mut p, e).unwrap().is_some());
    let pid = p.patterns[0].id;
    let e: Edit = serde_json::from_str(&format!(
        r#"{{"type":"setNotes","patternId":{pid},"notes":[{{"pitch":60,"start":0,"length":24,"velocity":0.7}}]}}"#
    ))
    .unwrap();
    apply(&mut p, e).unwrap();
    assert_eq!(p.patterns[0].notes.len(), 1);
    assert!(p.patterns[0].notes[0].id > 0);
}

#[test]
fn undo_merges_rapid_fader_moves() {
    let mut h = crate::history::History::default();
    let mut p = presets::template("empty");
    let tid = p.tracks[0].id;
    for v in [0.1f32, 0.2, 0.3] {
        let before = p.clone();
        let e = Edit::UpdateTrack {
            track_id: tid,
            name: None,
            color: None,
            volume: Some(v),
            pan: None,
            mute: None,
            solo: None,
            armed: None,
            monitor: None,
        };
        let key = crate::edit::merge_key(&e);
        apply(&mut p, e).unwrap();
        h.record(before, key);
    }
    assert!(h.undo(&mut p));
    assert_eq!(p.tracks[0].volume, 0.8);
    assert!(!h.can_undo());
}

#[test]
fn note_names() {
    assert_eq!(presets::note_number("C4"), Some(60));
    assert_eq!(presets::note_number("A4"), Some(69));
    assert_eq!(presets::note_number("F#5"), Some(78));
    assert_eq!(presets::note_number("Bb3"), Some(58));
}

#[test]
fn tempo_map_follows_changes_and_ramps() {
    let m = TempoMap::new(120.0, &[TempoPoint { tick: 384, bpm: 60.0, ramp: false }]);
    assert!((m.tick_to_secs(384.0) - 2.0).abs() < 1e-9);
    assert!((m.tick_to_secs(768.0) - 6.0).abs() < 1e-9);
    assert!((m.secs_to_tick(4.0) - 576.0).abs() < 1e-6);
    let r = TempoMap::new(120.0, &[TempoPoint { tick: 384, bpm: 60.0, ramp: true }]);
    let t = r.tick_to_secs(384.0);
    assert!(t > 2.0 && t < 4.0, "ramp should land between the two tempos ({t})");
    assert!((r.secs_to_tick(r.tick_to_secs(500.0)) - 500.0).abs() < 1e-6);
}

#[test]
fn automation_shapes_the_output() {
    let assets = AssetStore::default();
    let mut p = presets::template("chip-route");
    // Master volume: full for bar 1, silent from bar 3 on.
    apply(
        &mut p,
        Edit::AddAutomationLane {
            track_id: None,
            target: "volume".into(),
            points: Some(vec![AutoPoint { tick: 384, value: 0.9 }, AutoPoint { tick: 768, value: 0.0 }]),
        },
    )
    .unwrap();
    let (l, r) = render_mix(&p, &assets, 48000, |_| {}).unwrap();
    let bar_secs = 4.0 * 60.0 / p.bpm;
    let seg = |a: f64, b: f64| {
        let (s, e) = ((a * 48000.0) as usize, (b * 48000.0) as usize);
        stats(&l[s..e], &r[s..e]).1
    };
    assert!(seg(0.0, bar_secs) > 0.05);
    assert!(seg(3.0 * bar_secs, 5.0 * bar_secs) < 1e-4);
    // Bad targets are rejected with a helpful message.
    let err = apply(&mut p, Edit::AddAutomationLane { track_id: None, target: "cutoff".into(), points: None }).unwrap_err();
    assert!(err.contains("Can't automate"));
}
