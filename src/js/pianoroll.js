// Piano roll editor for the selected pattern.
import { invoke } from './api.js';
import { store, on, emit, apply, track, pattern, selectedPattern, selectPattern, currentTick } from './store.js';
import {
  el, fitCanvas, roundRect, barTicks, beatTicks, snapTicks, clamp, noteName, isBlack, inScale, SCALES, shade, toast, STEP,
} from './util.js';
import { seekTo } from './timeline.js';

const KEY_W = 56;
const ROW_H = 13;
const RULER_H = 18;
const VEL_H = 64;

const view = { scrollX: 0, scrollY: ROW_H * (127 - 84), ppt: 0.9 };
let canvas, bar;
let notes = []; // working copy of the pattern's notes
let notesFor = null; // pattern id the working copy belongs to
let sel = new Set();
let pendingSelKeys = null;
let drag = null;
let lastLength = STEP * 2;
let lastMouseTick = 0;
let tempId = -1;
let clipboard = [];
let chordMode = 'none';
let needsDraw = true;
let heldKey = null;
let gridSnap = '1/4'; // piano roll snap, separate from the playlist's

export const requestDraw = () => { needsDraw = true; };

/** Ids of the selected notes (for the assistant's context). */
export const selectedNoteIds = () => [...sel].filter((id) => id > 0);

export function init() {
  canvas = document.getElementById('pr-canvas');
  bar = document.getElementById('pr-bar');
  on('project', () => { syncNotes(); renderBar(); requestDraw(); });
  on('selection', () => { syncNotes(); renderBar(); requestDraw(); });
  new ResizeObserver(requestDraw).observe(canvas);
  canvas.addEventListener('mousedown', onDown);
  canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  canvas.addEventListener('mousemove', onHover);
  canvas.addEventListener('wheel', onWheel, { passive: false });
  canvas.addEventListener('dblclick', onDbl);
  window.addEventListener('mousemove', onMove);
  window.addEventListener('mouseup', onUp);
}

const keyOf = (n) => `${n.pitch}:${n.start}:${n.length}`;

function syncNotes() {
  const pat = selectedPattern();
  if (drag) return;
  if (!pat) { notes = []; notesFor = null; sel.clear(); return; }
  if (notesFor !== pat.id) {
    sel.clear();
    setTimeout(scrollToNotes, 0);
  }
  notesFor = pat.id;
  notes = pat.notes.map((n) => ({ ...n }));
  if (pendingSelKeys) {
    sel = new Set(notes.filter((n) => pendingSelKeys.has(keyOf(n))).map((n) => n.id));
    pendingSelKeys = null;
  } else {
    const ids = new Set(notes.map((n) => n.id));
    sel = new Set([...sel].filter((id) => ids.has(id)));
  }
}

function commit() {
  const pat = selectedPattern();
  if (!pat) return;
  pendingSelKeys = new Set(notes.filter((n) => sel.has(n.id)).map(keyOf));
  apply({
    type: 'setNotes',
    patternId: pat.id,
    notes: notes.map((n) => ({ id: n.id > 0 ? n.id : 0, pitch: n.pitch, start: Math.round(n.start), length: Math.max(1, Math.round(n.length)), velocity: n.velocity })),
  });
}

/** The instrument track used to audition notes. */
function previewTrack() {
  const t = track(store.selectedTrackId);
  if (t?.kind.type === 'instrument') return t;
  const pid = store.selectedPatternId;
  return store.project.tracks.find((x) => x.kind.type === 'instrument' && x.clips.some((c) => c.content.patternId === pid));
}

export function previewNote(pitch, vel = 0.8, ms = 220) {
  const t = previewTrack();
  if (!t) return;
  invoke('note_on', { trackId: t.id, pitch, vel }).catch(() => {});
  setTimeout(() => invoke('note_off', { trackId: t.id, pitch }).catch(() => {}), ms);
}

