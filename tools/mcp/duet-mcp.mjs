#!/usr/bin/env node
// Duet MCP server (stdio, no dependencies).
// Exposes the running app's control API to Claude Code as tools. If the app
// isn't running it is started automatically.
//
// Config: DUET_CONTROL = path to control.json (default: %APPDATA%\com.benvi.musicmaker\control.json)
//         DUET_EXE     = path to duet.exe (optional; otherwise searched for)

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const APPDATA = process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming');
const CONTROL = process.env.DUET_CONTROL || path.join(APPDATA, 'com.benvi.musicmaker', 'control.json');

const SONG_SPEC_HELP = `Song spec (JSON):
{ "name"?, "bpm"?, "timeSig"?: "4/4", "key"?: "A", "scale"?: "minor",
  "tempoChanges"?: [{"bar": 17, "bpm": 90, "ramp"?: true}],
  "replace"?: true (default: replace all non-video tracks/patterns; false = add tracks),
  "loopBars"?: [1, 9],
  "tracks": [{ "name", "preset"?: "<preset name>", "instrument"?: {partial synth params},
      "volume"?, "pan"?, "mute"?, "effects"?: [{"type":"reverb","mix":0.3}, ...],
      "patterns": {"A": {"bars"?: 4, "notes": "<notation>"}},
      "clips": [{"pattern": "A", "bar": 1, "bars"?: 4, "repeat"?: 2}],
      "automation"?: [{"target": "volume"|"pan"|"cutoff"|"pitch"|"fx:<effectIndex>:mix"|"fx:<effectIndex>:cutoff", "points": [[bar, value], ...]}] }],
  "master"?: {"volume"?, "effects"?: [...], "automation"?: [...]} }
Notation: NOTE:LEN with LEN in 16th steps (C4 = middle C; F#3, Bb2), r:LEN rest, @VEL (0-1), chords C4+E4+G4:8,
each line or ';' part is a separate voice from step 0. Drum grid lines: "kick: x...x...x...x..." (x hit, X accent, o soft, . rest).
Drum names: kick snare clap hat ohat rim tom1 tom2 tom3 crash ride.`;

