//! Application state shared by the Tauri commands.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::Consumer;
use serde::Serialize;
use tauri::AppHandle;

use crate::assets::{AssetStore, AudioData};
use crate::audio_io::{AudioLink, HostCmd, OpenRequest};
use crate::edit::{self, Edit};
use crate::engine::compile::compile;
use crate::engine::EngineMsg;
use crate::history::History;
use crate::model::*;

pub struct AppState {
    pub inner: Mutex<Inner>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResponse {
    pub project: Project,
    pub new_id: Option<u64>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: bool,
    pub path: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy)]
struct RecMidi {
    tick: f64,
    pitch: u8,
    vel: f32,
    on: bool,
}

pub struct RecordSession {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<(Consumer<f32>, Vec<f32>)>>,
    midi: Vec<RecMidi>,
    audio_track: Option<u64>,
    midi_track: Option<u64>,
}

pub struct Inner {
    #[allow(dead_code)]
    pub app: AppHandle,
    pub project: Project,
    pub history: History,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub assets: AssetStore,
    pub audio: Option<AudioLink>,
    pub audio_error: Option<String>,
    pub host: mpsc::Sender<HostCmd>,
    pub audio_req: OpenRequest,
    pub latency_offset_ms: f32,
    pub metronome: bool,
    pub selected_track: Option<u64>,
    pub midi: Option<midir::MidiInputConnection<()>>,
    pub midi_port: Option<String>,
    pub rec: Option<RecordSession>,
    pub data_dir: PathBuf,
    /// What the user has selected in the UI (sent by the UI; shown to the assistant).
    pub ui_context: serde_json::Value,
}

