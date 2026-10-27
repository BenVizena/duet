//! Offline mixdown (the same engine, run faster than real time) and video export via ffmpeg.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rtrb::RingBuffer;

use crate::assets::AssetStore;
use crate::engine::compile::compile;
use crate::engine::{Engine, EngineMsg, Shared};
use crate::model::{AssetKind, ClipContent, Project};

pub const EXPORT_SR: u32 = 48000;

/// Renders the whole song to stereo buffers, including effect tails.
pub fn render_mix(project: &Project, assets: &AssetStore, sr: u32, mut progress: impl FnMut(f32)) -> Result<(Vec<f32>, Vec<f32>), String> {
    let end = project.audio_end_tick();
    if end == 0 {
        return Err("Nothing to export: the song has no clips.".into());
    }
    let mut song = compile(project, assets, sr as f64);
    song.loop_enabled = false;
    let shared = Arc::new(Shared::new(sr));
    let (mut tx, rx) = RingBuffer::<EngineMsg>::new(8);
    let (gtx, _grx) = RingBuffer::new(8);
    let mut eng = Engine::new(sr as f64, shared, rx, gtx);
    let _ = tx.push(EngineMsg::SetSong(Box::new(song)));
    let _ = tx.push(EngineMsg::Play { record: false });

    let main_frames = (project.tempo_map().tick_to_secs(end as f64) * sr as f64) as usize;
    let max_frames = main_frames + sr as usize * 10;
    const CHUNK: usize = 1024;
    let mut buf = vec![0f32; CHUNK * 2];
    let mut l = Vec::with_capacity(main_frames + sr as usize * 2);
    let mut r = Vec::with_capacity(main_frames + sr as usize * 2);
    let mut last_loud = 0usize;
    while l.len() < max_frames {
        eng.process(&mut buf, 2);
        for f in buf.chunks_exact(2) {
            if f[0].abs() > 1e-4 || f[1].abs() > 1e-4 {
                last_loud = l.len();
            }
            l.push(f[0]);
            r.push(f[1]);
        }
        if l.len() > main_frames && l.len() - last_loud.max(main_frames) > sr as usize / 2 {
            break;
        }
        if l.len() % (CHUNK * 64) == 0 {
            progress((l.len() as f32 / main_frames.max(1) as f32).min(1.0));
        }
    }
    let keep = (last_loud + sr as usize / 10).max(main_frames).min(l.len());
    l.truncate(keep);
    r.truncate(keep);
    // Short fade at the very end.
    let fade = (sr as usize / 50).min(l.len());
    let n = l.len();
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        l[n - 1 - i] *= g;
        r[n - 1 - i] *= g;
    }
    Ok((l, r))
}

pub fn ffmpeg_available() -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-version");
    hide_window(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn ffmpeg_output(args: &[String]) -> Result<std::process::Output, String> {
    if !ffmpeg_available() {
        return Err("ffmpeg wasn't found. Install it (e.g. `winget install ffmpeg`) and restart Duet.".into());
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(args);
    hide_window(&mut cmd);
    let out = cmd.output().map_err(|e| format!("Couldn't run ffmpeg: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(4).collect();
        return Err(format!("ffmpeg failed: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    Ok(out)
}

/// Runs ffmpeg and returns stdout (e.g. a piped PNG).
pub fn run_ffmpeg(args: &[String]) -> Result<Vec<u8>, String> {
    Ok(ffmpeg_output(args)?.stdout)
}

/// Runs ffmpeg and returns its log (stderr), e.g. for showinfo output.
pub fn run_ffmpeg_stderr(args: &[String]) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&ffmpeg_output(args)?.stderr).into_owned())
}

fn fmt_secs(s: f64) -> String {
    format!("{:.3}", s.max(0.0))
}

/// Builds the video from the media tracks (lower tracks underneath) with the mixdown as audio.
pub fn export_video(project: &Project, mix_wav: &Path, audio_secs: f64, out: &Path) -> Result<(), String> {
    if !ffmpeg_available() {
        return Err("ffmpeg wasn't found. Install it (e.g. `winget install ffmpeg`) and restart Duet.".into());
    }
    let tempo = project.tempo_map();
    struct Item {
        path: String,
        image: bool,
        start: f64,
        len: f64,
        offset: f64,
    }
    let mut items = vec![];
    for t in project.tracks.iter().rev().filter(|t| t.is_media()) {
        let mut clips: Vec<_> = t.clips.iter().collect();
        clips.sort_by_key(|c| c.start);
        for c in clips {
            if let ClipContent::Media { asset_id } = c.content {
                if let Some(a) = project.asset(asset_id) {
                    items.push(Item {
                        path: a.path.clone(),
                        image: a.kind == AssetKind::Image,
                        start: tempo.tick_to_secs(c.start as f64),
                        len: tempo.tick_to_secs((c.start + c.length) as f64) - tempo.tick_to_secs(c.start as f64),
                        offset: c.offset,
                    });
                }
            }
        }
    }
    if items.is_empty() {
        return Err("There are no video or image clips to export.".into());
    }
    let dur = items.iter().map(|i| i.start + i.len).fold(audio_secs, f64::max);

    let mut args: Vec<String> = vec!["-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into()];
    args.extend(["-f", "lavfi", "-i"].map(String::from));
    args.push(format!("color=c=black:s=1280x720:r=30:d={}", fmt_secs(dur)));
    for it in &items {
        if it.image {
            args.extend(["-loop", "1", "-framerate", "30", "-t"].map(String::from));
            args.push(fmt_secs(it.len));
        } else {
            args.push("-ss".into());
            args.push(fmt_secs(it.offset));
            args.push("-t".into());
            args.push(fmt_secs(it.len));
        }
        args.push("-i".into());
        args.push(it.path.clone());
    }
    args.push("-i".into());
    args.push(mix_wav.to_string_lossy().into_owned());
    let audio_index = items.len() + 1;

    let mut filter = String::new();
    let mut prev = "0:v".to_string();
    for (i, it) in items.iter().enumerate() {
        let k = i + 1;
        filter.push_str(&format!(
            "[{k}:v]scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30,setpts=PTS-STARTPTS+{}/TB[v{k}];",
            fmt_secs(it.start)
        ));
        filter.push_str(&format!(
            "[{prev}][v{k}]overlay=eof_action=pass:enable='between(t,{},{})'[o{k}];",
            fmt_secs(it.start),
            fmt_secs(it.start + it.len)
        ));
        prev = format!("o{k}");
    }
    filter.pop();
    args.push("-filter_complex".into());
    args.push(filter);
    args.extend(["-map".to_string(), format!("[{prev}]"), "-map".to_string(), format!("{audio_index}:a")]);
    args.extend(
        ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-t"]
            .map(String::from),
    );
    args.push(fmt_secs(dur));
    args.push(out.to_string_lossy().into_owned());

    let mut cmd = Command::new("ffmpeg");
    cmd.args(&args);
    hide_window(&mut cmd);
    let output = cmd.output().map_err(|e| format!("Couldn't run ffmpeg: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = err.lines().rev().take(6).collect();
        return Err(format!("ffmpeg failed:\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
    }
    Ok(())
}