const TOOLS = [
  {
    name: 'status',
    description: 'Duet status: project name, playhead, whether playing, and what the user has selected in the UI (track, pattern, notes). Call this first to see what "this", "here" or "the selection" refers to.',
    inputSchema: { type: 'object', properties: {} },
    method: 'status',
  },
  {
    name: 'get_project',
    description: 'Summary of the current song: tempo/key/time signature, tracks (ids, instrument type, volume, pan, effects, automation, clips in bars) and every pattern\'s notes in text notation.',
    inputSchema: { type: 'object', properties: {} },
    method: 'get_project',
  },
  {
    name: 'get_pattern',
    description: 'One pattern\'s notes, both as notation text and as raw notes with ids/ticks (96 ticks per beat, 24 per 16th).',
    inputSchema: { type: 'object', properties: { pattern: { type: ['number', 'string'], description: 'Pattern id or name' } }, required: ['pattern'] },
    method: 'get_pattern',
  },
  {
    name: 'get_instrument',
    description: 'Full instrument parameters of a track (synth/sampler/drums), to tweak and send back with an edit {"type":"setInstrument"}.',
    inputSchema: { type: 'object', properties: { track: { type: ['number', 'string'], description: 'Track id or name' } }, required: ['track'] },
    method: 'get_instrument',
  },
  {
    name: 'list_presets',
    description: 'Instrument presets (built-in and the user\'s own), effect types with default params, automation targets and drum key names.',
    inputSchema: { type: 'object', properties: {} },
    method: 'list_presets',
  },
  {
    name: 'compose',
    description: `Write a whole song (or add tracks) from a declarative spec in one undoable step. Video/image tracks are always kept.\n${SONG_SPEC_HELP}`,
    inputSchema: {
      type: 'object',
      properties: {
        song: { type: 'object', description: 'The song spec described above' },
        message: { type: 'string', description: 'Short note shown to the user in the app, e.g. "Added a dark synthwave intro"' },
      },
      required: ['song'],
    },
    method: 'compose',
  },
  {
    name: 'edit',
    description: `Apply low-level edits (one undo step). Each edit is {"type": ..., ...camelCase fields}. Use "$N" as a value to refer to the id created by edit N in the same list.
Types: setBpm{bpm} setTimeSig{num,den} setKeyScale{root 0-11,scale} setLoop{start,end,enabled} setTempo{points:[{tick,bpm,ramp}]}
addTrack{kind:"instrument"|"audio",name?,instrument?} removeTrack{trackId} moveTrack{trackId,index}
updateTrack{trackId,name?,volume?,pan?,mute?,solo?} setInstrument{trackId,instrument}
addPattern{name?,length?(ticks),text?(notation)} updatePattern{patternId,name?,length?} removePattern{patternId} clonePattern{patternId}
setNotesText{patternId,text} setNotes{patternId,notes:[{pitch,start,length,velocity}]} (ticks: 96/beat)
addClip{trackId,start,length,offset?,content:{type:"pattern",patternId}} updateClip{trackId,clipId,start?,length?,offset?,gain?,toTrackId?} removeClip{trackId,clipId}
addEffect{trackId|null for master,effect:"reverb"...} updateEffect{trackId,effectId,kind?:{type,...all params},enabled?} removeEffect{trackId,effectId}
addAutomationLane{trackId|null,target,points?:[{tick,value}]} setAutomation{trackId|null,laneId,points?,enabled?} removeAutomationLane{trackId|null,laneId}
setMasterVolume{volume}`,
    inputSchema: {
      type: 'object',
      properties: {
        edits: { type: 'array', items: { type: 'object' } },
        message: { type: 'string', description: 'Short note shown to the user in the app' },
      },
      required: ['edits'],
    },
    method: 'edit',
  },
  { name: 'undo', description: 'Undo the last change (the user can also press Ctrl+Z).', inputSchema: { type: 'object', properties: {} }, method: 'undo' },
  { name: 'redo', description: 'Redo.', inputSchema: { type: 'object', properties: {} }, method: 'redo' },
  {
    name: 'transport',
    description: 'Play or stop playback in the app (so the user can hear the result), optionally from a bar.',
    inputSchema: { type: 'object', properties: { action: { enum: ['play', 'stop', 'seek'] }, bar: { type: 'number', description: '1-based bar to start from' } }, required: ['action'] },
    method: 'transport',
  },
  {
    name: 'render_check',
    description: 'Render the song offline and report loudness: overall peak/RMS, whether the master limiter is overworked, per-bar RMS (spot silent or too-loud sections) and per-track levels (mix balance). Optional "path" also writes a 48 kHz WAV. You cannot hear the music; use this to sanity-check your changes.',
    inputSchema: { type: 'object', properties: { path: { type: 'string' }, perTrack: { type: 'boolean' } } },
    method: 'render_check',
  },
  {
    name: 'analyze_audio',
    description: 'Listen to a recorded audio take (hummed melody, beatboxed/tapped rhythm, sung line). Default: the newest take. Returns the rhythm as per-bar drum-grid strings, the melody as notation ready for a pattern, tempo estimate, key guess, per-bar loudness and every detected note with its time, bar and pitch. On "song" grid, bars/steps line up with the timeline; pass "bpm" to quantize a take that was not performed to the click (first note = bar 1).',
    inputSchema: {
      type: 'object',
      properties: {
        clipId: { type: 'number', description: 'Audio clip id (from get_project)' },
        track: { type: ['number', 'string'], description: 'Track id or name: analyses its newest take' },
        assetId: { type: 'number' },
        bpm: { type: 'number', description: 'Quantize on a free grid at this tempo instead of the song grid' },
      },
    },
    method: 'analyze_audio',
  },
  {
    name: 'save_preset',
    description: 'Save a track\'s current instrument as a user preset (shows under "My presets").',
    inputSchema: { type: 'object', properties: { name: { type: 'string' }, track: { type: ['number', 'string'] } }, required: ['name', 'track'] },
    method: 'save_preset',
  },
  {
    name: 'media_info',
    description: 'Videos/images on the timeline (optional feature): file, kind, where each clip sits in song seconds and bars.',
    inputSchema: { type: 'object', properties: {} },
    method: 'media_info',
  },
  {
    name: 'media_frame',
    description: 'See the video frame or image showing at a time (default: the playhead). Returns the picture.',
    inputSchema: { type: 'object', properties: { secs: { type: 'number', description: 'Song time in seconds' }, bar: { type: 'number' }, width: { type: 'number' } } },
    method: 'media_frame',
    image: true,
  },
  {
    name: 'media_overview',
    description: 'Contact sheet of a whole video (grid of evenly spaced frames, with their times) or the image itself. Best first look when scoring to picture.',
    inputSchema: { type: 'object', properties: { assetId: { type: 'number' }, cols: { type: 'number' }, rows: { type: 'number' } } },
    method: 'media_overview',
    image: true,
  },
  {
    name: 'detect_scenes',
    description: 'Find scene cuts in a video, mapped to song seconds and bars, so music changes can land on cuts.',
    inputSchema: { type: 'object', properties: { assetId: { type: 'number' }, threshold: { type: 'number', description: '0.05-0.95, default 0.3 (lower = more cuts)' } } },
    method: 'detect_scenes',
  },
  {
    name: 'notify',
    description: 'Show a short message to the user in the app.',
    inputSchema: { type: 'object', properties: { message: { type: 'string' } }, required: ['message'] },
    method: 'notify',
  },
];

