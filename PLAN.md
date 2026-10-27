# Music Maker — Project Plan

A personal pattern-based digital audio workstation (DAW). It is a complete music-making tool on its own. As an optional extra, **images and videos can be placed on the timeline**, so music can be synced to video or matched to the mood of an image.

## Goals

- Full-featured music creation: sequencing, synths, samples, mixing, effects, recording.
- Works fully without any images or videos. Media is optional: projects with no media track behave like a normal DAW.
- Optional video and image clips on the timeline, previewed in sync with playback.
- Microphone/line input recording.
- Built for one user (me) on Windows; no need for plugin marketplaces, collaboration, or cloud features.

## Tech stack

| Layer | Choice | Why |
|---|---|---|
| Audio engine | **Rust** (`cpal` for audio I/O, `symphonia` for decoding, `hound` for writing WAV) | Low-latency, glitch-free real-time audio; no garbage-collection pauses |
| Engine messaging | `rtrb` (lock-free ring buffer) | Send commands to the audio thread without locks or allocations |
| Project model | `serde` (+ JSON or similar) | Project state is plain serializable data from day one |
| App shell | **Tauri v2** | Native window, Rust backend, small footprint |
| UI | Web tech (HTML/CSS/JS) inside Tauri's WebView2; timeline, piano roll and waveforms drawn on `<canvas>` | Fast to build; canvas stays fast with many clips/notes, unlike DOM elements |
| Video/image preview (optional) | WebView `<video>` / `<img>`, synced to the Rust transport position | Avoids FFmpeg for playback |
| Video export (optional) | `ffmpeg.exe` run as a separate process | Only needed for exporting video; no FFmpeg libraries to link |
| MIDI input | `midir` | Play/record from a MIDI keyboard |

## Architecture principles

These apply from Phase 1 because they're hard to add later.

- **Single source of truth for time:** the Rust engine owns the playback position. The UI follows it.
- **Serializable project model:** tracks, clips, notes, patterns, mixer and effect settings live in one `serde`-serializable data model, not scattered across the audio thread and JS objects. Save/load just serializes it.
- **Command layer for edits:** every change to the project goes through a command/action (e.g. `AddClip`, `MoveNote`, `SetVolume`). Undo/redo is built on top of these.
- **Audio graph from the start:** track → (effects slot) → volume/pan → master bus. Synths, samplers, effects and recording plug into this structure later.
- **Real-time safety:** no allocations, locks, file I/O or blocking in the audio callback. The UI/main thread talks to the audio thread through lock-free queues (`rtrb`). Big buffers (samples, recordings) are loaded/allocated off the audio thread and handed over.
- **Efficient IPC:** the UI gets transport position at ~30–60 Hz with a timestamp and smooths it with `requestAnimationFrame`. Bulk data (waveform peaks, thumbnails) goes over Tauri's binary IPC/channels, not JSON. Waveform peaks are computed in Rust.
- **Media is optional:** nothing in the engine, model or UI assumes a media track exists. The preview panel only shows when the project has media.

## Environment (already set up)

- Rust stable (cargo 1.99.1), MSVC target
- Visual Studio 2022 C++ build tools
- WebView2 runtime
- Node 22 / npm 11

## Phases

Each phase should run before the next one starts.

1. **Base app & foundations**
   - Tauri window with the Rust audio engine (cpal output via WASAPI).
   - Serializable project model + command layer (undo/redo can be a basic stack at this stage).
   - Audio graph skeleton: tracks → volume/pan → master.
   - Lock-free UI → audio-thread messaging.
   - Transport: play/stop/loop/tempo/metronome, with output-latency-aware position reporting to the UI.
   - Canvas timeline with audio tracks; import an audio file (WAV/MP3/FLAC via `symphonia`) onto a track, with waveform peaks from Rust.
   - Basic save/load of the project model (the full format can firm up in Phase 6).

2. **Composition tools**
   - Step sequencer, piano roll, built-in synth, sampler (WAV/MP3/FLAC).
   - MIDI keyboard input (`midir`) for playing and recording notes.

3. **Mic recording**
   - Choose an input device, arm a track, record onto the timeline, input monitoring.
   - Latency compensation: shift recorded audio back by the measured/reported round-trip latency so it lines up with playback.
   - Consider WASAPI exclusive mode or cpal's ASIO feature (needs the ASIO SDK at build time) for lower monitoring latency.

4. **Mixer & effects**
   - Mixer view: per-track volume/pan/mute/solo (built on the Phase 1 graph).
   - Effects: EQ/filter, delay, reverb, compressor.

5. **Video & image support (optional feature)**
   - Add a media track; drop videos/images onto it. Projects without one are unaffected.
   - Preview panel locked to the playhead, scrubbing, filmstrip thumbnails on the timeline (taken from a hidden `<video>` drawn to canvas).
   - Sync strategy: let `<video>` play on its own and correct only when drift is over ~1 frame (small `playbackRate` nudge, or a hard seek for big jumps/scrubs). Don't set `currentTime` on every tick.
   - Video audio: muted in the `<video>` element; if wanted in the mix, decode it in the Rust engine (`symphonia` reads MP4/AAC) as a normal audio clip.
   - Supported formats: WebView2 plays H.264/AAC MP4 and VP8/VP9 WebM. Show a clear error for unsupported files (e.g. HEVC, ProRes, some `.mov`), with optional ffmpeg transcoding later.

6. **Save & export**
   - Finalize the project file format (versioned, with relative media/sample paths).
   - Export mixdown to WAV.
   - If the project has video: export a video with the mixdown as its audio, using `ffmpeg.exe`.

## Status

- [x] Toolchain installed
- [x] Phase 1: base app, engine, model + command layer, undo/redo, audio graph, transport, canvas timeline, audio import
- [x] Phase 2: piano roll (in-key chord stamping, quantize), step sequencer, synth (chip, unison, FM, LFO, synced arp), sampler, 3 drum kits, MIDI + typing keyboard
- [x] Phase 3: input device choice, arm/record/monitor, automatic latency compensation + manual offset, MIDI recording
- [x] Phase 4: mixer, EQ/filter/delay/reverb/compressor, plus chorus, bitcrusher and tempo-synced pump
- [x] Phase 5: optional media track, synced preview, thumbnails, extract video audio
- [x] Phase 6: `.mmproj` save/load (relative paths, recordings copied next to the project), WAV export, MP4 export via ffmpeg
- Genre presets and templates: chip (GB), GBA orchestral, synthwave. Instruments stay general-purpose.

- [x] Automation lanes (track + master: volume, pan, synth cutoff, pitch bend, effect mix / filter cutoff)
- [x] Tempo map with step changes and ramps (engine, audio clips, video sync, export, recording all follow it)
- [x] User instrument presets
- [x] AI hooks: local control API, MCP server (`.mcp.json`), `compose-music` skill, in-app Assistant panel (Claude Code headless) with selection context, and video/image tools (frames, contact sheet, scene cuts)

### Not yet done / ideas
- ASIO / WASAPI exclusive mode (cpal `asio` feature is wired as a cargo feature but needs the ASIO SDK)
- Automation recording (moving a fader while playing writes points)
- Verified only with synthetic input: the mic path recorded silence on this machine, and typing-keyboard MIDI recording is covered by a unit test rather than a live session