// ---------- toolbar ----------
function renderBar() {
  const p = store.project;
  if (!p) return;
  const pat = selectedPattern();
  bar.innerHTML = '';
  const sel = el('select', { title: 'Pattern being edited', onchange: (e) => selectPattern(+e.target.value) },
    p.patterns.map((x) => el('option', { value: x.id, selected: x.id === pat?.id }, x.name)));
  const bars = pat ? pat.length / barTicks(p) : 1;
  const lenInput = el('input', {
    type: 'number', min: 0.25, max: 256, step: 0.25, value: +bars.toFixed(2), title: 'Pattern length in bars',
    onchange: (e) => pat && apply({ type: 'updatePattern', patternId: pat.id, length: Math.max(STEP, Math.round(+e.target.value * barTicks(p))) }),
  });
  const pt = previewTrack();
  bar.append(
    el('span', { class: 'muted' }, 'Pattern'), sel,
    el('button', { onclick: newPattern, title: 'New empty pattern' }, 'New'),
    el('button', { disabled: !pat, onclick: () => pat && apply({ type: 'clonePattern', patternId: pat.id }).then((id) => id && selectPattern(id)), title: 'Copy this pattern' }, 'Clone'),
    el('button', { disabled: !pat, onclick: renamePattern }, 'Rename'),
    el('button', { disabled: !pat, class: 'danger', onclick: () => pat && confirm(`Delete pattern "${pat.name}" and its clips?`) && apply({ type: 'removePattern', patternId: pat.id }) }, 'Delete'),
    el('div', { class: 'sep' }),
    el('span', { class: 'muted' }, 'Bars'), lenInput,
    el('span', { class: 'muted' }, 'Snap'),
    el('select', { title: 'Piano roll grid (hold Alt to place/drag without snapping)', onchange: (e) => { gridSnap = e.target.value; requestDraw(); } },
      [['bar', 'Bar'], ['beat', 'Beat'], ['1/2', '1/2 beat'], ['1/3', '1/3 beat'], ['1/4', '1/4 beat'], ['1/6', '1/6 beat'], ['1/8', '1/8 beat'], ['none', 'None']]
        .map(([v, l]) => el('option', { value: v, selected: v === gridSnap }, l))),
    el('button', { disabled: !pat, onclick: quantize, title: 'Snap note starts to the grid (Ctrl+Q). Applies to selected notes, or all if none selected.' }, 'Quantize'),
    el('span', { class: 'muted' }, 'Chord'),
    el('select', { title: 'Stamp a chord from the current key/scale when you place a note', onchange: (e) => { chordMode = e.target.value; } },
      [['none', 'Single note'], ['triad', 'Triad (in key)'], ['seventh', '7th chord (in key)'], ['power', 'Power (1-5-8)'], ['octave', 'Octave']]
        .map(([v, l]) => el('option', { value: v, selected: v === chordMode }, l))),
    el('div', { class: 'sep' }),
    el('span', { class: 'muted small' }, pt ? `Hearing: ${pt.name}` : 'Select an instrument track to hear notes'),
    el('span', { class: 'muted small' }, ' · click: add · drag: move · edge: resize · right-click: delete · Ctrl+drag: select'),
  );
}

async function newPattern() {
  const id = await apply({ type: 'addPattern', length: barTicks(store.project) });
  if (id) selectPattern(id);
}

function renamePattern() {
  const pat = selectedPattern();
  const name = pat && prompt('Pattern name', pat.name);
  if (name) apply({ type: 'updatePattern', patternId: pat.id, name });
}

function quantize() {
  const s = snapTicks(gridSnap, store.project);
  const target = sel.size ? notes.filter((n) => sel.has(n.id)) : notes;
  for (const n of target) n.start = Math.max(0, Math.round(n.start / s) * s);
  commit();
  toast(`Quantized ${target.length} note${target.length === 1 ? '' : 's'} to ${gridSnap === 'none' ? 'ticks' : gridSnap}`, 'ok', 1800);
}

// ---------- geometry ----------
const tickAtX = (x) => view.scrollX + (x - KEY_W) / view.ppt;
const xAtTick = (t) => KEY_W + (t - view.scrollX) * view.ppt;
const pitchAtY = (y) => clamp(127 - Math.floor((y - RULER_H + view.scrollY) / ROW_H), 0, 127);
const yAtPitch = (p) => RULER_H + (127 - p) * ROW_H - view.scrollY;
const gridBottom = () => canvas.clientHeight - VEL_H;
const snap = (alt) => (alt ? 1 : snapTicks(gridSnap, store.project));

