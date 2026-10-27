---
name: compose-music
description: Compose, arrange, edit or mix music in the Duet DAW through its MCP tools (mcp__duet__*). Use for requests like "write a battle theme", "add a dark cyberpunk chord progression", "clean up the timing", "make the bass punchier", "score this video", or anything that should change the song in the running app. Also covers scoring to the project's video/images.
---

# Composing in Duet

You change the song in the running Duet app through the `duet` MCP tools. Every change is one undo step, so the user can press Ctrl+Z. You can't hear audio; `render_check` is how you check your work.

## Workflow

1. `status` shows what the user has selected (track, pattern, selected note ids, playhead). "This", "here", "these notes" and "the selection" refer to it.
2. `get_project` shows tempo, key, tracks, effects, automation, clips (in bars) and every pattern's notes in notation.
3. Make the change:
   - Whole new song or new sections/tracks: `compose` (declarative, one call).
   - Targeted changes: `edit` (setNotesText, setInstrument, updateTrack, addEffect, automation, tempo...).
4. `render_check` catches silence, clipping and a lopsided mix (per-track levels). Fix what it reports.
5. Optionally `transport {action:"play", bar}` so the user hears it. Reply with one or two sentences: what you changed and where (track, bars).

Keep the user's existing material unless they ask to replace it. With `compose`, use `"replace": false` to add tracks to a song. `replace: true` (the default) rewrites all non-video tracks.

## Note notation

- `NOTE:LEN`: LEN is in 16th-note steps (4 = quarter, 16 = one 4/4 bar, 1.333 = 16th triplet). `C4` is middle C. Sharps are `F#3`, flats are `Bb2`.
- `r:LEN` is a rest. `@0.6` sets velocity (default 0.8). `C4+E4+G4:8` is a chord. A missing `:LEN` reuses the last length.
- Each line (or `;`-separated part) is an independent voice starting at step 0. `|` bar marks are ignored but help readability.
- Drum grid lines put one character per 16th: `kick: x...x...x...x...` where `x` is a hit, `X` an accent, `o` a soft hit, `.` a rest.
- Drum keys: kick snare clap hat (closed) ohat (open) rim tom1/tom2/tom3 crash ride.

Make sure each pattern's notes add up to its `bars` (16 steps per 4/4 bar). The tools warn when they overflow.

## Song spec (for `compose`)

```json
{"bpm": 100, "key": "A", "scale": "minor", "timeSig": "4/4", "loopBars": [1, 17],
 "tempoChanges": [{"bar": 13, "bpm": 84, "ramp": true}],
 "tracks": [
  {"name": "Pad", "preset": "Neon Pad", "volume": 0.6,
   "effects": [{"type": "chorus"}, {"type": "reverb", "size": 0.8, "mix": 0.35}, {"type": "pump", "depth": 0.5}],
   "patterns": {"prog": {"bars": 4, "notes": "A3+C4+E4:16 | F3+A3+C4:16 | C4+E4+G4:16 | G3+B3+D4:16"}},
   "clips": [{"pattern": "prog", "bar": 1, "repeat": 4}],
   "automation": [{"target": "fx:1:mix", "points": [[1, 0.1], [9, 0.5]]}]},
  {"name": "Drums", "preset": "Electro Drum Machine",
   "patterns": {"beat": {"bars": 1, "notes": "kick: x...x...x...x...\nsnare: ....X.......X...\nhat: ..x...x...x...x."}},
   "clips": [{"pattern": "beat", "bar": 5, "bars": 12}]}],
 "master": {"effects": [{"type": "compressor", "threshold": -12, "ratio": 2.5}],
            "automation": [{"target": "volume", "points": [[16, 0.9], [17, 0]]}]}}
```

- Bars are 1-based. A clip with `bars` longer than its pattern loops the pattern.
- `preset` takes any name from `list_presets`. `instrument` is merged over the preset, for example `{"cutoff": 2000, "arp": [0, 3, 7]}`.
- Effects are `eq filter delay reverb compressor chorus bitcrusher pump`. Params you leave out use defaults. Delay `time` is in beats.
- Automation targets are `volume` (0–1.5), `pan` (-1..1), `cutoff` (synth filter offset in octaves; the synth needs `filterOn`), `pitch` (semitones), `fx:<i>:mix` (0–1) and `fx:<i>:cutoff` (Hz, Filter effect). `<i>` is the effect's index in that track's `effects` list. The value is linear between points and holds outside them.

## Common requests

