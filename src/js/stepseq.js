// Step sequencer view of the selected pattern: drum rows for drum kits,
// in-key note rows for melodic instruments.
import { invoke } from './api.js';
import { store, on, apply, track, selectedPattern, currentTick } from './store.js';
import { el, fitCanvas, roundRect, barTicks, clamp, noteName, inScale, STEP, shade } from './util.js';
import { previewNote } from './pianoroll.js';

const LABEL_W = 110;
const CELL = 24;
const TOP = 6;

const DRUM_ROWS = [
  [49, 'Crash'], [51, 'Ride'], [46, 'Open hat'], [42, 'Closed hat'], [39, 'Clap'],
  [38, 'Snare'], [37, 'Rim'], [50, 'Tom high'], [45, 'Tom mid'], [41, 'Tom low'], [36, 'Kick'],
];

let canvas, bar;
let scrollX = 0;
let scrollY = 0;
let paint = null; // { adding, rows visited }
let needsDraw = true;
let melodicOctave = 4;
let noteSteps = 1;

export const requestDraw = () => { needsDraw = true; };

export function init() {
  canvas = document.getElementById('ss-canvas');
  bar = document.getElementById('ss-bar');
  on('project', () => { renderBar(); requestDraw(); });
  on('selection', () => { renderBar(); requestDraw(); });
  new ResizeObserver(requestDraw).observe(canvas);
  canvas.addEventListener('mousedown', onDown);
  canvas.addEventListener('contextmenu', (e) => e.preventDefault());
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) scrollX = Math.max(0, scrollX + (e.deltaX || e.deltaY));
    else scrollY = Math.max(0, scrollY + e.deltaY);
    requestDraw();
  }, { passive: false });
  window.addEventListener('mousemove', onMove);
  window.addEventListener('mouseup', () => {
    if (paint?.changed) commit();
    paint = null;
  });
}

function instTrack() {
  const t = track(store.selectedTrackId);
  return t?.kind.type === 'instrument' ? t : null;
}

const isDrums = () => instTrack()?.kind.instrument.type === 'drums';

function rows() {
  if (isDrums()) return DRUM_ROWS;
  const p = store.project;
  const out = [];
  const lo = (melodicOctave + 1) * 12;
  for (let pi = lo + 24; pi >= lo; pi--) if (inScale(pi, p.keyRoot, p.scale)) out.push([pi, noteName(pi)]);
  return out;
}

function renderBar() {
  bar.innerHTML = '';
  const t = instTrack();
  const pat = selectedPattern();
  bar.append(
    el('span', { class: 'muted' }, pat ? `Pattern: ${pat.name}` : 'No pattern'),
    el('span', { class: 'muted' }, t ? ` · Track: ${t.name}` : ' · select an instrument track'),
    el('div', { class: 'sep' }),
  );
  if (!isDrums()) {
    bar.append(
      el('span', { class: 'muted' }, 'Octave'),
      el('button', { class: 'mini', onclick: () => { melodicOctave = clamp(melodicOctave - 1, 0, 8); requestDraw(); } }, '−'),
      el('span', {}, String(melodicOctave)),
      el('button', { class: 'mini', onclick: () => { melodicOctave = clamp(melodicOctave + 1, 0, 8); requestDraw(); } }, '+'),
      el('span', { class: 'muted' }, 'Note length'),
      el('select', { onchange: (e) => { noteSteps = +e.target.value; } },
        [[1, '1 step'], [2, '2 steps'], [4, '1 beat'], [8, '2 beats']].map(([v, l]) => el('option', { value: v, selected: v === noteSteps }, l))),
    );
  }
  bar.append(el('span', { class: 'muted small' }, ' · click/drag to toggle steps · right-click to clear · click a row name to hear it'));
}

let working = null;
function notesNow() {
  return working || selectedPattern()?.notes || [];
}

function commit() {
  const pat = selectedPattern();
  if (!pat || !working) return;
  const notes = working;
  working = null;
  apply({ type: 'setNotes', patternId: pat.id, notes });
}

function cellAt(x, y) {
  const r = rows();
  const ri = Math.floor((y - TOP + scrollY) / CELL);
  const step = Math.floor((x - LABEL_W + scrollX) / CELL);
  return { ri, step, row: r[ri] };
}

function noteIndex(notes, pitch, step) {
  return notes.findIndex((n) => n.pitch === pitch && n.start >= step * STEP && n.start < (step + 1) * STEP);
}

