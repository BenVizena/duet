# Duet

A personal DAW for Windows (Rust audio engine + Tauri v2 + plain JS UI). It covers sequencing, built-in synths, sampling, mixing, effects and recording. You can also put **videos and images on the timeline** to score music to picture, but that's entirely optional.

## Run it

```powershell
npm install            # once: installs the Tauri CLI
npm run dev            # run with live reload of the UI (src/)
npm run build          # release build + installer in src-tauri/target/release/bundle/nsis/
```

You need the Rust toolchain (`%USERPROFILE%\.cargo\bin` on PATH), the VS 2022 C++ build tools and WebView2. Video export also needs `ffmpeg` on PATH.

Engine tests (offline renders of every template, preset and effect):

```powershell
cd src-tauri; cargo test --release
```

## Quick tour

- **New** → pick a template: *Chip route theme* (Game Boy), *Tactics battle* (GBA orchestral), *Neon drive* (synthwave) or *Empty*. Press **Space**.
- **Instrument presets** (left panel): click one to put it on the selected track, or **+** to add it as a new track. They're grouped as Chip, Orchestral, Synthwave and Keys, plus drum kits (chip / acoustic-ish / 80s machine) and a sampler.
- **Playlist**: click an instrument lane to place the selected pattern. Drag clips to move them, drag their edges to resize or trim, Ctrl-drag to copy, right-click for more. On the ruler, click to seek and drag the top strip to set the loop.
- **Piano roll / Step sequencer**: edit the selected pattern. The piano roll highlights the project's key/scale and can stamp in-key chords.
- **Instrument tab**: the synth has pulse (12.5/25/50/75%), triangle, saw, sine+FM, a drawable GB-style wavetable and LFSR noise. It also has unison, filter + envelope, delayed vibrato, pitch sweep, glide, an arpeggiator (free or tempo-synced), an LFO (cutoff / pulse width / volume) and bit depth.
- **Mixer**: volume/pan/mute/solo per track, plus EQ, filter, delay, reverb, compressor, chorus, bitcrusher and *Pump* (tempo-synced sidechain-style ducking). The master bus has a safety limiter.
- **Recording**: arm a track (●) and press **Ctrl+R**. On audio tracks this records the mic/line input (🎧 = monitor); the default input opens automatically when you arm. On instrument tracks it records what you play on the typing keyboard (Z–M / Q–U rows, `[` `]` for octave) or a MIDI keyboard (pick it in **Audio** settings). Recordings are latency-compensated automatically; fine-tune with *Recording offset* if needed.
- **Automation**: click **A** on a track header (or the Master row) and pick a target: volume, pan, synth filter cutoff, pitch bend, or an effect's mix/cutoff. Click in a lane to add points, drag them, right-click to delete. Values are linear between points.
- **Tempo changes**: the **Tempo** button above the track headers shows the tempo lane. Click to add a tempo point, drag it up/down, double-click to type a BPM or add `ramp` for a smooth glide. Audio clips, video sync, export and recording all follow the tempo map.
- **Your own presets**: **Save preset…** in the Instrument tab. Presets live in `%APPDATA%\com.benvi.musicmaker\user_presets.json` and show up under *My presets* (right-click to delete).
- **Video / images (optional)**: **+ Video/Image** or drop files on the playlist. The preview panel follows the playhead. Right-click a video clip to pull its audio onto its own track. **Export → video** renders an MP4 with your mix (needs ffmpeg).
- **Save** writes a `.mmproj` file. Recordings are copied into a `<project> Audio` folder next to it, and media paths are stored relative to the project when possible.

Press **?** in the app for all shortcuts.

## AI: the in-app assistant and Claude Code

**In the app:** open **✨ Ask AI** (or Ctrl+K) and type requests like *"how do I add automation?"*, *"clean up the spacing a little"* or *"put in a dark cyberpunk chord progression"*.
- **How it works:** the panel runs Claude Code headlessly (`claude -p`) with your existing Claude Code login, so there's no API key to set up. It sees your selection (track, pattern, selected notes, playhead), edits the song live, and the conversation continues until you press *New chat*.
- **Undo:** each change is one undo step.
- **Cost:** it uses whatever login Claude Code has. With a Pro/Max subscription, requests count toward your plan's usage limits, with no extra charge unless you've turned on extra usage. With an API key (`ANTHROPIC_API_KEY` or a Console login), they're billed as API usage. The dollar figure under each answer is Claude Code's API-equivalent estimate.

**From Claude Code in this folder:** `.mcp.json` registers the `duet` MCP server. Approve it the first time Claude Code asks, then ask for music in plain language. The `compose-music` skill in `.claude/skills/` teaches Claude the workflow, the note notation and genre recipes. The server starts Duet if it isn't running. To use it from any folder:

```powershell
claude mcp add --scope user duet -- node <path-to-repo>\tools\mcp\duet-mcp.mjs
```

**Tools:**

| Tool | What it does |
|---|---|
| `status` | Playhead and the user's selection |
| `get_project` | Whole song summary, with notes in text notation |
| `get_pattern` / `get_instrument` | Read one pattern or one track's instrument |
| `list_presets` | Instrument presets, effect defaults, automation targets |
| `compose` | Declarative song spec: tracks, patterns, clips, effects, automation, tempo |
| `edit` | Any low-level edit, with `$N` references between edits |
| `undo` / `redo` | Undo or redo |
| `transport` | Play or stop |
| `render_check` | Offline render with loudness per bar and per track, plus an optional WAV |
| `save_preset` | Save a track's instrument as a user preset |
| `notify` | Show a message in the app |

**For the optional video/images:**

| Tool | What it does |
|---|---|
| `media_info` | Where each video/image clip sits on the timeline |
| `media_frame` | The picture at a time or at the playhead |
| `media_overview` | A contact sheet of the whole video |
| `detect_scenes` | Cut times, mapped to song seconds and bars |

These let Claude match the music to what's on screen and land changes on cuts.

**Under the hood:** the app exposes a token-protected HTTP API on `127.0.0.1`. The port and token are in `%APPDATA%\com.benvi.musicmaker\control.json`. `tools/mcp/duet-mcp.mjs` is a dependency-free bridge from MCP to that API.

## Layout

```
src/                 UI (no bundler; ES modules, window.__TAURI__ globals)
  js/store.js        project state, serialized edit queue, undo/redo calls
  js/timeline.js     playlist, track headers, ruler
  js/pianoroll.js    piano roll        js/stepseq.js    step sequencer
  js/instrument.js   instrument editor js/mixer.js      mixer + effects
  js/media.js        optional video/image preview, sync, thumbnails
src-tauri/src/
  model.rs           serializable project model
  edit.rs            command layer (every change is an Edit)
  history.rs         snapshot undo/redo with merging for fader drags
  engine/            real-time engine: synth voices, effects, compiler
  audio_io.rs        cpal output/input on a dedicated thread
  state.rs           app state, recording
  export.rs          offline mixdown + ffmpeg video export
  presets.rs         instrument presets, user presets, starter templates
  notation.rs        compact note text notation ("C4:4 r:2 E4+G4:8")
  compose.rs         declarative song specs (used by AI tools)
  control.rs         local control API for MCP / scripts
  assistant.rs       in-app assistant (runs Claude Code headlessly)
tools/mcp/           MCP server for Claude Code
tools/assistant/     instructions + app guide for the in-app assistant
.claude/skills/      compose-music skill
```