function noteAt(x, y) {
  const t = tickAtX(x);
  const p = pitchAtY(y);
  let body = null;
  for (let i = notes.length - 1; i >= 0 && !body; i--) {
    const n = notes[i];
    if (n.pitch === p && t >= n.start && t < n.start + n.length) body = n;
  }
  // Resize handle: a zone straddling each note's right end, so grabbing right on
  // (or just past) the edge works even for short notes. The part past the end
  // yields to another note's body.
  for (let i = notes.length - 1; i >= 0; i--) {
    const n = notes[i];
    if (n.pitch !== p) continue;
    const x0 = xAtTick(n.start), x1 = xAtTick(n.start + n.length);
    const inside = Math.min(7, Math.max(2, (x1 - x0) / 3));
    if (x >= x1 - inside && (x <= x1 || (!body && x <= x1 + 4))) return { n, edge: true };
  }
  return body ? { n: body, edge: false } : null;
}

function chordPitches(root) {
  const p = store.project;
  const scale = (SCALES[p.scale] || SCALES.major).steps;
  if (chordMode === 'power') return [root, root + 7, root + 12];
  if (chordMode === 'octave') return [root, root + 12];
  if (chordMode === 'none') return [root];
  // Diatonic chord: stack scale degrees in thirds from the clicked note.
  const deg = (pitch) => scale.indexOf((((pitch - p.keyRoot) % 12) + 12) % 12);
  const d = deg(root);
  if (d < 0) return [root, root + 4, root + 7];
  const count = chordMode === 'seventh' ? 4 : 3;
  const out = [];
  const base = root - scale[d]; // pitch of the key root at or below the clicked note
  for (let k = 0; k < count; k++) {
    const idx = d + k * 2;
    const oct = Math.floor(idx / scale.length);
    out.push(base + scale[idx % scale.length] + oct * 12);
  }
  return out.filter((x) => x <= 127);
}