function onDown(e) {
  store.focus = 'stepseq';
  const pat = selectedPattern();
  if (!pat) return;
  const { offsetX: x, offsetY: y } = e;
  const c = cellAt(x, y);
  if (!c.row) return;
  if (x < LABEL_W) {
    previewNote(c.row[0], 0.85, 250);
    return;
  }
  const steps = Math.round(pat.length / STEP);
  if (c.step < 0 || c.step >= steps) return;
  working = notesNow().map((n) => ({ ...n }));
  const has = noteIndex(working, c.row[0], c.step) >= 0;
  paint = { adding: e.button === 0 && !has, changed: false, done: new Set() };
  toggle(c);
}

function toggle(c) {
  const key = `${c.ri}:${c.step}`;
  if (paint.done.has(key)) return;
  paint.done.add(key);
  const pitch = c.row[0];
  const i = noteIndex(working, pitch, c.step);
  if (paint.adding && i < 0) {
    const len = isDrums() ? STEP : STEP * noteSteps;
    working.push({ id: 0, pitch, start: c.step * STEP, length: len, velocity: 0.8 });
    previewNote(pitch, 0.8, 150);
    paint.changed = true;
  } else if (!paint.adding && i >= 0) {
    working.splice(i, 1);
    paint.changed = true;
  }
  requestDraw();
}

function onMove(e) {
  if (!paint) return;
  const r = canvas.getBoundingClientRect();
  const c = cellAt(e.clientX - r.left, e.clientY - r.top);
  const pat = selectedPattern();
  if (c.row && c.step >= 0 && c.step < Math.round(pat.length / STEP)) toggle(c);
}

export function draw() {
  needsDraw = false;
  const { ctx, w, h } = fitCanvas(canvas);
  ctx.fillStyle = '#141519';
  ctx.fillRect(0, 0, w, h);
  const pat = selectedPattern();
  const p = store.project;
  if (!pat || !p) {
    ctx.fillStyle = '#8c91a6';
    ctx.font = '13px "Segoe UI"';
    ctx.fillText('No pattern selected.', 20, 30);
    return;
  }
  if (!instTrack()) {
    ctx.fillStyle = '#8c91a6';
    ctx.font = '13px "Segoe UI"';
    ctx.fillText('Select an instrument track (in the playlist) to sequence it.', 20, 30);
    return;
  }
  const r = rows();
  const steps = Math.round(pat.length / STEP);
  const stepsPerBar = Math.round(barTicks(p) / STEP);
  const notes = notesNow();
  const t = instTrack();
  const color = pat.color || t?.color || '#ff4fd8';
  const playStep = playingStep(pat);
  ctx.font = '12px "Segoe UI"';
  r.forEach(([pitch, label], ri) => {
    const y = TOP + ri * CELL - scrollY;
    if (y + CELL < 0 || y > h) return;
    ctx.fillStyle = ri % 2 ? '#1a1b22' : '#1d1e26';
    ctx.fillRect(0, y, LABEL_W - 4, CELL - 2);
    ctx.fillStyle = '#d4d6e0';
    ctx.fillText(label, 8, y + 16);
    for (let s = 0; s < steps; s++) {
      const x = LABEL_W + s * CELL - scrollX;
      if (x + CELL < LABEL_W || x > w) continue;
      const barIdx = Math.floor(s / stepsPerBar);
      const beatGroup = Math.floor((s % stepsPerBar) / 4) % 2;
      const on = noteIndex(notes, pitch, s) >= 0;
      roundRect(ctx, x + 1, y + 1, CELL - 4, CELL - 4, 4);
      if (on) {
        const n = notes[noteIndex(notes, pitch, s)];
        ctx.fillStyle = shade(color, -0.3 + n.velocity * 0.4);
      } else {
        ctx.fillStyle = beatGroup ? (barIdx % 2 ? '#262836' : '#2a2c3a') : barIdx % 2 ? '#20222d' : '#232532';
      }
      ctx.fill();
      if (s === playStep) {
        ctx.strokeStyle = '#3ee6ff';
        ctx.lineWidth = 1.5;
        ctx.stroke();
      }
    }
  });
}

function playingStep(pat) {
  if (!store.transport.playing) return -1;
  const tick = currentTick();
  for (const t of store.project.tracks) {
    for (const c of t.clips) {
      if (c.content.patternId === pat.id && tick >= c.start && tick < c.start + c.length) {
        return Math.floor(((tick - c.start + c.offset) % pat.length) / STEP);
      }
    }
  }
  return -1;
}

export function frame() {
  if (store.tab !== 'stepseq') return;
  if (store.transport.playing) needsDraw = true;
  if (needsDraw) draw();
}