- **"Clean up the spacing/timing"**: quantize note starts to the grid (16ths usually, or 8ths if the part is sparse). Fix accidental overlaps and tiny gaps on legato lines, and make repeated notes even. Keep the rhythm the user played; don't rewrite the melody. Apply it to the selected notes, or the selected pattern if nothing's selected, with `setNotes` (keep ids) or `setNotesText`.
- **"Humanize"**: give velocities small variations (±0.08) and keep the timing tight.
- **"Add a chord progression with feeling X"**: write a new pattern on a new (or the selected) pad/keys track in the project's key, placed where asked (default: under the selection or the loop). Add a bass track if there isn't one.
- **"More energy" / "build-up"**: denser drums (16th hats, fills on the last beat), rising filter cutoff automation, raise the arp octave, crash on the downbeat.
- **Mix fixes** ("bass too loud", "muddy"): adjust `updateTrack volume`/`pan` or EQ, then confirm with `render_check` per-track levels.

## Style recipes (starting points, not limits)

**Game Boy / Pokémon-like:** 2 pulse leads (12.5% and 25%) with delayed vibrato, a Wave Bass bouncing between root and octave in 8ths, Chip Drum Kit (kick 1 & 3, snare 2 & 4, 8th hats). Major keys with bright I–IV–V–vi, or mixolydian for adventure. 120–150 bpm. Melodies are singable with stepwise motion and leaps to chord tones; harmony runs in thirds/sixths or as arpeggios. A little ping-pong delay goes on the harmony.

**GBA tactics / Fire Emblem-like:** Brass Section melody, Strings Ensemble pads, Harp/Pizzicato 16th arpeggios, Timpani hits, Orchestra Drums. Minor or harmonic minor, i–VI–VII–V or i–iv–V. 130–170 bpm for battle and 70–100 for map themes. The melody uses dotted rhythms, and the V chord with the raised 7th (G# in A minor) drives cadences.

**Synthwave / neon / outrun:** Neon Pad or PWM Strings chords (sus2, add9, maj7 color), an Outrun Bass of driving 8ths on the root, a Synced Arp or Neon Arp Pluck with 3/16 ping-pong delay, Supersaw Lead, and an Electro Drum Machine (four-on-the-floor or a half-time backbeat, big reverb on the snare). Pump on the pads and bass. 85–118 bpm. Useful progressions are i–VI–III–VII and i–iv–VI–V.

**Dark cyberpunk:** Phrygian or harmonic minor, minor chords a half step apart (i–II, i–bVI–bII), low detuned saw bass with a slowly opening filter (cutoff automation), bitcrusher on percussion, sparse plucks with long delays, and tension from tritones and sus chords. 90–110 bpm, or half-time 140–160.

**Lo-fi / chill:** FM Electric Piano 7th and 9th chords (ii–V–I, iv–i), swing feel (delay every second 8th by about one 16th triplet), soft drums, chorus, a lowpass filter, and velocity variation.

General craft:
- Keep everything in the project's key unless you're adding chromatic color on purpose. Voice-lead chords (move each note as little as possible).
- Leave the melody register (C5–C6) to one instrument at a time. Bass sits at C2–C3 and pads at C3–C5.
- Arrange in 4- or 8-bar phrases: intro, A, B, A', and add a change (drop, fill or key lift) every 8 or 16 bars.
- Keep the master limiter quiet: track volumes around 0.5–0.8 and master 0.7–0.9.

## Scoring to the video/images (optional)

When the project has media (`status.hasMedia`):

1. `media_info` shows where the clips sit. `media_overview` gives a contact sheet of the whole video so you can judge mood, color, pace and setting. `media_frame` looks at specific moments, such as the playhead.
2. `detect_scenes` gives the cut times in song seconds and bars.
3. Choose a tempo so the important cuts land on bar or beat lines (`bar = 1 + secs × bpm / 240` in 4/4). Use `tempoChanges` to line up later cuts, or start sections on the bars nearest the cuts.
4. Match the mood. Dark, desaturated or night scenes get minor/phrygian, low pads and slow tempo. Bright daytime scenes get major/lydian and lighter instruments. Action gets a faster tempo and driving drums. Neon or city-at-night scenes get synthwave. Change instrumentation or energy at each cut, and place hits (crash + chord) on big cuts.
5. Tell the user which cuts you scored to.

## Other tools

`get_pattern` returns one pattern with note ids. `get_instrument` returns a track's full synth params so you can tweak and send them back with `setInstrument`. `save_preset` saves an instrument as a user preset. `notify` shows a message in the app. `undo` and `redo` are also available.
