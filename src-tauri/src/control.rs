//! Local control API for AI assistants and scripts (used by the MCP server in
//! tools/mcp). Plain HTTP on 127.0.0.1 with a random token; the port and token
//! are written to `<app data>/control.json`.
//!
//! POST /api  {"method": "...", "params": {...}}  ->  {"ok": true, "result": ...} | {"ok": false, "error": "..."}

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::assets::AssetStore;
use crate::compose::{self, SongSpec};
use crate::edit::{self, Edit};
use crate::engine::EngineMsg;
use crate::export;
use crate::model::*;
use crate::notation;
use crate::presets;
use crate::state::{AppState, Inner};

pub const DEFAULT_PORT: u16 = 47615;

fn random_token() -> String {
    let mut s = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i ^ std::process::id() as u64);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s
}

/// Starts the server thread and writes control.json. Returns the port.
pub fn start(app: AppHandle, data_dir: &Path) -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", DEFAULT_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|e| format!("control API: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = random_token();
    let info = json!({ "port": port, "token": token, "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION") });
    std::fs::write(data_dir.join("control.json"), serde_json::to_string_pretty(&info).unwrap())
        .map_err(|e| format!("control API: {e}"))?;
    std::thread::Builder::new()
        .name("control-api".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let app = app.clone();
                let token = token.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &app, &token);
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(port)
}

fn handle(mut stream: TcpStream, app: &AppHandle, token: &str) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut auth = String::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "x-token" => auth = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; len.min(64 << 20)];
    reader.read_exact(&mut body)?;
    let (status, out) = if method != "POST" || path != "/api" {
        ("404 Not Found", json!({"ok": false, "error": "POST /api only"}))
    } else if auth != token {
        ("401 Unauthorized", json!({"ok": false, "error": "bad token (see control.json)"}))
    } else {
        let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let m = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));
        match dispatch(app, m, params) {
            Ok(v) => ("200 OK", json!({"ok": true, "result": v})),
            Err(e) => ("200 OK", json!({"ok": false, "error": e})),
        }
    };
    let body = serde_json::to_vec(&out).unwrap_or_default();
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(&body)?;
    stream.flush()
}

type R = Result<Value, String>;

fn with_inner<T>(app: &AppHandle, f: impl FnOnce(&mut Inner) -> T) -> T {
    let st = app.state::<AppState>();
    let mut g = st.inner.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g)
}

/// Tells the UI the project changed (with an optional message for a toast).
pub fn notify_changed(app: &AppHandle, message: Option<String>) {
    let resp = with_inner(app, |g| g.response(None));
    let _ = app.emit("project-changed", json!({ "response": resp, "message": message }));
}