// ---------- drawing ----------
export function draw() {
  needsDraw = false;
  const p = store.project;
  const { ctx, w, h } = fitCanvas(canvas);
  ctx.fillStyle = '#141519';
  ctx.fillRect(0, 0, w, h);
  const pat = selectedPattern();
  if (!p || !pat) {
    ctx.fillStyle = '#8c91a6';
    ctx.font = '13px "Segoe UI"';
    ctx.fillText('No pattern selected. Click "New" above, or click an instrument lane in the playlist.', 20, 40);
    return;
  }
  const gb = gridBottom();
  const t = previewTrack();
  const color = pat.color || t?.color || '#ff4fd8';

  // rows
  ctx.save();
  ctx.beginPath();
  ctx.rect(KEY_W, RULER_H, w - KEY_W, gb - RULER_H);
  ctx.clip();
  const pTop = pitchAtY(RULER_H), pBot = pitchAtY(gb);
  for (let pi = pBot; pi <= pTop; pi++) {
    const y = yAtPitch(pi);
    const scaleNote = inScale(pi, p.keyRoot, p.scale);
    const root = ((pi - p.keyRoot) % 12 + 12) % 12 === 0;
    ctx.fillStyle = root ? '#252238' : scaleNote ? (isBlack(pi) ? '#1b1c25' : '#1f2029') : isBlack(pi) ? '#141519' : '#18191f';
    ctx.fillRect(KEY_W, y, w - KEY_W, ROW_H);
    ctx.fillStyle = pi % 12 === 0 ? '#343848' : '#1c1d25';
    ctx.fillRect(KEY_W, y + ROW_H - 1, w - KEY_W, 1);
  }
  // vertical grid
  const bt = barTicks(p), beat = beatTicks(p), s = snapTicks(gridSnap, p);
  const minor = s * view.ppt >= 6 ? s : beat;
  for (let tk = Math.floor(view.scrollX / minor) * minor; tk < tickAtX(w); tk += minor) {
    const x = Math.round(xAtTick(tk)) + 0.5;
    ctx.fillStyle = Math.abs(tk % bt) < 0.5 ? '#3a3f52' : Math.abs(tk % beat) < 0.5 ? '#2a2d3b' : '#1f2029';
    ctx.fillRect(x, RULER_H, 1, gb - RULER_H);
  }
  // beyond the pattern end
  const ex = xAtTick(pat.length);
  if (ex < w) {
    ctx.fillStyle = '#00000088';
    ctx.fillRect(Math.max(ex, KEY_W), RULER_H, w - ex, gb);
    ctx.fillStyle = '#ff4fd8';
    ctx.fillRect(ex, RULER_H, 2, gb - RULER_H);
  }
  // notes
  ctx.font = '10px "Segoe UI"';
  for (const n of notes) {
    const x = xAtTick(n.start);
    const nw = Math.max(3, n.length * view.ppt - 1);
    if (x > w || x + nw < KEY_W) continue;
    const y = yAtPitch(n.pitch);
    if (y > gb || y + ROW_H < RULER_H) continue;
    const isSel = sel.has(n.id);
    roundRect(ctx, x, y + 1, nw, ROW_H - 2, 3);
    ctx.fillStyle = shade(color, -0.35 + n.velocity * 0.45);
    ctx.fill();
    ctx.strokeStyle = isSel ? '#ffffff' : shade(color, -0.6);
    ctx.lineWidth = isSel ? 1.5 : 1;
    ctx.stroke();
    if (nw > 26) {
      ctx.fillStyle = '#0b0b10cc';
      ctx.fillText(noteName(n.pitch), x + 3, y + ROW_H - 3);
    }
  }
  if (drag?.mode === 'select') {
    const x0 = Math.min(drag.x0, drag.x1), y0 = Math.min(drag.y0, drag.y1);
    ctx.strokeStyle = '#3ee6ff';
    ctx.fillStyle = '#3ee6ff18';
    ctx.fillRect(x0, y0, Math.abs(drag.x1 - drag.x0), Math.abs(drag.y1 - drag.y0));
    ctx.strokeRect(x0 + 0.5, y0 + 0.5, Math.abs(drag.x1 - drag.x0), Math.abs(drag.y1 - drag.y0));
  }
  const lt = localPlayhead();
  if (lt !== null) {
    const x = xAtTick(lt);
    ctx.fillStyle = '#3ee6ff';
    ctx.fillRect(Math.round(x), RULER_H, 1.5, gb - RULER_H);
  }
  ctx.restore();

  // keyboard
  for (let pi = pBot; pi <= pTop; pi++) {
    const y = yAtPitch(pi);
    const black = isBlack(pi);
    ctx.fillStyle = heldKey === pi ? '#ff4fd8' : black ? '#22232b' : '#d9dbe4';
    ctx.fillRect(0, y, KEY_W, ROW_H - 0.5);
    if (pi % 12 === 0) {
      ctx.fillStyle = '#20222c';
      ctx.fillText(noteName(pi), KEY_W - 26, y + ROW_H - 3);
    }
  }
  ctx.fillStyle = '#343848';
  ctx.fillRect(KEY_W - 1, RULER_H, 1, gb - RULER_H);

  // ruler
  ctx.fillStyle = '#1c1e26';
  ctx.fillRect(0, 0, w, RULER_H);
  ctx.fillStyle = '#c9ccd8';
  ctx.font = '11px "Segoe UI"';
  for (let tk = Math.floor(view.scrollX / bt) * bt; tk < tickAtX(w); tk += bt) {
    const x = xAtTick(tk);
    if (x < KEY_W) continue;
    ctx.fillRect(Math.round(x), 0, 1, RULER_H);
    ctx.fillText(String(Math.round(tk / bt) + 1), x + 3, 13);
  }

  // velocity lane
  ctx.fillStyle = '#101116';
  ctx.fillRect(0, gb, w, VEL_H);
  ctx.fillStyle = '#343848';
  ctx.fillRect(0, gb, w, 1);
  ctx.fillStyle = '#8c91a6';
  ctx.fillText('Velocity', 6, gb + 16);
  for (const n of notes) {
    const x = xAtTick(n.start);
    if (x < KEY_W || x > w) continue;
    const vh = (VEL_H - 8) * n.velocity;
    ctx.fillStyle = sel.has(n.id) ? '#ffffff' : color;
    ctx.fillRect(x, gb + VEL_H - 2 - vh, 3, vh);
    ctx.fillRect(x - 2, gb + VEL_H - 2 - vh, 7, 2);
  }
}

