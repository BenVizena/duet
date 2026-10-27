//! Tauri commands called from the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

use crate::assets::{decode_file, write_wav};
use crate::edit::{self, Edit};
use crate::engine::EngineMsg;
use crate::model::*;
use crate::state::{AppState, ProjectResponse};
use crate::{audio_io, export, midi, presets, project_io};

type R<T> = Result<T, String>;

fn lock<'a>(s: &'a State<'_, AppState>) -> std::sync::MutexGuard<'a, crate::state::Inner> {
    s.inner.lock().unwrap_or_else(|e| e.into_inner())
}

const AUDIO_EXT: [&str; 9] = ["wav", "mp3", "flac", "ogg", "oga", "m4a", "aac", "aif", "aiff"];
const VIDEO_EXT: [&str; 5] = ["mp4", "webm", "m4v", "mov", "ogv"];
const IMAGE_EXT: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

fn ext_of(p: &str) -> String {
    Path::new(p).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string())
}

#[tauri::command]
pub fn get_state(state: State<'_, AppState>) -> R<serde_json::Value> {
    let g = lock(&state);
    Ok(json!({
        "response": g.response(None),
        "presets": presets::all_presets(),
        "templates": presets::templates(),
        "metronome": g.metronome,
    }))
}

#[tauri::command]
pub fn get_presets() -> Vec<presets::Preset> {
    presets::all_presets()
}

#[tauri::command]
pub fn save_user_preset(state: State<'_, AppState>, name: String, track_id: u64) -> R<Vec<presets::Preset>> {
    let inst = {
        let g = lock(&state);
        g.project.track(track_id).and_then(|t| t.instrument().cloned()).ok_or("Select an instrument track")?
    };
    presets::save_user_preset(&name, inst)?;
    Ok(presets::all_presets())
}

#[tauri::command]
pub fn delete_user_preset(name: String) -> R<Vec<presets::Preset>> {
    presets::delete_user_preset(&name)?;
    Ok(presets::all_presets())
}

/// The UI reports what's selected so the assistant knows what "this" means.
#[tauri::command]
pub fn set_ui_context(state: State<'_, AppState>, context: serde_json::Value) {
    lock(&state).ui_context = context;
}

#[tauri::command]
pub fn assistant_status() -> serde_json::Value {
    crate::assistant::status()
}

#[tauri::command(async)]
pub fn assistant_ask(app: AppHandle, state: State<'_, AppState>, prompt: String) -> R<()> {
    let context = {
        let g = lock(&state);
        let pos = g.audio.as_ref().map(|a| a.shared.position()).unwrap_or(0.0);
        json!({
            "project": g.project.name,
            "playheadBar": (pos / g.project.bar_ticks() as f64 * 1000.0).round() / 1000.0 + 1.0,
            "ui": g.ui_context,
        })
    };
    crate::assistant::ask(app, prompt, context)
}

#[tauri::command]
pub fn assistant_cancel() {
    crate::assistant::cancel();
}

#[tauri::command]
pub fn assistant_reset() {
    crate::assistant::reset();
}

#[tauri::command]
pub fn apply_edit(state: State<'_, AppState>, edit: Edit) -> R<ProjectResponse> {
    let mut g = lock(&state);
    let arms = matches!(edit, Edit::UpdateTrack { armed: Some(true), .. } | Edit::UpdateTrack { monitor: Some(true), .. });
    let id = g.apply(edit)?;
    let mut resp = g.response(id);
    if arms {
        if let Err(e) = g.ensure_input_for_armed() {
            resp.warnings.push(e);
        }
    }
    Ok(resp)
}

#[tauri::command]
pub fn undo(state: State<'_, AppState>) -> R<ProjectResponse> {
    let mut g = lock(&state);
    g.undo();
    Ok(g.response(None))
}

#[tauri::command]
pub fn redo(state: State<'_, AppState>) -> R<ProjectResponse> {
    let mut g = lock(&state);
    g.redo();
    Ok(g.response(None))
}

/// action: "play" | "stop" | "toggle". Returns a project response when stopping
/// a recording created new clips.
#[tauri::command(async)]
pub fn transport(state: State<'_, AppState>, action: String) -> R<Option<ProjectResponse>> {
    let mut g = lock(&state);
    let playing = g.audio.as_ref().map(|a| a.shared.playing.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false);
    let stop = action == "stop" || (action == "toggle" && (playing || g.is_recording()));
    if stop {
        if g.is_recording() {
            g.stop_recording()?;
            return Ok(Some(g.response(None)));
        }
        g.send(EngineMsg::Stop);
    } else {
        if g.audio.is_none() {
            return Err(g.audio_error.clone().unwrap_or_else(|| "Audio output isn't running".into()));
        }
        g.send(EngineMsg::Play { record: false });
    }
    Ok(None)
}