fn dispatch(app: &AppHandle, method: &str, p: Value) -> R {
    let msg = p.get("message").and_then(|m| m.as_str()).map(String::from);
    match method {
        "status" => status(app),
        "get_project" => with_inner(app, |g| Ok(summary(&g.project))),
        "get_project_full" => with_inner(app, |g| serde_json::to_value(&g.project).map_err(|e| e.to_string())),
        "get_pattern" => get_pattern(app, &p),
        "get_instrument" => with_inner(app, |g| {
            let t = find_track(&g.project, p.get("track"))?;
            serde_json::to_value(t.instrument().ok_or("Not an instrument track")?).map_err(|e| e.to_string())
        }),
        "compose" => {
            let spec: SongSpec = serde_json::from_value(p.get("song").cloned().unwrap_or(p.clone()))
                .map_err(|e| format!("Invalid song spec: {e}"))?;
            let r = with_inner(app, |g| g.apply_with(None, |proj| compose::apply(proj, spec)))?;
            notify_changed(app, msg.or(Some("Song updated by the assistant".into())));
            Ok(json!({ "tracks": r.tracks.iter().map(|(n, id)| json!({"name": n, "id": id})).collect::<Vec<_>>(), "warnings": r.warnings }))
        }
        "edit" => {
            let edits = p.get("edits").and_then(|e| e.as_array()).cloned().ok_or("params.edits must be an array")?;
            let ids = with_inner(app, |g| g.apply_with(None, |proj| apply_edits(proj, edits)))?;
            notify_changed(app, msg);
            Ok(json!({ "newIds": ids }))
        }
        "undo" | "redo" => {
            let ok = with_inner(app, |g| if method == "undo" { g.undo() } else { g.redo() });
            notify_changed(app, msg);
            Ok(json!({ "done": ok }))
        }
        "transport" => {
            let action = p.get("action").and_then(|a| a.as_str()).unwrap_or("play");
            with_inner(app, |g| {
                if let Some(bar) = p.get("bar").and_then(|b| b.as_f64()) {
                    let t = (bar - 1.0).max(0.0) * g.project.bar_ticks() as f64;
                    g.send(EngineMsg::Seek(t));
                }
                match action {
                    "play" => g.send(EngineMsg::Play { record: false }),
                    "stop" => {
                        if g.is_recording() {
                            g.stop_recording()?;
                        } else {
                            g.send(EngineMsg::Stop);
                        }
                    }
                    "seek" => {}
                    _ => return Err("action must be play, stop or seek".to_string()),
                }
                Ok(json!({ "action": action }))
            })
        }
        "list_presets" => {
            const FX: [&str; 8] = ["eq", "filter", "delay", "reverb", "compressor", "chorus", "bitcrusher", "pump"];
            let effects: Vec<Value> = FX.iter().map(|n| json!({"type": n, "defaults": EffectKind::default_for(n)})).collect();
            Ok(json!({
            "presets": presets::all_presets().iter().map(|x| json!({"name": x.name, "category": x.category, "type": inst_type(&x.instrument)})).collect::<Vec<_>>(),
            "effects": effects,
            "automationTargets": "volume (0..1.5), pan (-1..1), cutoff (synth filter offset in octaves, needs filterOn), pitch (semitones), fx:<effect>:mix (0..1), fx:<filter effect>:cutoff (Hz)",
            "drumKeys": notation::DRUM_NAMES.iter().map(|(n, p)| format!("{n}={p}")).collect::<Vec<_>>().join(" "),
            }))
        }
        "save_preset" => {
            let name = p.get("name").and_then(|n| n.as_str()).ok_or("name required")?.to_string();
            let inst = with_inner(app, |g| find_track(&g.project, p.get("track")).and_then(|t| t.instrument().cloned().ok_or("Not an instrument track".into())))?;
            presets::save_user_preset(&name, inst)?;
            let _ = app.emit("presets-changed", ());
            Ok(json!({ "saved": name }))
        }
        "render_check" => render_check(app, &p),
        "media_info" => with_inner(app, |g| Ok(json!(media_clips(&g.project)))),
        "media_frame" => media_frame(app, &p),
        "media_overview" => media_overview(app, &p),
        "detect_scenes" => detect_scenes(app, &p),
        "notify" => {
            let _ = app.emit("assistant-notify", msg.unwrap_or_default());
            Ok(json!({}))
        }
        _ => Err(format!("Unknown method '{method}'")),
    }
}

fn inst_type(i: &Instrument) -> &'static str {
    match i {
        Instrument::Synth(_) => "synth",
        Instrument::Sampler(_) => "sampler",
        Instrument::Drums(_) => "drums",
    }
}

fn status(app: &AppHandle) -> R {
    with_inner(app, |g| {
        let (playing, pos) = g
            .audio
            .as_ref()
            .map(|a| (a.shared.playing.load(Ordering::Relaxed), a.shared.position()))
            .unwrap_or((false, 0.0));
        let tempo = g.project.tempo_map();
        let bar = round3(pos / g.project.bar_ticks() as f64 + 1.0);
        let secs = round3(tempo.tick_to_secs(pos));
        let has_media = g.project.tracks.iter().any(|t| t.is_media() && !t.clips.is_empty());
        Ok(json!({
            "app": "Music Maker",
            "project": g.project.name,
            "path": g.path,
            "dirty": g.dirty,
            "audioRunning": g.audio.is_some(),
            "playing": playing,
            "playheadBar": bar,
            "playheadSecs": secs,
            "hasMedia": has_media,
            "ui": g.ui_context,
        }))
    })
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn bar_of(p: &Project, tick: u64) -> f64 {
    round3(tick as f64 / p.bar_ticks() as f64 + 1.0)
}

fn find_track<'a>(p: &'a Project, r: Option<&Value>) -> Result<&'a Track, String> {
    match r {
        Some(Value::Number(n)) => p.track(n.as_u64().unwrap_or(0)).ok_or_else(|| "Track not found".into()),
        Some(Value::String(s)) => p
            .tracks
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("No track named '{s}'")),
        _ => Err("params.track (id or name) is required".into()),
    }
}