/** Where the playhead is inside this pattern, if a clip of it is playing. */
function localPlayhead() {
  const pat = selectedPattern();
  if (!pat || !pat.length) return null;
  const tick = currentTick();
  for (const t of store.project.tracks) {
    for (const c of t.clips) {
      if (c.content.patternId === pat.id && tick >= c.start && tick < c.start + c.length) {
        return (tick - c.start + c.offset) % pat.length;
      }
    }
  }
  return null;
}

export function frame() {
  if (store.tab !== 'pianoroll') return;
  if (store.transport.playing) needsDraw = true;
  if (needsDraw) draw();
}

// ---------- mouse ----------
function onWheel(e) {
  e.preventDefault();
  const x = e.offsetX;
  if (e.ctrlKey) {
    const t = tickAtX(x);
    view.ppt = clamp(view.ppt * (e.deltaY < 0 ? 1.15 : 1 / 1.15), 0.08, 8);
    view.scrollX = Math.max(0, t - (x - KEY_W) / view.ppt);
  } else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
    view.scrollX = Math.max(0, view.scrollX + (e.deltaX || e.deltaY) / view.ppt);
  } else {
    view.scrollY = clamp(view.scrollY + e.deltaY, 0, 128 * ROW_H - (gridBottom() - RULER_H));
  }
  requestDraw();
}

function onHover(e) {
  if (drag) return;
  const { offsetX: x, offsetY: y } = e;
  if (x < KEY_W || y < RULER_H || y > gridBottom()) { canvas.style.cursor = y > gridBottom() ? 'ns-resize' : 'pointer'; return; }
  const h = noteAt(x, y);
  canvas.style.cursor = h ? (h.edge ? 'ew-resize' : 'move') : 'crosshair';
}

function selectedNotes() {
  return notes.filter((n) => sel.has(n.id));
}

function onDown(e) {
  store.focus = 'pianoroll';
  const pat = selectedPattern();
  if (!pat) return;
  const { offsetX: x, offsetY: y } = e;
  const gb = gridBottom();
  if (y < RULER_H) {
    const c = store.project.tracks.flatMap((t) => t.clips).find((c) => c.content.patternId === pat.id);
    if (c) seekTo(c.start - c.offset + Math.max(0, tickAtX(x)));
    return;
  }
  if (y > gb) {
    drag = { mode: 'vel' };
    setVelocity(x, y);
    return;
  }
  if (x < KEY_W) {
    const t = previewTrack();
    if (t) {
      heldKey = pitchAtY(y);
      invoke('note_on', { trackId: t.id, pitch: heldKey, vel: 0.85 }).catch(() => {});
      drag = { mode: 'keys', trackId: t.id };
      requestDraw();
    }
    return;
  }
  const tick = tickAtX(x);
  const pitch = pitchAtY(y);
  lastMouseTick = tick;
  if (e.button === 2) {
    drag = { mode: 'erase', changed: false };
    erase(x, y);
    return;
  }
  if (e.button !== 0) return;
  const h = noteAt(x, y);
  if (h) {
    if (e.shiftKey) {
      sel.has(h.n.id) ? sel.delete(h.n.id) : sel.add(h.n.id);
      requestDraw();
      return;
    }
    if (!sel.has(h.n.id)) sel = new Set([h.n.id]);
    if (e.ctrlKey && !h.edge) {
      // duplicate selection and drag the copies
      const copies = selectedNotes().map((n) => ({ ...n, id: tempId-- }));
      notes.push(...copies);
      const anchor = copies[selectedNotes().indexOf(h.n)] || copies[0];
      sel = new Set(copies.map((n) => n.id));
      startNoteDrag('move', anchor, tick, pitch);
    } else {
      startNoteDrag(h.edge ? 'resize' : 'move', h.n, tick, pitch);
    }
    previewNote(h.n.pitch, h.n.velocity);
    requestDraw();
    return;
  }
  if (e.ctrlKey) {
    drag = { mode: 'select', x0: x, y0: y, x1: x, y1: y, base: e.shiftKey ? new Set(sel) : new Set() };
    return;
  }
  // add note(s)
  const s = snap(e.altKey);
  const start = Math.max(0, Math.floor(tick / s) * s);
  const created = chordPitches(pitch).map((pp) => ({ id: tempId--, pitch: pp, start, length: lastLength, velocity: 0.8 }));
  notes.push(...created);
  sel = new Set(created.map((n) => n.id));
  for (const n of created) previewNote(n.pitch, n.velocity);
  startNoteDrag('move', created[0], tick, pitch);
  drag.changed = true;
  requestDraw();
}

