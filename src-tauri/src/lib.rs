mod assets;
mod audio_io;
mod assistant;
mod cmds;
mod compose;
mod control;
mod edit;
mod engine;
mod export;
mod history;
mod listen;
mod midi;
mod model;
mod notation;
mod presets;
mod project_io;
mod state;

#[cfg(test)]
mod tests;

use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use engine::MASTER_METER;
use state::{AppState, Inner};

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct TransportEvent {
    /// The tick currently being heard (output latency compensated).
    tick: f64,
    playing: bool,
    recording: bool,
    meters: Vec<f32>,
    master: f32,
    input: f32,
    load: f32,
}

/// ~60 Hz: report transport/meters to the UI and free garbage from the audio thread.
fn spawn_poller(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("transport-poller".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(16));
            let st = app.state::<AppState>();
            let ev = {
                let mut g = st.inner.lock().unwrap_or_else(|e| e.into_inner());
                let tempo = g.project.tempo_map();
                let (ls, le, lon) = (g.project.loop_start as f64, g.project.loop_end as f64, g.project.loop_enabled);
                let recording = g.is_recording();
                match g.audio.as_mut() {
                    Some(a) => {
                        while a.garbage.pop().is_ok() {}
                        let s = &a.shared;
                        let playing = s.playing.load(Ordering::Relaxed);
                        let pos = s.position();
                        let mut tick = pos;
                        if playing {
                            let lat = s.latency_frames.load(Ordering::Relaxed) as f64 / a.sr as f64
                                * tempo.bpm_at(pos)
                                * model::PPQ as f64
                                / 60.0;
                            tick = pos - lat;
                            if lon && !recording && pos >= ls && tick < ls && le > ls {
                                tick = le - (ls - tick);
                            }
                            tick = tick.max(0.0);
                        }
                        let n = s.num_tracks.load(Ordering::Relaxed) as usize;
                        TransportEvent {
                            tick,
                            playing,
                            recording,
                            meters: (0..n).map(|i| s.meter(i)).collect(),
                            master: s.meter(MASTER_METER),
                            input: s.input_meter(),
                            load: s.load(),
                        }
                    }
                    None => TransportEvent::default(),
                }
            };
            let _ = app.emit("transport", ev);
        })
        .expect("spawn poller");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("duet"));
            let _ = std::fs::create_dir_all(&data_dir);
            presets::load_user_presets(&data_dir);
            if let Err(e) = assistant::setup(&data_dir) {
                eprintln!("assistant: {e}");
            }
            if let Err(e) = control::start(app.handle().clone(), &data_dir) {
                eprintln!("{e}");
            }
            let host = audio_io::spawn_host();
            let mut inner = Inner::new(app.handle().clone(), host, data_dir);
            if let Err(e) = inner.open_audio() {
                eprintln!("audio: {e}");
            }
            app.manage(AppState { inner: Mutex::new(inner) });
            spawn_poller(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let dirty = window.state::<AppState>().inner.lock().map(|g| g.dirty).unwrap_or(false);
                if dirty {
                    api.prevent_close();
                    let w = window.clone();
                    window
                        .dialog()
                        .message("You have unsaved changes. Quit without saving?")
                        .title("Duet")
                        .kind(MessageDialogKind::Warning)
                        .buttons(MessageDialogButtons::OkCancelCustom("Quit".into(), "Cancel".into()))
                        .show(move |quit| {
                            if quit {
                                let _ = w.destroy();
                            }
                        });
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            cmds::get_state,
            cmds::get_presets,
            cmds::apply_edit,
            cmds::undo,
            cmds::redo,
            cmds::transport,
            cmds::record_start,
            cmds::seek,
            cmds::set_metronome,
            cmds::note_on,
            cmds::note_off,
            cmds::all_notes_off,
            cmds::set_selected_track,
            cmds::import_audio,
            cmds::load_sample,
            cmds::pick_media,
            cmds::classify_files,
            cmds::add_media,
            cmds::extract_video_audio,
            cmds::get_peaks,
            cmds::new_project,
            cmds::open_project,
            cmds::save_project,
            cmds::export_audio,
            cmds::export_video,
            cmds::audio_devices,
            cmds::audio_status,
            cmds::set_audio_config,
            cmds::midi_connect,
            cmds::is_dirty,
            cmds::save_user_preset,
            cmds::delete_user_preset,
            cmds::set_ui_context,
            cmds::assistant_status,
            cmds::assistant_ask,
            cmds::assistant_cancel,
            cmds::assistant_reset,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Duet");
}