/// Converts notes to notation text (monophonic voices on separate lines).
pub fn notes_to_text(notes: &[Note]) -> String {
    let step = (PPQ / 4) as f64;
    let mut sorted = notes.to_vec();
    sorted.sort_by_key(|n| (n.start, n.pitch));
    let mut voices: Vec<(u64, Vec<String>)> = vec![];
    let fmt_len = |ticks: f64| {
        let s = ticks / step;
        if (s - s.round()).abs() < 1e-6 {
            format!("{}", s.round() as i64)
        } else {
            format!("{:.3}", s).trim_end_matches('0').to_string()
        }
    };
    for n in &sorted {
        let vi = match voices.iter().position(|(end, _)| *end <= n.start) {
            Some(i) => i,
            None => {
                voices.push((0, vec![]));
                voices.len() - 1
            }
        };
        let (end, toks) = &mut voices[vi];
        if n.start > *end {
            toks.push(format!("r:{}", fmt_len((n.start - *end) as f64)));
        }
        let vel = if (n.velocity - 0.8).abs() > 0.005 { format!("@{:.2}", n.velocity) } else { String::new() };
        toks.push(format!("{}:{}{}", notation::note_name(n.pitch), fmt_len(n.length as f64), vel));
        *end = n.start + n.length;
    }
    voices.into_iter().map(|(_, t)| t.join(" ")).collect::<Vec<_>>().join("\n")
}

fn summary(p: &Project) -> Value {
    let bt = p.bar_ticks();
    let fx_json = |e: &Effect| {
        let mut v = serde_json::to_value(&e.kind).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.insert("id".into(), json!(e.id));
            m.insert("enabled".into(), json!(e.enabled));
        }
        v
    };
    let lanes = |ls: &[AutomationLane]| {
        ls.iter()
            .map(|l| json!({"id": l.id, "target": l.target, "enabled": l.enabled, "points": l.points.iter().map(|x| json!([bar_of(p, x.tick), x.value])).collect::<Vec<_>>()}))
            .collect::<Vec<_>>()
    };
    let tracks: Vec<Value> = p
        .tracks
        .iter()
        .map(|t| {
            let clips: Vec<Value> = t
                .clips
                .iter()
                .map(|c| match &c.content {
                    ClipContent::Pattern { pattern_id } => json!({
                        "id": c.id, "patternId": pattern_id,
                        "pattern": p.pattern(*pattern_id).map(|x| x.name.clone()),
                        "startBar": bar_of(p, c.start), "bars": round3(c.length as f64 / bt as f64),
                    }),
                    ClipContent::Audio { asset_id } | ClipContent::Media { asset_id } => json!({
                        "id": c.id, "assetId": asset_id, "asset": p.asset(*asset_id).map(|a| a.name.clone()),
                        "startBar": bar_of(p, c.start), "bars": round3(c.length as f64 / bt as f64), "offsetSecs": c.offset,
                    }),
                })
                .collect();
            let kind = match &t.kind {
                TrackKind::Instrument { .. } => "instrument",
                TrackKind::Audio => "audio",
                TrackKind::Media => "media",
            };
            json!({
                "id": t.id, "name": t.name, "kind": kind,
                "instrumentType": t.instrument().map(inst_type),
                "volume": t.volume, "pan": t.pan, "mute": t.mute, "solo": t.solo,
                "effects": t.effects.iter().map(fx_json).collect::<Vec<_>>(),
                "automation": lanes(&t.automation),
                "clips": clips,
            })
        })
        .collect();
    let patterns: Vec<Value> = p
        .patterns
        .iter()
        .map(|x| json!({"id": x.id, "name": x.name, "bars": round3(x.length as f64 / bt as f64), "notes": notes_to_text(&x.notes)}))
        .collect();
    const KEYS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    json!({
        "name": p.name,
        "bpm": p.bpm,
        "tempoChanges": p.tempo_changes.iter().map(|t| json!({"bar": bar_of(p, t.tick), "bpm": t.bpm, "ramp": t.ramp})).collect::<Vec<_>>(),
        "timeSig": format!("{}/{}", p.time_sig_num, p.time_sig_den),
        "key": KEYS[(p.key_root % 12) as usize], "scale": p.scale,
        "ticksPerBar": bt, "ticksPerStep": PPQ / 4,
        "lengthBars": round3(p.end_tick_any() as f64 / bt as f64),
        "loop": {"enabled": p.loop_enabled, "startBar": bar_of(p, p.loop_start), "endBar": bar_of(p, p.loop_end)},
        "tracks": tracks,
        "patterns": patterns,
        "master": {"volume": p.master.volume, "effects": p.master.effects.iter().map(fx_json).collect::<Vec<_>>(), "automation": lanes(&p.master.automation)},
        "notation": "Pattern notes use the text notation: NOTE:LEN (LEN in 16th steps), r:LEN rest, @VEL velocity, A+B chord; each line is a voice.",
    })
}