function startNoteDrag(mode, anchor, tick, pitch) {
  const orig = new Map(selectedNotes().map((n) => [n.id, { start: n.start, pitch: n.pitch, length: n.length }]));
  drag = { mode, anchorId: anchor.id, tick0: tick, pitch0: pitch, orig, changed: false, lastPitch: anchor.pitch };
}

function setVelocity(x, y) {
  const v = clamp(1 - (y - gridBottom() - 4) / (VEL_H - 8), 0.02, 1);
  const hits = notes.filter((n) => {
    const nx = xAtTick(n.start);
    return x >= nx - 3 && x <= nx + Math.max(6, Math.min(n.length * view.ppt, 12));
  });
  const target = hits.some((n) => sel.has(n.id)) ? hits.filter((n) => sel.has(n.id)) : hits;
  for (const n of target) n.velocity = v;
  if (target.length) drag.changed = true;
  requestDraw();
}

function erase(x, y) {
  const h = noteAt(x, y);
  if (h) {
    notes = notes.filter((n) => n !== h.n);
    sel.delete(h.n.id);
    drag.changed = true;
    requestDraw();
  }
}

function onMove(e) {
  if (!drag) return;
  const r = canvas.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  if (drag.mode === 'keys') {
    const p = pitchAtY(y);
    if (p !== heldKey) {
      invoke('note_off', { trackId: drag.trackId, pitch: heldKey }).catch(() => {});
      heldKey = p;
      invoke('note_on', { trackId: drag.trackId, pitch: p, vel: 0.85 }).catch(() => {});
      requestDraw();
    }
    return;
  }
  if (drag.mode === 'vel') return setVelocity(x, y);
  if (drag.mode === 'erase') return erase(x, y);
  if (drag.mode === 'select') {
    drag.x1 = x;
    drag.y1 = y;
    const t0 = tickAtX(Math.min(drag.x0, x)), t1 = tickAtX(Math.max(drag.x0, x));
    const p0 = pitchAtY(Math.max(drag.y0, y)), p1 = pitchAtY(Math.min(drag.y0, y));
    sel = new Set(drag.base);
    for (const n of notes) if (n.pitch >= p0 && n.pitch <= p1 && n.start < t1 && n.start + n.length > t0) sel.add(n.id);
    requestDraw();
    return;
  }
  const tick = tickAtX(x);
  const s = snap(e.altKey);
  const a = drag.orig.get(drag.anchorId);
  if (!a) return;
  if (drag.mode === 'move') {
    const ns = Math.max(0, Math.round((a.start + tick - drag.tick0) / s) * s);
    const dt = ns - a.start;
    const dp = pitchAtY(y) - drag.pitch0;
    let minStart = Infinity;
    for (const [, o] of drag.orig) minStart = Math.min(minStart, o.start);
    const dtc = Math.max(dt, -minStart);
    for (const n of notes) {
      const o = drag.orig.get(n.id);
      if (!o) continue;
      n.start = o.start + dtc;
      n.pitch = clamp(o.pitch + dp, 0, 127);
    }
    const anchor = notes.find((n) => n.id === drag.anchorId);
    if (anchor && anchor.pitch !== drag.lastPitch) {
      drag.lastPitch = anchor.pitch;
      previewNote(anchor.pitch, anchor.velocity, 150);
    }
    if (dtc !== 0 || dp !== 0) drag.changed = true;
  } else if (drag.mode === 'resize') {
    if (!drag.changed && Math.abs(tick - drag.tick0) * view.ppt < 3) return;
    const end = Math.round((a.start + a.length + tick - drag.tick0) / s) * s;
    const dl = Math.max(end, a.start + Math.min(s, a.length)) - (a.start + a.length);
    for (const n of notes) {
      const o = drag.orig.get(n.id);
      if (o) n.length = Math.max(1, o.length + dl);
    }
    const anchor = notes.find((n) => n.id === drag.anchorId);
    if (anchor) lastLength = anchor.length;
    if (dl !== 0) drag.changed = true;
  }
  requestDraw();
}