#[tauri::command]
pub fn record_start(state: State<'_, AppState>) -> R<()> {
    lock(&state).start_recording()
}

#[tauri::command]
pub fn seek(state: State<'_, AppState>, tick: f64) {
    lock(&state).send(EngineMsg::Seek(tick));
}

#[tauri::command]
pub fn set_metronome(state: State<'_, AppState>, on: bool) {
    let mut g = lock(&state);
    g.metronome = on;
    g.send(EngineMsg::SetMetronome(on));
}

#[tauri::command]
pub fn note_on(state: State<'_, AppState>, track_id: u64, pitch: u8, vel: f32) {
    lock(&state).note_on(track_id, pitch, vel);
}

#[tauri::command]
pub fn note_off(state: State<'_, AppState>, track_id: u64, pitch: u8) {
    lock(&state).note_off(track_id, pitch);
}

#[tauri::command]
pub fn all_notes_off(state: State<'_, AppState>) {
    lock(&state).send(EngineMsg::AllNotesOff);
}

#[tauri::command]
pub fn set_selected_track(state: State<'_, AppState>, track_id: Option<u64>) {
    lock(&state).selected_track = track_id;
}

fn pick_audio_files(app: &AppHandle, multiple: bool) -> Vec<String> {
    let d = app.dialog().file().add_filter("Audio", &AUDIO_EXT);
    let picked = if multiple { d.blocking_pick_files() } else { d.blocking_pick_file().map(|f| vec![f]) };
    picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Decodes audio files and puts them on the timeline (on `track_id` if given, else new audio tracks).
#[tauri::command(async)]
pub fn import_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Option<Vec<String>>,
    track_id: Option<u64>,
    start: Option<u64>,
) -> R<Option<ProjectResponse>> {
    let paths = match paths {
        Some(p) => p,
        None => pick_audio_files(&app, true),
    };
    if paths.is_empty() {
        return Ok(None);
    }
    let mut decoded = vec![];
    let mut warnings = vec![];
    for p in &paths {
        match decode_file(Path::new(p)) {
            Ok(d) => decoded.push((p.clone(), Arc::new(d))),
            Err(e) => warnings.push(e),
        }
    }
    let mut g = lock(&state);
    if decoded.is_empty() {
        return Err(warnings.join("\n"));
    }
    let start = start.unwrap_or(0);
    let mut items = vec![];
    for (path, data) in decoded {
        let id = g.project.alloc_id();
        g.assets.insert(id, data.clone());
        items.push((id, path, data));
    }
    let id = g.apply_with(None, |p| {
        let mut last = None;
        for (i, (asset_id, path, data)) in items.into_iter().enumerate() {
            edit::apply(
                p,
                Edit::AddAsset {
                    asset: AssetRef {
                        id: asset_id,
                        name: file_name(&path),
                        path: path.clone(),
                        kind: AssetKind::Audio,
                        duration: data.duration(),
                        sample_rate: data.sr,
                    },
                },
            )?;
            let target = match track_id.filter(|t| i == 0 && p.track(*t).map(|t| t.is_audio()).unwrap_or(false)) {
                Some(t) => t,
                None => edit::apply(
                    p,
                    Edit::AddTrack { kind: "audio".into(), name: Some(file_name(&path)), instrument: None, index: None },
                )?
                .unwrap(),
            };
            last = edit::apply(
                p,
                Edit::AddClip {
                    track_id: target,
                    start,
                    length: p.duration_ticks(start, data.duration()),
                    offset: None,
                    content: ClipContent::Audio { asset_id },
                },
            )?;
        }
        Ok(last)
    })?;
    let mut resp = g.response(id);
    resp.warnings = warnings;
    Ok(Some(resp))
}