fn get_pattern(app: &AppHandle, p: &Value) -> R {
    with_inner(app, |g| {
        let proj = &g.project;
        let pat = match p.get("pattern") {
            Some(Value::Number(n)) => proj.pattern(n.as_u64().unwrap_or(0)),
            Some(Value::String(s)) => proj.patterns.iter().find(|x| x.name.eq_ignore_ascii_case(s)),
            _ => None,
        }
        .ok_or("params.pattern (id or name) not found")?;
        Ok(json!({
            "id": pat.id, "name": pat.name, "bars": round3(pat.length as f64 / proj.bar_ticks() as f64),
            "text": notes_to_text(&pat.notes),
            "notes": pat.notes.iter().map(|n| json!({"id": n.id, "pitch": n.pitch, "name": notation::note_name(n.pitch), "start": n.start, "length": n.length, "velocity": n.velocity})).collect::<Vec<_>>(),
        }))
    })
}

/// Replaces "$N" strings with the id created by edit N of the same batch.
fn substitute(v: &mut Value, ids: &[Option<u64>]) -> Result<(), String> {
    match v {
        Value::String(s) if s.starts_with('$') => {
            let i: usize = s[1..].parse().map_err(|_| format!("bad reference '{s}'"))?;
            let id = ids.get(i).copied().flatten().ok_or_else(|| format!("'{s}' refers to an edit that created nothing"))?;
            *v = json!(id);
        }
        Value::Array(a) => {
            for x in a {
                substitute(x, ids)?;
            }
        }
        Value::Object(m) => {
            for (_, x) in m.iter_mut() {
                substitute(x, ids)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_edits(p: &mut Project, edits: Vec<Value>) -> Result<Vec<Option<u64>>, String> {
    let mut ids = vec![];
    for (i, mut e) in edits.into_iter().enumerate() {
        substitute(&mut e, &ids).map_err(|err| format!("edit #{i}: {err}"))?;
        let edit: Edit = serde_json::from_value(e).map_err(|err| format!("edit #{i}: {err}"))?;
        ids.push(edit::apply(p, edit).map_err(|err| format!("edit #{i}: {err}"))?);
    }
    Ok(ids)
}

fn db(x: f64) -> f64 {
    if x <= 1e-9 {
        -120.0
    } else {
        round3(20.0 * x.log10())
    }
}

fn stats(l: &[f32], r: &[f32]) -> (f64, f64, usize) {
    let mut peak = 0f64;
    let mut sum = 0f64;
    let mut clips = 0;
    for i in 0..l.len() {
        let a = (l[i].abs().max(r[i].abs())) as f64;
        peak = peak.max(a);
        if a >= 0.979 {
            clips += 1;
        }
        sum += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
    }
    (peak, (sum / (2 * l.len().max(1)) as f64).sqrt(), clips)
}

fn render_check(app: &AppHandle, params: &Value) -> R {
    let (project, assets): (Project, AssetStore) = with_inner(app, |g| (g.project.clone(), g.assets.snapshot()));
    const SR: u32 = 22050;
    let (l, r) = export::render_mix(&project, &assets, SR, |_| {})?;
    let (peak, rms, limited) = stats(&l, &r);
    let tempo = project.tempo_map();
    let bt = project.bar_ticks() as f64;
    let mut bars = vec![];
    let mut b = 0.0;
    loop {
        let s = (tempo.tick_to_secs(b * bt) * SR as f64) as usize;
        let e = ((tempo.tick_to_secs((b + 1.0) * bt) * SR as f64) as usize).min(l.len());
        if s >= l.len() {
            break;
        }
        bars.push(db(stats(&l[s..e], &r[s..e]).1));
        b += 1.0;
    }
    let mut per_track = vec![];
    if params.get("perTrack").and_then(|v| v.as_bool()).unwrap_or(true) {
        for t in project.tracks.iter().filter(|t| !t.is_media() && !t.mute) {
            let mut solo = project.clone();
            for x in solo.tracks.iter_mut() {
                x.solo = x.id == t.id;
            }
            if let Ok((tl, tr)) = export::render_mix(&solo, &assets, SR, |_| {}) {
                let (pk, rm, _) = stats(&tl, &tr);
                per_track.push(json!({"track": t.name, "peakDb": db(pk), "rmsDb": db(rm)}));
            }
        }
    }
    let mut wrote = None;
    if let Some(path) = params.get("path").and_then(|p| p.as_str()) {
        let (fl, fr) = export::render_mix(&project, &assets, export::EXPORT_SR, |_| {})?;
        crate::assets::write_wav(Path::new(path), &fl, &fr, export::EXPORT_SR, 24)?;
        wrote = Some(path.to_string());
    }
    Ok(json!({
        "durationSecs": round3(l.len() as f64 / SR as f64),
        "peakDb": db(peak),
        "rmsDb": db(rms),
        "limiterHitSamples": limited,
        "advice": if limited > SR as usize / 10 { "The master limiter is working hard: lower track volumes or master volume." }
                  else if rms < 0.02 { "Quite quiet overall: raise volumes." } else { "Levels look OK." },
        "barRmsDb": bars,
        "tracks": per_track,
        "wrote": wrote,
    }))
}

// ---------- media (optional video/image) ----------

struct MediaClip {
    track: u64,
    clip: u64,
    asset: AssetRef,
    start: u64,
    end: u64,
    offset: f64,
}

fn media_list(p: &Project) -> Vec<MediaClip> {
    let mut v = vec![];
    for t in p.tracks.iter().filter(|t| t.is_media()) {
        for c in &t.clips {
            if let ClipContent::Media { asset_id } = c.content {
                if let Some(a) = p.asset(asset_id) {
                    v.push(MediaClip { track: t.id, clip: c.id, asset: a.clone(), start: c.start, end: c.start + c.length, offset: c.offset });
                }
            }
        }
    }
    v
}

fn media_clips(p: &Project) -> Vec<Value> {
    let tempo = p.tempo_map();
    media_list(p)
        .into_iter()
        .map(|m| {
            json!({
                "trackId": m.track, "clipId": m.clip, "assetId": m.asset.id, "name": m.asset.name,
                "kind": m.asset.kind, "path": m.asset.path, "fileDurationSecs": m.asset.duration,
                "startSecs": round3(tempo.tick_to_secs(m.start as f64)), "endSecs": round3(tempo.tick_to_secs(m.end as f64)),
                "startBar": bar_of(p, m.start), "endBar": bar_of(p, m.end), "fileOffsetSecs": m.offset,
            })
        })
        .collect()
}

fn ffmpeg_png(args: Vec<String>) -> Result<Vec<u8>, String> {
    let out = export::run_ffmpeg(&args)?;
    if out.is_empty() {
        return Err("ffmpeg produced no image".into());
    }
    Ok(out)
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

fn media_frame(app: &AppHandle, p: &Value) -> R {
    let (proj, playhead) = with_inner(app, |g| (g.project.clone(), g.audio.as_ref().map(|a| a.shared.position()).unwrap_or(0.0)));
    let tempo = proj.tempo_map();
    let tick = if let Some(s) = p.get("secs").and_then(|v| v.as_f64()) {
        tempo.secs_to_tick(s)
    } else if let Some(b) = p.get("bar").and_then(|v| v.as_f64()) {
        (b - 1.0).max(0.0) * proj.bar_ticks() as f64
    } else {
        playhead
    };
    let list = media_list(&proj);
    let m = list
        .iter()
        .find(|m| tick >= m.start as f64 && tick < m.end as f64)
        .ok_or_else(|| format!("No video/image at that time. Media clips: {}", json!(media_clips(&proj))))?;
    let width = p.get("width").and_then(|v| v.as_u64()).unwrap_or(768).clamp(64, 1920);
    let file_t = m.offset + tempo.tick_to_secs(tick) - tempo.tick_to_secs(m.start as f64);
    let mut args: Vec<String> = vec!["-v".into(), "error".into()];
    if m.asset.kind == AssetKind::Video {
        args.extend(["-ss".into(), format!("{file_t:.3}")]);
    }
    args.extend(["-i".into(), m.asset.path.clone(), "-frames:v".into(), "1".into()]);
    args.extend(["-vf".into(), format!("scale='min({width},iw)':-2"), "-f".into(), "image2pipe".into(), "-vcodec".into(), "png".into(), "-".into()]);
    let png = ffmpeg_png(args)?;
    Ok(json!({
        "image": {"mimeType": "image/png", "data": b64(&png)},
        "info": {"asset": m.asset.name, "kind": m.asset.kind, "songSecs": round3(tempo.tick_to_secs(tick)), "bar": round3(tick / proj.bar_ticks() as f64 + 1.0), "fileSecs": round3(file_t)},
    }))
}

fn pick_asset<'a>(list: &'a [MediaClip], p: &Value) -> Result<&'a MediaClip, String> {
    match p.get("assetId").and_then(|v| v.as_u64()) {
        Some(id) => list.iter().find(|m| m.asset.id == id).ok_or_else(|| "That asset isn't on a media track".into()),
        None => list.first().ok_or_else(|| "The project has no video or image clips".into()),
    }
}

fn media_overview(app: &AppHandle, p: &Value) -> R {
    let proj = with_inner(app, |g| g.project.clone());
    let list = media_list(&proj);
    let m = pick_asset(&list, p)?;
    let cols = p.get("cols").and_then(|v| v.as_u64()).unwrap_or(4).clamp(1, 8);
    let rows = p.get("rows").and_then(|v| v.as_u64()).unwrap_or(3).clamp(1, 8);
    let width = p.get("width").and_then(|v| v.as_u64()).unwrap_or(1280).clamp(256, 2400);
    if m.asset.kind == AssetKind::Image {
        let png = ffmpeg_png(vec![
            "-v".into(), "error".into(), "-i".into(), m.asset.path.clone(), "-frames:v".into(), "1".into(),
            "-vf".into(), format!("scale='min({width},iw)':-2"), "-f".into(), "image2pipe".into(), "-vcodec".into(), "png".into(), "-".into(),
        ])?;
        return Ok(json!({"image": {"mimeType": "image/png", "data": b64(&png)}, "info": {"asset": m.asset.name, "kind": "image"}}));
    }
    let n = cols * rows;
    let dur = m.asset.duration.max(0.1);
    let png = ffmpeg_png(vec![
        "-v".into(), "error".into(), "-i".into(), m.asset.path.clone(),
        "-vf".into(), format!("fps={n}/{dur:.3},scale={}:-2,tile={cols}x{rows}", width / cols),
        "-frames:v".into(), "1".into(), "-f".into(), "image2pipe".into(), "-vcodec".into(), "png".into(), "-".into(),
    ])?;
    let times: Vec<f64> = (0..n).map(|i| round3((i as f64 + 0.5) * dur / n as f64)).collect();
    Ok(json!({
        "image": {"mimeType": "image/png", "data": b64(&png)},
        "info": {"asset": m.asset.name, "grid": format!("{cols}x{rows}, left-to-right then top-to-bottom"), "fileSecsPerTile": times,
                 "placements": media_clips(&proj).into_iter().filter(|c| c["assetId"] == json!(m.asset.id)).collect::<Vec<_>>()},
    }))
}

fn detect_scenes(app: &AppHandle, p: &Value) -> R {
    let proj = with_inner(app, |g| g.project.clone());
    let list = media_list(&proj);
    let m = pick_asset(&list, p)?;
    if m.asset.kind != AssetKind::Video {
        return Err("Scene detection needs a video".into());
    }
    let thr = p.get("threshold").and_then(|v| v.as_f64()).unwrap_or(0.3).clamp(0.05, 0.95);
    let err = export::run_ffmpeg_stderr(&[
        "-hide_banner".into(), "-i".into(), m.asset.path.clone(),
        "-vf".into(), format!("select='gt(scene,{thr})',showinfo"), "-an".into(), "-f".into(), "null".into(), "-".into(),
    ])?;
    let cuts: Vec<f64> = err
        .lines()
        .filter_map(|l| l.split("pts_time:").nth(1))
        .filter_map(|r| r.split_whitespace().next()?.parse::<f64>().ok())
        .collect();
    let tempo = proj.tempo_map();
    let placements: Vec<Value> = list
        .iter()
        .filter(|c| c.asset.id == m.asset.id)
        .map(|c| {
            let start_secs = tempo.tick_to_secs(c.start as f64);
            let end_secs = tempo.tick_to_secs(c.end as f64);
            let song: Vec<Value> = cuts
                .iter()
                .map(|t| start_secs + t - c.offset)
                .filter(|s| *s >= start_secs && *s < end_secs)
                .map(|s| json!({"songSecs": round3(s), "bar": round3(tempo.secs_to_tick(s) / proj.bar_ticks() as f64 + 1.0)}))
                .collect();
            json!({"clipId": c.clip, "cuts": song})
        })
        .collect();
    Ok(json!({"asset": m.asset.name, "fileCutSecs": cuts.iter().map(|x| round3(*x)).collect::<Vec<_>>(), "placements": placements}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_text_roundtrips() {
        let notes = notation::parse("C4:4 r:2 D4:2@0.5 E4:1.5\nG3:8 G3:8").unwrap();
        let text = notes_to_text(&notes);
        let back = notation::parse(&text).unwrap();
        let key = |v: &[Note]| v.iter().map(|n| (n.pitch, n.start, n.length, (n.velocity * 100.0) as i32)).collect::<Vec<_>>();
        assert_eq!(key(&back), key(&notes), "{text}");
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"Ma"), "TWE=");
        assert_eq!(b64(b"M"), "TQ==");
    }

    #[test]
    fn substitutes_references() {
        let mut p = presets::template("empty");
        let tid = p.tracks[0].id;
        let ids = apply_edits(
            &mut p,
            vec![
                json!({"type": "addPattern", "text": "C4:4 E4:4"}),
                json!({"type": "addClip", "trackId": tid, "start": 0, "length": 384, "content": {"type": "pattern", "patternId": "$0"}}),
            ],
        )
        .unwrap();
        let pid = ids[0].unwrap();
        assert!(p.tracks[0].clips.iter().any(|c| matches!(c.content, ClipContent::Pattern { pattern_id } if pattern_id == pid)));
    }
}