function onUp() {
  if (!drag) return;
  const d = drag;
  drag = null;
  if (d.mode === 'keys') {
    invoke('note_off', { trackId: d.trackId, pitch: heldKey }).catch(() => {});
    heldKey = null;
    requestDraw();
    return;
  }
  if (d.changed) commit();
  requestDraw();
}

function onDbl(e) {
  // Double-click a note to delete it (quick alternative to right-click).
  const h = e.offsetX > KEY_W && e.offsetY > RULER_H && e.offsetY < gridBottom() && noteAt(e.offsetX, e.offsetY);
  if (h && !drag) {
    notes = notes.filter((n) => n !== h.n);
    sel.delete(h.n.id);
    commit();
  }
}

// ---------- keyboard ----------
export function onKey(e) {
  const pat = selectedPattern();
  if (!pat) return false;
  const k = e.key;
  const s = snapTicks(gridSnap, store.project);
  const selN = selectedNotes();
  if ((k === 'Delete' || k === 'Backspace') && selN.length) {
    notes = notes.filter((n) => !sel.has(n.id));
    sel.clear();
    commit();
    return true;
  }
  if (e.ctrlKey && k.toLowerCase() === 'a') {
    sel = new Set(notes.map((n) => n.id));
    requestDraw();
    return true;
  }
  if (e.ctrlKey && k.toLowerCase() === 'q') { quantize(); return true; }
  if (e.ctrlKey && k.toLowerCase() === 'c' && selN.length) {
    const min = Math.min(...selN.map((n) => n.start));
    clipboard = selN.map((n) => ({ ...n, start: n.start - min }));
    toast(`Copied ${selN.length} notes`, 'ok', 1200);
    return true;
  }
  if (e.ctrlKey && k.toLowerCase() === 'v' && clipboard.length) {
    const at = Math.max(0, Math.floor(lastMouseTick / s) * s);
    const pasted = clipboard.map((n) => ({ ...n, id: tempId--, start: n.start + at }));
    notes.push(...pasted);
    sel = new Set(pasted.map((n) => n.id));
    commit();
    return true;
  }
  if (e.ctrlKey && k.toLowerCase() === 'd' && selN.length) {
    const min = Math.min(...selN.map((n) => n.start));
    const max = Math.max(...selN.map((n) => n.start + n.length));
    const span = Math.ceil((max - min) / s) * s;
    const copies = selN.map((n) => ({ ...n, id: tempId--, start: n.start + span }));
    notes.push(...copies);
    sel = new Set(copies.map((n) => n.id));
    commit();
    return true;
  }
  if (selN.length && ['ArrowUp', 'ArrowDown'].includes(k)) {
    const d = (k === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 12 : 1);
    for (const n of selN) n.pitch = clamp(n.pitch + d, 0, 127);
    previewNote(selN[0].pitch, selN[0].velocity, 150);
    commit();
    return true;
  }
  if (selN.length && ['ArrowLeft', 'ArrowRight'].includes(k)) {
    const d = (k === 'ArrowRight' ? 1 : -1) * (e.shiftKey ? barTicks(store.project) : s);
    if (selN.every((n) => n.start + d >= 0)) for (const n of selN) n.start += d;
    commit();
    return true;
  }
  return false;
}

export function scrollToNotes() {
  const pat = selectedPattern();
  if (!pat?.notes.length) return;
  const avg = pat.notes.reduce((a, n) => a + n.pitch, 0) / pat.notes.length;
  view.scrollY = clamp((127 - avg) * ROW_H - (gridBottom() - RULER_H) / 2, 0, 128 * ROW_H);
  view.scrollX = 0;
  requestDraw();
}