// ---------- talking to the app ----------

function readControl() {
  try {
    return JSON.parse(fs.readFileSync(CONTROL, 'utf8'));
  } catch {
    return null;
  }
}

async function post(ctl, method, params) {
  const res = await fetch(`http://127.0.0.1:${ctl.port}/api`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Token': ctl.token },
    body: JSON.stringify({ method, params }),
    signal: AbortSignal.timeout(300_000),
  });
  return res.json();
}

function findExe() {
  const candidates = [
    process.env.DUET_EXE,
    path.resolve(HERE, '..', '..', 'src-tauri', 'target', 'release', 'duet.exe'),
    path.join(process.env.LOCALAPPDATA || '', 'Duet', 'duet.exe'),
    path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Duet', 'duet.exe'),
    path.resolve(HERE, '..', '..', 'src-tauri', 'target', 'debug', 'duet.exe'),
  ].filter(Boolean);
  return candidates.find((p) => fs.existsSync(p));
}

let launching = null;
async function call(method, params) {
  let ctl = readControl();
  try {
    if (!ctl) throw new Error('no control file');
    return await post(ctl, method, params);
  } catch (e) {
    if (e.name === 'TimeoutError') throw new Error('Duet took too long to answer.');
    launching ||= launchApp();
    await launching;
    launching = null;
    ctl = readControl();
    return post(ctl, method, params);
  }
}

async function launchApp() {
  const exe = findExe();
  if (!exe) throw new Error('Duet is not running and its exe was not found. Start the app (or set DUET_EXE).');
  const before = readControl()?.pid;
  spawn(exe, [], { detached: true, stdio: 'ignore' }).unref();
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 500));
    const ctl = readControl();
    if (ctl && ctl.pid !== before) {
      try {
        const r = await post(ctl, 'status', {});
        if (r.ok) return;
      } catch { /* not up yet */ }
    }
  }
  throw new Error('Started Duet but it did not respond within 30 s.');
}

// ---------- MCP (JSON-RPC 2.0 over stdio, newline-delimited) ----------

function send(msg) {
  process.stdout.write(JSON.stringify(msg) + '\n');
}

async function handle(msg) {
  const { id, method, params } = msg;
  if (method === 'initialize') {
    return send({
      jsonrpc: '2.0', id,
      result: {
        protocolVersion: params?.protocolVersion || '2025-06-18',
        capabilities: { tools: {} },
        serverInfo: { name: 'duet', version: '0.2.0' },
        instructions: 'Tools to compose and edit music in the running Duet DAW, and to look at its optional video/images. Read the compose-music skill for the workflow and notation.',
      },
    });
  }
  if (method === 'ping') return send({ jsonrpc: '2.0', id, result: {} });
  if (method === 'tools/list') {
    return send({ jsonrpc: '2.0', id, result: { tools: TOOLS.map(({ name, description, inputSchema }) => ({ name, description, inputSchema })) } });
  }
  if (method === 'tools/call') {
    const tool = TOOLS.find((t) => t.name === params?.name);
    if (!tool) return send({ jsonrpc: '2.0', id, error: { code: -32602, message: `Unknown tool ${params?.name}` } });
    try {
      const r = await call(tool.method, params.arguments || {});
      if (!r.ok) return send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: r.error }] } });
      const content = [];
      let result = r.result;
      if (tool.image && result?.image) {
        content.push({ type: 'image', data: result.image.data, mimeType: result.image.mimeType });
        result = result.info;
      }
      content.push({ type: 'text', text: JSON.stringify(result, null, 1) });
      return send({ jsonrpc: '2.0', id, result: { content } });
    } catch (e) {
      return send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: String(e.message || e) }] } });
    }
  }
  if (id !== undefined) send({ jsonrpc: '2.0', id, error: { code: -32601, message: `Method not found: ${method}` } });
}

let buf = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  buf += chunk;
  let nl;
  while ((nl = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, nl).trim();
    buf = buf.slice(nl + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    handle(msg).catch((e) => process.stderr.write(String(e) + '\n'));
  }
});