/// Loads a sample into a sampler track.
#[tauri::command(async)]
pub fn load_sample(app: AppHandle, state: State<'_, AppState>, track_id: u64, path: Option<String>) -> R<Option<ProjectResponse>> {
    let path = match path {
        Some(p) => p,
        None => match pick_audio_files(&app, false).into_iter().next() {
            Some(p) => p,
            None => return Ok(None),
        },
    };
    let data = Arc::new(decode_file(Path::new(&path))?);
    let mut g = lock(&state);
    let asset_id = g.project.alloc_id();
    g.assets.insert(asset_id, data.clone());
    g.apply_with(None, |p| {
        edit::apply(
            p,
            Edit::AddAsset {
                asset: AssetRef {
                    id: asset_id,
                    name: file_name(&path),
                    path: path.clone(),
                    kind: AssetKind::Audio,
                    duration: data.duration(),
                    sample_rate: data.sr,
                },
            },
        )?;
        let t = p.track_mut(track_id).ok_or("Track not found")?;
        match &mut t.kind {
            TrackKind::Instrument { instrument } => {
                let mut sp = match instrument {
                    Instrument::Sampler(s) => s.clone(),
                    _ => SamplerParams::default(),
                };
                sp.asset_id = Some(asset_id);
                *instrument = Instrument::Sampler(sp);
            }
            _ => return Err("Samples go on instrument tracks".into()),
        }
        Ok(())
    })?;
    Ok(Some(g.response(None)))
}

#[tauri::command(async)]
pub fn pick_media(app: AppHandle) -> Vec<String> {
    let mut all: Vec<&str> = VIDEO_EXT.to_vec();
    all.extend(IMAGE_EXT);
    app.dialog()
        .file()
        .add_filter("Video & images", &all)
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileKind {
    path: String,
    kind: String,
}

/// Classifies dropped files: "audio" | "video" | "image" | "project" | "unknown".
#[tauri::command]
pub fn classify_files(paths: Vec<String>) -> Vec<FileKind> {
    paths
        .into_iter()
        .map(|p| {
            let e = ext_of(&p);
            let kind = if AUDIO_EXT.contains(&e.as_str()) {
                "audio"
            } else if VIDEO_EXT.contains(&e.as_str()) {
                "video"
            } else if IMAGE_EXT.contains(&e.as_str()) {
                "image"
            } else if e == project_io::EXTENSION {
                "project"
            } else {
                "unknown"
            };
            FileKind { path: p, kind: kind.into() }
        })
        .collect()
}

/// Adds a video or image to a media track (creating one if needed).
/// `duration` comes from the UI, which reads it from the video element.
#[tauri::command]
pub fn add_media(
    state: State<'_, AppState>,
    path: String,
    kind: String,
    duration: f64,
    start: u64,
    track_id: Option<u64>,
) -> R<ProjectResponse> {
    let kind = match kind.as_str() {
        "video" => AssetKind::Video,
        "image" => AssetKind::Image,
        _ => return Err("Only videos and images go on media tracks".into()),
    };
    let mut g = lock(&state);
    let id = g.apply_with(None, |p| {
        let asset_id = edit::apply(
            p,
            Edit::AddAsset {
                asset: AssetRef { id: 0, name: file_name(&path), path: path.clone(), kind, duration, sample_rate: 0 },
            },
        )?
        .unwrap();
        let target = match track_id.filter(|t| p.track(*t).map(|t| t.is_media()).unwrap_or(false)) {
            Some(t) => t,
            None => match p.tracks.iter().find(|t| t.is_media()) {
                Some(t) => t.id,
                None => edit::apply(p, Edit::AddTrack { kind: "media".into(), name: None, instrument: None, index: Some(0) })?
                    .unwrap(),
            },
        };
        let length = if kind == AssetKind::Image { p.bar_ticks() * 2 } else { p.duration_ticks(start, duration.max(0.1)) };
        edit::apply(
            p,
            Edit::AddClip { track_id: target, start, length, offset: None, content: ClipContent::Media { asset_id } },
        )
    })?;
    Ok(g.response(id))
}

/// Decodes a video's soundtrack and places it on a new audio track under each of its clips.
#[tauri::command(async)]
pub fn extract_video_audio(state: State<'_, AppState>, asset_id: u64) -> R<ProjectResponse> {
    let (path, name) = {
        let g = lock(&state);
        let a = g.project.asset(asset_id).ok_or("Asset not found")?;
        (a.path.clone(), a.name.clone())
    };
    let data = Arc::new(decode_file(Path::new(&path)).map_err(|e| format!("Couldn't read the video's audio: {e}"))?);
    let mut g = lock(&state);
    let audio_id = g.project.alloc_id();
    g.assets.insert(audio_id, data.clone());
    g.apply_with(None, |p| {
        edit::apply(
            p,
            Edit::AddAsset {
                asset: AssetRef {
                    id: audio_id,
                    name: format!("{name} (audio)"),
                    path: path.clone(),
                    kind: AssetKind::Audio,
                    duration: data.duration(),
                    sample_rate: data.sr,
                },
            },
        )?;
        let clips: Vec<(u64, u64, f64)> = p
            .tracks
            .iter()
            .filter(|t| t.is_media())
            .flat_map(|t| t.clips.iter())
            .filter(|c| matches!(c.content, ClipContent::Media { asset_id: a } if a == asset_id))
            .map(|c| (c.start, c.length, c.offset))
            .collect();
        let track = edit::apply(
            p,
            Edit::AddTrack { kind: "audio".into(), name: Some(format!("{name} audio")), instrument: None, index: None },
        )?
        .unwrap();
        for (start, length, offset) in clips {
            edit::apply(
                p,
                Edit::AddClip { track_id: track, start, length, offset: Some(offset), content: ClipContent::Audio { asset_id: audio_id } },
            )?;
        }
        Ok(())
    })?;
    Ok(g.response(None))
}

#[tauri::command]
pub fn get_peaks(state: State<'_, AppState>, asset_id: u64) -> R<tauri::ipc::Response> {
    let mut g = lock(&state);
    let p = g.assets.peaks(asset_id).ok_or("No audio data for this asset")?;
    Ok(tauri::ipc::Response::new(p.as_ref().clone()))
}

#[tauri::command]
pub fn new_project(state: State<'_, AppState>, template: String) -> R<ProjectResponse> {
    let mut g = lock(&state);
    g.replace_project(presets::template(&template), None);
    Ok(g.response(None))
}

fn load_project_assets(g: &mut crate::state::Inner) -> Vec<String> {
    let mut warnings = vec![];
    let audio: Vec<(u64, String)> = g
        .project
        .assets
        .iter()
        .filter(|a| a.kind == AssetKind::Audio && g.assets.get(a.id).is_none())
        .map(|a| (a.id, a.path.clone()))
        .collect();
    for (id, path) in audio {
        match decode_file(Path::new(&path)) {
            Ok(d) => g.assets.insert(id, Arc::new(d)),
            Err(_) => warnings.push(format!("Missing audio file: {path}")),
        }
    }
    for a in g.project.assets.iter().filter(|a| a.kind != AssetKind::Audio) {
        if !Path::new(&a.path).exists() {
            warnings.push(format!("Missing media file: {}", a.path));
        }
    }
    warnings
}

#[tauri::command(async)]
pub fn open_project(app: AppHandle, state: State<'_, AppState>, path: Option<String>) -> R<Option<ProjectResponse>> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => match app
            .dialog()
            .file()
            .add_filter("Music Maker project", &[project_io::EXTENSION])
            .blocking_pick_file()
            .and_then(|f| f.into_path().ok())
        {
            Some(p) => p,
            None => return Ok(None),
        },
    };
    let project = project_io::load(&path)?;
    let mut g = lock(&state);
    // Decode before swapping in, so the engine never sees clips without data.
    let old = std::mem::replace(&mut g.project, project);
    let warnings = load_project_assets(&mut g);
    let project = std::mem::replace(&mut g.project, old);
    g.replace_project(project, Some(path));
    let mut resp = g.response(None);
    resp.warnings = warnings;
    Ok(Some(resp))
}

