// Global UI state + the serialized edit queue. The Rust side owns the project;
// every change goes through apply() and the UI re-renders from the response.
import { invoke } from './api.js';
import { PPQ, toast, bpmAt } from './util.js';

export const store = {
  project: null,
  canUndo: false,
  canRedo: false,
  dirty: false,
  path: null,
  presets: [],
  templates: [],
  selectedTrackId: null,
  selectedPatternId: null,
  selectedClip: null, // { trackId, clipId }
  snap: 'beat',
  metronome: false,
  typingKeyboard: true,
  octave: 4,
  focus: 'timeline', // which panel gets editing shortcuts
  tab: 'pianoroll',
  transport: { tick: 0, playing: false, recording: false, at: performance.now() },
  meters: [],
  master: 0,
  input: 0,
  load: 0,
};

const subs = new Map();
export function on(ev, fn) {
  if (!subs.has(ev)) subs.set(ev, new Set());
  subs.get(ev).add(fn);
}
export function emit(ev, data) {
  for (const fn of subs.get(ev) || []) {
    try { fn(data); } catch (e) { console.error(ev, e); }
  }
}

export const track = (id) => store.project?.tracks.find((t) => t.id === id);
export const pattern = (id) => store.project?.patterns.find((p) => p.id === id);
export const asset = (id) => store.project?.assets.find((a) => a.id === id);
export const selectedTrack = () => track(store.selectedTrackId);
export const selectedPattern = () => pattern(store.selectedPatternId);

export function setResponse(r) {
  if (!r) return;
  store.project = r.project;
  store.canUndo = r.canUndo;
  store.canRedo = r.canRedo;
  store.dirty = r.dirty;
  store.path = r.path;
  const p = store.project;
  if (!track(store.selectedTrackId)) selectTrack(p.tracks.find((t) => t.kind.type === 'instrument')?.id ?? p.tracks[0]?.id ?? null, false);
  if (!pattern(store.selectedPatternId)) store.selectedPatternId = p.patterns[0]?.id ?? null;
  if (store.selectedClip && !track(store.selectedClip.trackId)?.clips.some((c) => c.id === store.selectedClip.clipId)) store.selectedClip = null;
  emit('project');
  if (r.warnings?.length) toast(r.warnings.join('\n'), 'warn', 8000);
}

export function selectTrack(id, notify = true) {
  if (store.selectedTrackId === id) return;
  store.selectedTrackId = id;
  invoke('set_selected_track', { trackId: id }).catch(() => {});
  // Follow the track: edit a pattern it actually plays, if the current one isn't on it.
  const t = track(id);
  const pats = (t?.clips || []).filter((c) => c.content.type === 'pattern').map((c) => c.content.patternId);
  if (pats.length && !pats.includes(store.selectedPatternId)) store.selectedPatternId = pats[0];
  if (notify) emit('selection');
}

export function selectPattern(id) {
  store.selectedPatternId = id;
  emit('selection');
}

// Edits run strictly one at a time so the UI never sees responses out of order.
let chain = Promise.resolve();
export function apply(edit) {
  const p = chain.then(() => invoke('apply_edit', { edit })).then((r) => {
    setResponse(r);
    return r.newId;
  });
  chain = p.catch(() => {});
  return p.catch((e) => {
    toast(String(e), 'error');
    return undefined;
  });
}

/** For continuous controls (faders, knobs): only the latest value per key is sent while a request is in flight. */
const live = new Map();
export function applyLive(key, edit) {
  let s = live.get(key);
  if (!s) live.set(key, (s = { busy: false, next: null }));
  if (s.busy) { s.next = edit; return; }
  s.busy = true;
  apply(edit).finally(() => {
    s.busy = false;
    if (s.next) { const n = s.next; s.next = null; applyLive(key, n); }
  });
}

export function command(cmd, args) {
  return chain.then(() => invoke(cmd, args)).catch((e) => {
    toast(String(e), 'error');
    throw e;
  });
}

export async function undo() { setResponse(await command('undo')); }
export async function redo() { setResponse(await command('redo')); }

/** Playhead position, extrapolated between transport events for smooth drawing. */
export function currentTick() {
  const t = store.transport;
  const p = store.project;
  if (!t.playing || !p) return t.tick;
  let tick = t.tick + (performance.now() - t.at) * ((bpmAt(p, t.tick) * PPQ) / 60000);
  if (p.loopEnabled && !t.recording && t.tick < p.loopEnd && tick >= p.loopEnd && p.loopEnd > p.loopStart) {
    tick = p.loopStart + (tick - p.loopEnd);
  }
  return tick;
}

export function hasMedia() {
  return !!store.project?.tracks.some((t) => t.kind.type === 'media' && t.clips.length);
}