impl Inner {
    pub fn new(app: AppHandle, host: mpsc::Sender<HostCmd>, data_dir: PathBuf) -> Self {
        Inner {
            app,
            project: crate::presets::template("empty"),
            history: History::default(),
            path: None,
            dirty: false,
            assets: AssetStore::default(),
            audio: None,
            audio_error: None,
            host,
            audio_req: OpenRequest::default(),
            latency_offset_ms: 0.0,
            metronome: false,
            selected_track: None,
            midi: None,
            midi_port: None,
            rec: None,
            data_dir,
            ui_context: serde_json::Value::Null,
        }
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    pub fn response(&self, new_id: Option<u64>) -> ProjectResponse {
        ProjectResponse {
            project: self.project.clone(),
            new_id,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            dirty: self.dirty,
            path: self.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            warnings: vec![],
        }
    }

    pub fn send(&mut self, m: EngineMsg) {
        if let Some(a) = self.audio.as_mut() {
            let _ = a.tx.push(m);
        }
    }

    /// Recompile the project and hand it to the audio thread.
    pub fn sync_engine(&mut self) {
        if let Some(a) = self.audio.as_mut() {
            let song = compile(&self.project, &self.assets, a.sr as f64);
            let _ = a.tx.push(EngineMsg::SetSong(Box::new(song)));
        }
    }

    pub fn apply(&mut self, e: Edit) -> Result<Option<u64>, String> {
        let key = edit::merge_key(&e);
        self.apply_with(key, |p| edit::apply(p, e))
    }

    /// Runs several mutations as a single undo step.
    pub fn apply_with<T>(&mut self, key: Option<String>, f: impl FnOnce(&mut Project) -> Result<T, String>) -> Result<T, String> {
        let before = self.project.clone();
        match f(&mut self.project) {
            Ok(v) => {
                self.history.record(before, key);
                self.dirty = true;
                self.sync_engine();
                Ok(v)
            }
            Err(e) => {
                self.project = before;
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        let ok = self.history.undo(&mut self.project);
        if ok {
            self.dirty = true;
            self.sync_engine();
        }
        ok
    }

    pub fn redo(&mut self) -> bool {
        let ok = self.history.redo(&mut self.project);
        if ok {
            self.dirty = true;
            self.sync_engine();
        }
        ok
    }

    /// Replace the whole project (new/open). Not undoable.
    pub fn replace_project(&mut self, p: Project, path: Option<PathBuf>) {
        self.stop_recording_quiet();
        self.send(EngineMsg::Stop);
        self.send(EngineMsg::Stop);
        self.send(EngineMsg::AllNotesOff);
        self.project = p;
        self.path = path;
        self.dirty = false;
        self.history.clear();
        self.sync_engine();
    }

    /// (Re)open the audio devices according to `audio_req`.
    pub fn open_audio(&mut self) -> Result<(), String> {
        self.stop_recording_quiet();
        let (tx, rx) = mpsc::channel();
        self.host.send(HostCmd::Open(self.audio_req.clone(), tx)).map_err(|_| "Audio thread stopped")?;
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(link)) => {
                self.audio = Some(link);
                self.audio_error = None;
                let m = self.metronome;
                self.send(EngineMsg::SetMetronome(m));
                self.sync_engine();
                Ok(())
            }
            Ok(Err(e)) => {
                self.audio = None;
                self.audio_error = Some(e.clone());
                Err(e)
            }
            Err(_) => {
                self.audio = None;
                let e = "Timed out opening the audio device".to_string();
                self.audio_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Opens the default input if an audio track is armed but no input is open.
    pub fn ensure_input_for_armed(&mut self) -> Result<(), String> {
        let wants_input = self.project.tracks.iter().any(|t| t.is_audio() && t.armed);
        let has_input = self.audio.as_ref().map(|a| a.input_rate.is_some()).unwrap_or(false);
        let input_off = self.audio_req.input.as_deref().map(|s| s == "none").unwrap_or(true);
        if wants_input && !has_input && input_off && self.rec.is_none() {
            self.audio_req.input = Some("default".into());
            self.open_audio()?;
        }
        Ok(())
    }

    fn latency_ticks(&self) -> f64 {
        match &self.audio {
            Some(a) => {
                let frames = a.shared.latency_frames.load(Ordering::Relaxed) as f64;
                let pos = a.shared.position();
                frames / a.sr as f64 * self.project.tempo_map().bpm_at(pos) * PPQ as f64 / 60.0
            }
            None => 0.0,
        }
    }

    pub fn note_on(&mut self, track_id: u64, pitch: u8, vel: f32) {
        self.send(EngineMsg::NoteOn { track_id, pitch, vel });
        self.record_midi(pitch, vel, true);
    }

    pub fn note_off(&mut self, track_id: u64, pitch: u8) {
        self.send(EngineMsg::NoteOff { track_id, pitch });
        self.record_midi(pitch, 0.0, false);
    }

    fn record_midi(&mut self, pitch: u8, vel: f32, on: bool) {
        let tick = match &self.audio {
            Some(a) => a.shared.position(),
            None => return,
        } - self.latency_ticks();
        if let Some(rec) = self.rec.as_mut() {
            if rec.midi_track.is_some() {
                rec.midi.push(RecMidi { tick, pitch, vel, on });
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.rec.is_some()
    }

    pub fn start_recording(&mut self) -> Result<(), String> {
        if self.rec.is_some() {
            return Ok(());
        }
        let audio_track = self.project.tracks.iter().find(|t| t.armed && t.is_audio()).map(|t| t.id);
        let midi_track = self.project.tracks.iter().find(|t| t.armed && t.is_instrument()).map(|t| t.id);
        if audio_track.is_none() && midi_track.is_none() {
            return Err("Arm a track first (the record button on a track header).".into());
        }
        if audio_track.is_some() {
            self.ensure_input_for_armed()?;
        }
        let link = self.audio.as_mut().ok_or("Audio output isn't running. Check Audio Settings.")?;
        let stop = Arc::new(AtomicBool::new(false));
        let mut handle = None;
        if audio_track.is_some() {
            let mut cons = link
                .record
                .take()
                .ok_or("No audio input is open. Pick an input device in Audio Settings.")?;
            while cons.pop().is_ok() {}
            let stop2 = stop.clone();
            handle = Some(std::thread::spawn(move || {
                let mut data = Vec::with_capacity(48000 * 2 * 30);
                loop {
                    let stopping = stop2.load(Ordering::Acquire);
                    while let Ok(s) = cons.pop() {
                        data.push(s);
                    }
                    if stopping {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                (cons, data)
            }));
        }
        let _ = link.tx.push(EngineMsg::Play { record: true });
        self.rec = Some(RecordSession { stop, handle, midi: vec![], audio_track, midi_track });
        Ok(())
    }

    fn stop_recording_quiet(&mut self) {
        if self.rec.is_some() {
            if let Err(e) = self.stop_recording() {
                eprintln!("recording stop error: {e}");
            }
        }
    }

    /// Stops transport and turns whatever was recorded into clips (one undo step).
    pub fn stop_recording(&mut self) -> Result<(), String> {
        let Some(mut rec) = self.rec.take() else { return Ok(()) };
        let Some(link) = self.audio.as_mut() else { return Err("Audio stopped while recording".into()) };
        let shared = link.shared.clone();
        let start_tick = shared.record_start();
        let stop_tick = shared.position();
        let _ = link.tx.push(EngineMsg::Stop);
        std::thread::sleep(Duration::from_millis(40));
        rec.stop.store(true, Ordering::Release);

        let mut take: Option<(u64, AudioData)> = None;
        if let Some(h) = rec.handle.take() {
            let (cons, data) = h.join().map_err(|_| "Recorder thread failed")?;
            link.record = Some(cons);
            let rate = link.input_rate.unwrap_or(link.sr);
            let out_lat = shared.latency_frames.load(Ordering::Relaxed) as f64 / link.sr as f64;
            let in_lat = shared.input_latency_frames.load(Ordering::Relaxed) as f64 / rate as f64;
            let comp_secs = out_lat + in_lat + self.latency_offset_ms as f64 / 1000.0;
            let skip = ((comp_secs.max(0.0) * rate as f64) as usize * 2).min(data.len());
            let frames = &data[skip..];
            if frames.len() >= 2 * (rate as usize / 20) {
                let mut l = Vec::with_capacity(frames.len() / 2);
                let mut r = Vec::with_capacity(frames.len() / 2);
                for f in frames.chunks_exact(2) {
                    l.push(f[0]);
                    r.push(f[1]);
                }
                take = Some((rec.audio_track.unwrap(), AudioData { sr: rate, l, r }));
            }
        }

        let bar = self.project.bar_ticks() as f64;
        let rec_dir = self.recordings_dir();
        let mut saved_take = None;
        if let Some((track, data)) = take {
            std::fs::create_dir_all(&rec_dir).map_err(|e| e.to_string())?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let path = rec_dir.join(format!("Take {stamp}.wav"));
            crate::assets::write_wav(&path, &data.l, &data.r, data.sr, 24)?;
            saved_take = Some((track, path, Arc::new(data)));
        }

        let midi_notes = rec.midi_track.map(|t| (t, midi_to_notes(&rec.midi, start_tick, stop_tick, bar)));

        if saved_take.is_none() && midi_notes.as_ref().map(|(_, n)| n.1.is_empty()).unwrap_or(true) {
            return Ok(());
        }
        let saved_take = saved_take.map(|(track, path, data)| {
            let id = self.project.alloc_id();
            self.assets.insert(id, data.clone());
            (track, path, data, id)
        });
        self.apply_with(None, |p| {
            if let Some((track, path, data, asset_id)) = saved_take {
                let n = p.assets.iter().filter(|a| a.name.starts_with("Take")).count() + 1;
                edit::apply(
                    p,
                    Edit::AddAsset {
                        asset: AssetRef {
                            id: asset_id,
                            name: format!("Take {n}"),
                            path: path.to_string_lossy().into_owned(),
                            kind: AssetKind::Audio,
                            duration: data.duration(),
                            sample_rate: data.sr,
                        },
                    },
                )?;
                edit::apply(
                    p,
                    Edit::AddClip {
                        track_id: track,
                        start: start_tick.max(0.0) as u64,
                        length: p.duration_ticks(start_tick.max(0.0) as u64, data.duration()),
                        offset: None,
                        content: ClipContent::Audio { asset_id },
                    },
                )?;
            }
            if let Some((track, (base, notes, len))) = midi_notes {
                if !notes.is_empty() {
                    let n = p.patterns.len() + 1;
                    let pid = edit::apply(
                        p,
                        Edit::AddPattern { name: Some(format!("Recorded {n}")), length: Some(len), notes: Some(notes), text: None },
                    )?
                    .unwrap();
                    edit::apply(
                        p,
                        Edit::AddClip {
                            track_id: track,
                            start: base,
                            length: len,
                            offset: None,
                            content: ClipContent::Pattern { pattern_id: pid },
                        },
                    )?;
                }
            }
            Ok(())
        })
    }
}

/// Pairs note-on/off events into notes relative to the bar where recording started.
fn midi_to_notes(events: &[RecMidi], start: f64, stop: f64, bar: f64) -> (u64, Vec<Note>, u64) {
    let base = (start.max(0.0) / bar).floor() * bar;
    let mut open: [Option<(f64, f32)>; 128] = [None; 128];
    let mut notes = vec![];
    let mut push = |pitch: u8, s: f64, e: f64, vel: f32| {
        let s = s.max(start);
        if e > s {
            notes.push(Note {
                id: 0,
                pitch,
                start: (s - base) as u64,
                length: ((e - s) as u64).max(4),
                velocity: vel,
            });
        }
    };
    for ev in events {
        let p = ev.pitch.min(127) as usize;
        if ev.on {
            if let Some((s, v)) = open[p].take() {
                push(ev.pitch, s, ev.tick, v);
            }
            open[p] = Some((ev.tick, ev.vel));
        } else if let Some((s, v)) = open[p].take() {
            push(ev.pitch, s, ev.tick, v);
        }
    }
    for (p, o) in open.iter().enumerate() {
        if let Some((s, v)) = o {
            push(p as u8, *s, stop, *v);
        }
    }
    let len = (((stop - base) / bar).ceil().max(1.0) * bar) as u64;
    (base as u64, notes, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_midi_becomes_bar_aligned_notes() {
        let bar = 384.0;
        let ev = |tick: f64, pitch: u8, on: bool| RecMidi { tick, pitch, vel: 0.8, on };
        // Recording started mid-bar 2; one note released, one still held at stop.
        let events = [ev(400.0, 60, true), ev(480.0, 60, false), ev(500.0, 64, true)];
        let (base, notes, len) = midi_to_notes(&events, 390.0, 700.0, bar);
        assert_eq!(base, 384);
        assert_eq!(len, 384);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].pitch, notes[0].start, notes[0].length), (60, 16, 80));
        assert_eq!((notes[1].pitch, notes[1].start, notes[1].length), (64, 116, 200));
    }
}