#[tauri::command(async)]
pub fn save_project(app: AppHandle, state: State<'_, AppState>, save_as: bool) -> R<Option<ProjectResponse>> {
    let (existing, name) = {
        let g = lock(&state);
        (g.path.clone(), g.project.name.clone())
    };
    let path = match existing.filter(|_| !save_as) {
        Some(p) => p,
        None => match app
            .dialog()
            .file()
            .set_file_name(format!("{name}.{}", project_io::EXTENSION))
            .add_filter("Music Maker project", &[project_io::EXTENSION])
            .blocking_save_file()
            .and_then(|f| f.into_path().ok())
        {
            Some(mut p) => {
                if p.extension().is_none() {
                    p.set_extension(project_io::EXTENSION);
                }
                p
            }
            None => return Ok(None),
        },
    };
    let mut g = lock(&state);
    if save_as || g.path.is_none() {
        if let Some(stem) = path.file_stem() {
            g.project.name = stem.to_string_lossy().into_owned();
        }
    }
    let rec_dir = g.recordings_dir();
    let updated = project_io::save(&path, &g.project, &rec_dir)?;
    g.project = updated;
    g.path = Some(path);
    g.dirty = false;
    Ok(Some(g.response(None)))
}

#[tauri::command(async)]
pub fn export_audio(app: AppHandle, state: State<'_, AppState>) -> R<Option<String>> {
    let name = lock(&state).project.name.clone();
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name(format!("{name}.wav"))
        .add_filter("WAV audio", &["wav"])
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
    else {
        return Ok(None);
    };
    let (project, assets) = {
        let g = lock(&state);
        (g.project.clone(), g.assets.snapshot())
    };
    let app2 = app.clone();
    let (l, r) = export::render_mix(&project, &assets, export::EXPORT_SR, move |p| {
        let _ = app2.emit("export-progress", p);
    })?;
    write_wav(&path, &l, &r, export::EXPORT_SR, 24)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command(async)]
pub fn export_video(app: AppHandle, state: State<'_, AppState>) -> R<Option<String>> {
    let name = lock(&state).project.name.clone();
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name(format!("{name}.mp4"))
        .add_filter("MP4 video", &["mp4"])
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
    else {
        return Ok(None);
    };
    let (project, assets) = {
        let g = lock(&state);
        (g.project.clone(), g.assets.snapshot())
    };
    let app2 = app.clone();
    let mix = if project.audio_end_tick() > 0 {
        Some(export::render_mix(&project, &assets, export::EXPORT_SR, move |p| {
            let _ = app2.emit("export-progress", p * 0.5);
        })?)
    } else {
        None
    };
    let (l, r) = mix.unwrap_or_else(|| (vec![0.0; 4800], vec![0.0; 4800]));
    let secs = l.len() as f64 / export::EXPORT_SR as f64;
    let tmp = std::env::temp_dir().join(format!("music-maker-mix-{}.wav", std::process::id()));
    write_wav(&tmp, &l, &r, export::EXPORT_SR, 16)?;
    let _ = app.emit("export-progress", 0.6f32);
    let res = export::export_video(&project, &tmp, secs, &path);
    let _ = std::fs::remove_file(&tmp);
    res?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn audio_devices() -> serde_json::Value {
    let (outputs, inputs) = audio_io::list_devices();
    json!({ "outputs": outputs, "inputs": inputs, "midi": midi::ports(), "ffmpeg": export::ffmpeg_available() })
}

#[tauri::command]
pub fn audio_status(state: State<'_, AppState>) -> serde_json::Value {
    let g = lock(&state);
    status_json(&g)
}

fn status_json(g: &crate::state::Inner) -> serde_json::Value {
    let a = g.audio.as_ref();
    json!({
        "running": a.is_some(),
        "error": g.audio_error,
        "output": a.map(|a| a.output_name.clone()),
        "input": a.and_then(|a| a.input_name.clone()),
        "sampleRate": a.map(|a| a.sr),
        "inputRate": a.and_then(|a| a.input_rate),
        "latencyMs": a.map(|a| a.shared.latency_frames.load(std::sync::atomic::Ordering::Relaxed) as f64 / a.sr as f64 * 1000.0),
        "warnings": a.map(|a| a.warnings.clone()).unwrap_or_default(),
        "config": {
            "output": g.audio_req.output,
            "input": g.audio_req.input,
            "buffer": g.audio_req.buffer,
            "latencyOffsetMs": g.latency_offset_ms,
        },
        "midiPort": g.midi_port,
    })
}

#[tauri::command(async)]
pub fn set_audio_config(
    state: State<'_, AppState>,
    output: Option<String>,
    input: Option<String>,
    buffer: Option<u32>,
    latency_offset_ms: f32,
) -> R<serde_json::Value> {
    let mut g = lock(&state);
    g.audio_req.output = output;
    g.audio_req.input = input;
    g.audio_req.buffer = buffer;
    g.latency_offset_ms = latency_offset_ms;
    let res = g.open_audio();
    let s = status_json(&g);
    res.map(|_| s)
}

#[tauri::command]
pub fn midi_connect(app: AppHandle, state: State<'_, AppState>, port: Option<String>) -> R<()> {
    let mut g = lock(&state);
    g.midi = None;
    g.midi_port = None;
    let Some(name) = port else { return Ok(()) };
    let app2 = app.clone();
    let conn = midi::connect(&name, move |ev| {
        use tauri::Manager;
        let st = app2.state::<AppState>();
        let mut g = st.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(track) = g.selected_track else { return };
        match ev {
            midi::MidiEvent::NoteOn(p, v) => g.note_on(track, p, v),
            midi::MidiEvent::NoteOff(p) => g.note_off(track, p),
        }
        drop(g);
        let _ = app2.emit("midi-note", ());
    })?;
    g.midi = Some(conn);
    g.midi_port = Some(name);
    Ok(())
}

#[tauri::command]
pub fn is_dirty(state: State<'_, AppState>) -> bool {
    lock(&state).dirty
}
