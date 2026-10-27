// The playlist: headers (DOM) + rows (canvas) + ruler (canvas).
// Rows: optional tempo lane, tracks (each optionally followed by its automation
// lanes), and the master row (with its own lanes).
import { invoke } from './api.js';
import {
  store, on, emit, apply, applyLive, command, track, pattern, asset, selectTrack, selectPattern, currentTick, setResponse,
} from './store.js';
import {
  el, fitCanvas, roundRect, barTicks, beatTicks, snapTicks, clamp, showMenu, toast, shade, gainToDb, tickToSecs, secsToTick, durationTicks, bpmAt,
} from './util.js';
import * as media from './media.js';
import { targetInfo, availableTargets, valueToNorm, normToValue } from './automation.js';

export const TRACK_H = 56;
const LANE_H = 46;
const MASTER_H = 30;
const LOOP_H = 11;
const TEMPO_MIN = 40;
const TEMPO_MAX = 240;

const view = { scrollX: 0, scrollY: 0, ppt: 0.28 };
let canvas, ruler, headers, inner;
let drag = null;
let rulerDrag = null;
let needsDraw = true;
let rows = [];
let totalH = 0;
const peaks = new Map(); // assetId -> Float32Array | 'loading'
const expanded = new Set(); // track ids (and 'master') whose automation lanes are shown
let showTempo = false;
try { showTempo = localStorage.getItem('mm.showTempo') === '1'; } catch { /* storage unavailable */ }

export const requestDraw = () => { needsDraw = true; };

export function init() {
  canvas = document.getElementById('timeline');
  ruler = document.getElementById('ruler');
  headers = document.getElementById('track-headers');
  inner = el('div', { class: 'th-inner' });
  headers.append(inner);
  const spacer = document.getElementById('ruler-spacer');
  spacer.innerHTML = '';
  spacer.append(el('span', { class: 'muted small', style: { flex: 1 } }, 'Tracks'),
    el('button', { id: 'btn-tempo-lane', class: 'mini toggle' + (showTempo ? ' on' : ''), title: 'Show the tempo lane (tempo changes and ramps)', onclick: toggleTempoLane }, 'Tempo'));

  on('project', () => { renderHeaders(); requestDraw(); });
  on('selection', () => { markSelectedHeader(); requestDraw(); });
  on('thumbs', requestDraw);
  new ResizeObserver(requestDraw).observe(canvas);

  canvas.addEventListener('mousedown', onMouseDown);
  canvas.addEventListener('dblclick', onDblClick);
  canvas.addEventListener('contextmenu', onContextMenu);
  canvas.addEventListener('mousemove', onHover);
  canvas.addEventListener('wheel', onWheel, { passive: false });
  headers.addEventListener('wheel', onWheel, { passive: false });
  ruler.addEventListener('mousedown', onRulerDown);
  ruler.addEventListener('wheel', onWheel, { passive: false });
  window.addEventListener('mousemove', onMouseMove);
  window.addEventListener('mouseup', onMouseUp);
}

function toggleTempoLane() {
  showTempo = !showTempo;
  try { localStorage.setItem('mm.showTempo', showTempo ? '1' : '0'); } catch { /* ignore */ }
  document.getElementById('btn-tempo-lane').classList.toggle('on', showTempo);
  renderHeaders();
  requestDraw();
}

// ---------- layout ----------
function computeRows() {
  const p = store.project;
  const out = [];
  let y = 0;
  if (showTempo) { out.push({ type: 'tempo', y, h: LANE_H }); y += LANE_H; }
  for (const t of p.tracks) {
    out.push({ type: 'track', track: t, y, h: TRACK_H });
    y += TRACK_H;
    if (expanded.has(t.id)) {
      for (const lane of t.automation) { out.push({ type: 'lane', owner: t.id, color: t.color, lane, y, h: LANE_H }); y += LANE_H; }
    }
  }
  out.push({ type: 'master', y, h: MASTER_H });
  y += MASTER_H;
  if (expanded.has('master')) {
    for (const lane of p.master.automation) { out.push({ type: 'lane', owner: null, color: '#ff4fd8', lane, y, h: LANE_H }); y += LANE_H; }
  }
  rows = out;
  totalH = y;
}

const rowAtY = (y) => rows.find((r) => y + view.scrollY >= r.y && y + view.scrollY < r.y + r.h) || null;
const rowTop = (r) => r.y - view.scrollY;

// ---------- coordinates ----------
const tickAtX = (x) => view.scrollX + x / view.ppt;
const xAtTick = (t) => (t - view.scrollX) * view.ppt;
const snap = () => snapTicks(store.snap, store.project);
const snapRound = (t, s = snap()) => Math.round(t / s) * s;
const snapFloor = (t, s = snap()) => Math.floor(t / s) * s;

export function locate(clientX, clientY) {
  const r = canvas.getBoundingClientRect();
  if (clientX < r.left || clientX > r.right || clientY < r.top || clientY > r.bottom) return null;
  const x = clientX - r.left;
  const y = clientY - r.top;
  const row = rowAtY(y);
  return { x, y, tick: Math.max(0, tickAtX(x)), track: row?.type === 'track' ? row.track : null };
}

function hit(x, y) {
  const row = rowAtY(y);
  const tick = tickAtX(x);
  if (!row || row.type !== 'track') return { row, tick };
  const t = row.track;
  for (let k = t.clips.length - 1; k >= 0; k--) {
    const c = t.clips[k];
    if (tick >= c.start && tick < c.start + c.length) {
      const x0 = xAtTick(c.start);
      const x1 = xAtTick(c.start + c.length);
      const wide = x1 - x0 > 16;
      const edge = wide && x1 - x < 7 ? 'right' : wide && x - x0 < 7 ? 'left' : null;
      return { row, t, c, tick, edge };
    }
  }
  return { row, t, tick };
}

// ---------- headers ----------
const KIND_LABEL = { instrument: 'inst', audio: 'audio', media: 'video/img' };

function markSelectedHeader() {
  for (const h of inner.children) if (h.dataset.id) h.classList.toggle('sel', +h.dataset.id === store.selectedTrackId);
}

function autoButton(ownerKey, ownerId, lanes) {
  const on = expanded.has(ownerKey) && lanes.length > 0;
  return el('button', {
    class: 'auto' + (on ? ' on' : ''),
    title: lanes.length ? `Show/hide automation (${lanes.length} lane${lanes.length > 1 ? 's' : ''}). Right-click to add another.` : 'Add an automation lane',
    onmousedown: (e) => e.stopPropagation(),
    onclick: (e) => {
      e.stopPropagation();
      if (!lanes.length) return addLaneMenu(ownerId, e.clientX, e.clientY);
      expanded.has(ownerKey) ? expanded.delete(ownerKey) : expanded.add(ownerKey);
      renderHeaders();
      requestDraw();
    },
    oncontextmenu: (e) => { e.preventDefault(); e.stopPropagation(); addLaneMenu(ownerId, e.clientX, e.clientY); },
  }, 'A');
}

function addLaneMenu(ownerId, x, y) {
  const lanes = ownerId === null ? store.project.master.automation : track(ownerId).automation;
  const used = new Set(lanes.map((l) => l.target));
  showMenu(x, y, [
    { label: 'Add automation lane:', disabled: true, action() {} },
    ...availableTargets(ownerId).map((t) => ({
      label: t.label,
      disabled: used.has(t.target),
      action: async () => {
        const id = await apply({ type: 'addAutomationLane', trackId: ownerId, target: t.target });
        if (id) { expanded.add(ownerId ?? 'master'); renderHeaders(); requestDraw(); }
      },
    })),
  ]);
}

function renderHeaders() {
  const p = store.project;
  if (!p) return;
  computeRows();
  inner.innerHTML = '';
  document.getElementById('tl-empty').hidden = p.tracks.length > 0;
  for (const r of rows) {
    if (r.type === 'track') inner.append(trackHeader(r.track));
    else if (r.type === 'lane') inner.append(laneHeader(r));
    else if (r.type === 'tempo') {
      inner.append(el('div', { class: 'th lane tempo', style: { height: `${r.h}px` } },
        el('div', { class: 'stripe', style: { background: '#3ee6ff' } }),
        el('div', { class: 'body' }, el('div', { class: 'name' }, 'Tempo', el('span', { class: 'kind' }, `${Math.round(p.bpm)} bpm start`)),
          el('div', { class: 'muted small' }, 'click to add · dbl-click to edit'))));
    } else if (r.type === 'master') {
      inner.append(el('div', { class: 'th master', style: { height: `${r.h}px` } },
        el('div', { class: 'stripe', style: { background: '#ff4fd8' } }),
        el('div', { class: 'body row' }, el('div', { class: 'name' }, 'Master'), autoButton('master', null, p.master.automation))));
    }
  }
  inner.style.transform = `translateY(${-view.scrollY}px)`;
}

function trackHeader(t) {
  const type = t.kind.type;
  const btn = (label, cls, on, title, fn) =>
    el('button', { class: `${cls}${on ? ' on' : ''}`, title, onmousedown: (e) => e.stopPropagation(), onclick: (e) => { e.stopPropagation(); fn(); } }, label);
  const upd = (fields) => apply({ type: 'updateTrack', trackId: t.id, ...fields });
  const hasVolAuto = t.automation.some((l) => l.target === 'volume' && l.points.length && l.enabled);
  const vol = el('input', {
    type: 'range', min: 0, max: 1.5, step: 0.01, value: t.volume,
    title: hasVolAuto ? 'Volume is automated (see the lane)' : `Volume ${gainToDb(t.volume)} dB`,
    disabled: hasVolAuto,
    onmousedown: (e) => e.stopPropagation(),
    oninput: (e) => applyLive('mix:' + t.id, { type: 'updateTrack', trackId: t.id, volume: +e.target.value }),
  });
  const name = el('div', { class: 'name', title: t.name, ondblclick: (e) => { e.stopPropagation(); rename(t, name); } },
    t.name, el('span', { class: 'kind' }, KIND_LABEL[type]));
  const row = el('div', { class: 'btns' },
    type !== 'media' ? btn('M', 'm', t.mute, 'Mute', () => upd({ mute: !t.mute })) : null,
    type !== 'media' ? btn('S', 's', t.solo, 'Solo', () => upd({ solo: !t.solo })) : null,
    type !== 'media' ? btn('●', 'a', t.armed, type === 'audio' ? 'Arm for recording (mic/line input)' : 'Arm for recording (MIDI / typing keyboard)', () => upd({ armed: !t.armed })) : null,
    type === 'audio' ? btn('🎧', 'mon', t.monitor, 'Monitor input through this track', () => upd({ monitor: !t.monitor, armed: !t.monitor ? true : t.armed })) : null,
    type !== 'media' ? autoButton(t.id, t.id, t.automation) : null,
    type !== 'media' ? vol : el('span', { class: 'muted small' }, 'optional'),
  );
  return el('div', {
    class: 'th' + (t.id === store.selectedTrackId ? ' sel' : ''),
    dataset: { id: t.id },
    style: { height: `${TRACK_H}px` },
    onmousedown: () => { selectTrack(t.id); store.focus = 'timeline'; },
    oncontextmenu: (e) => { e.preventDefault(); trackMenu(t, e.clientX, e.clientY); },
  }, el('div', { class: 'stripe', style: { background: t.color } }), el('div', { class: 'body' }, name, row));
}

function laneHeader(r) {
  const info = targetInfo(r.lane.target, r.owner);
  const value = r.lane.points.length ? '' : `${info.fmt(info.def)} (not automated yet)`;
  return el('div', { class: 'th lane', style: { height: `${r.h}px` } },
    el('div', { class: 'stripe', style: { background: r.color, opacity: 0.45 } }),
    el('div', { class: 'body' },
      el('div', { class: 'name' }, info.label, el('span', { class: 'kind' }, value)),
      el('div', { class: 'btns' },
        el('label', { class: 'small muted', title: 'Enable this lane' },
          el('input', { type: 'checkbox', checked: r.lane.enabled, onchange: (e) => apply({ type: 'setAutomation', trackId: r.owner, laneId: r.lane.id, enabled: e.target.checked }) }), ' on'),
        el('button', { title: 'Clear all points', onclick: () => apply({ type: 'setAutomation', trackId: r.owner, laneId: r.lane.id, points: [] }) }, 'Clear'),
        el('button', { title: 'Remove lane', onclick: () => apply({ type: 'removeAutomationLane', trackId: r.owner, laneId: r.lane.id }) }, '✕'))));
}

function rename(t, nameEl) {
  const input = el('input', { class: 'rename', type: 'text', value: t.name });
  nameEl.replaceWith(input);
  input.focus();
  input.select();
  const done = (ok) => {
    if (ok && input.value.trim() && input.value !== t.name) apply({ type: 'updateTrack', trackId: t.id, name: input.value.trim() });
    else renderHeaders();
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter') done(true); if (e.key === 'Escape') done(false); e.stopPropagation(); });
  input.addEventListener('blur', () => done(true));
}

const COLORS = ['#e8a33d', '#4fb3d9', '#8bc34a', '#e57373', '#ba68c8', '#4db6ac', '#ffb74d', '#7986cb', '#f06292', '#3ee6ff', '#ff4fd8', '#aed581'];

function trackMenu(t, x, y) {
  const p = store.project;
  const i = p.tracks.indexOf(t);
  const type = t.kind.type;
  showMenu(x, y, [
    { label: 'Rename', action: () => { const n = inner.querySelector(`.th[data-id="${t.id}"] .name`); if (n) rename(t, n); } },
    { label: 'Change color', action: () => apply({ type: 'updateTrack', trackId: t.id, color: COLORS[(COLORS.indexOf(t.color) + 1) % COLORS.length] }) },
    { label: 'Move up', disabled: i === 0, action: () => apply({ type: 'moveTrack', trackId: t.id, index: i - 1 }) },
    { label: 'Move down', disabled: i === p.tracks.length - 1, action: () => apply({ type: 'moveTrack', trackId: t.id, index: i + 1 }) },
    type !== 'media' ? { label: 'Add automation lane…', action: () => addLaneMenu(t.id, x, y) } : null,
    'sep',
    type === 'audio' ? { label: 'Import audio onto this track…', action: () => importAudioAt(t.id, 0) } : null,
    type === 'media' ? { label: 'Add video / image…', action: () => media.pickAndAdd(t.id, 0) } : null,
    type === 'instrument' ? { label: 'Edit instrument', action: () => emit('open-tab', 'instrument') } : null,
    { label: 'Delete track', action: () => apply({ type: 'removeTrack', trackId: t.id }) },
  ]);
}

export async function importAudioAt(trackId, start) {
  const r = await command('import_audio', { paths: null, trackId, start });
  if (r) setResponse(r);
}

// ---------- drawing ----------
function clipColor(t, c) {
  if (c.content.type === 'pattern') return pattern(c.content.patternId)?.color || t.color;
  return t.color;
}

function clipName(c) {
  switch (c.content.type) {
    case 'pattern': return pattern(c.content.patternId)?.name || 'Pattern';
    default: return asset(c.content.assetId)?.name || 'Clip';
  }
}

function getPeaks(assetId) {
  const p = peaks.get(assetId);
  if (p instanceof Float32Array) return p;
  if (p !== 'loading') {
    peaks.set(assetId, 'loading');
    invoke('get_peaks', { assetId })
      .then((buf) => { peaks.set(assetId, new Float32Array(buf)); requestDraw(); })
      .catch(() => peaks.set(assetId, 'missing'));
  }
  return null;
}

/** Seconds into a clip's file at x pixels from the clip's left edge. */
function fileSecsAt(p, c, px) {
  return c.offset + tickToSecs(p, c.start + px / view.ppt) - tickToSecs(p, c.start);
}

function drawClip(ctx, t, c, x, y, w, h, selected, ghost) {
  const color = clipColor(t, c);
  ctx.save();
  ctx.globalAlpha = ghost ? 0.6 : 1;
  roundRect(ctx, x, y, w, h, 4);
  ctx.fillStyle = shade(color, -0.62);
  ctx.fill();
  ctx.save();
  ctx.clip();
  ctx.fillStyle = shade(color, -0.25);
  ctx.fillRect(x, y, w, 14);
  ctx.fillStyle = '#fff';
  ctx.font = '11px "Segoe UI", sans-serif';
  if (w > 24) ctx.fillText(clipName(c), Math.max(x, 0) + 4, y + 11);
  const cy = y + 15;
  const ch = h - 17;
  const p = store.project;
  if (c.content.type === 'pattern') {
    const pat = pattern(c.content.patternId);
    if (pat && pat.notes.length && pat.length > 0) {
      let lo = 127, hi = 0;
      for (const n of pat.notes) { lo = Math.min(lo, n.pitch); hi = Math.max(hi, n.pitch); }
      const range = Math.max(hi - lo + 1, 8);
      const nh = Math.max(2, Math.min(4, ch / range));
      ctx.fillStyle = shade(color, 0.35);
      const end = c.start + c.length;
      const vis0 = tickAtX(Math.max(0, x)) - pat.length;
      let k = Math.max(Math.floor(c.offset / pat.length), Math.floor((vis0 - c.start + c.offset) / pat.length));
      for (; ; k++) {
        const base = c.start - c.offset + k * pat.length;
        if (base >= end || xAtTick(base) > canvas.clientWidth) break;
        for (const n of pat.notes) {
          const s = base + n.start;
          if (s < c.start || s >= end || n.start >= pat.length) continue;
          const nx = xAtTick(s);
          const nw = Math.max(1.5, Math.min(n.length, end - s) * view.ppt - 1);
          const ny = cy + ((lo + range - 1 - n.pitch) / range) * (ch - nh);
          ctx.fillRect(nx, ny, nw, nh);
        }
      }
      if (pat.length < c.length) {
        ctx.fillStyle = '#ffffff22';
        for (let b = c.start - c.offset + pat.length; b < end; b += pat.length) {
          if (b > c.start) ctx.fillRect(xAtTick(b), cy, 1, ch);
        }
      }
    }
  } else if (c.content.type === 'audio') {
    const a = asset(c.content.assetId);
    const pk = a && getPeaks(a.id);
    if (pk && a.sampleRate) {
      const bins = pk.length / 2;
      const mid = cy + ch / 2;
      ctx.fillStyle = shade(color, 0.3);
      const px0 = Math.max(0, Math.floor(-x));
      const px1 = Math.min(w, canvas.clientWidth - x);
      const binsPerSec = a.sampleRate / 256;
      let s1 = fileSecsAt(p, c, px0);
      for (let px = px0; px < px1; px++) {
        const s0 = s1;
        s1 = fileSecsAt(p, c, px + 1);
        const b0 = Math.floor(s0 * binsPerSec);
        const b1 = Math.max(b0 + 1, Math.floor(s1 * binsPerSec));
        if (b0 >= bins) break;
        let mn = 0, mx = 0;
        for (let b = b0; b < Math.min(b1, bins); b++) { mn = Math.min(mn, pk[b * 2]); mx = Math.max(mx, pk[b * 2 + 1]); }
        const g = Math.min(1, c.gain);
        ctx.fillRect(x + px, mid - mx * g * ch / 2, 1, Math.max(1, (mx - mn) * g * ch / 2));
      }
    }
  } else if (c.content.type === 'media') {
    const a = asset(c.content.assetId);
    if (a) {
      const tw = Math.round(ch * 16 / 9);
      if (a.kind === 'image') {
        const img = media.getImage(a);
        if (img) for (let tx = 0; tx < w && x + tx < canvas.clientWidth; tx += tw) if (x + tx + tw > 0) ctx.drawImage(img, x + tx, cy, tw, ch);
      } else {
        const th = media.getThumbs(a);
        for (let tx = 0; tx < w && x + tx < canvas.clientWidth; tx += tw) {
          if (x + tx + tw < 0) continue;
          const secs = fileSecsAt(p, c, tx);
          const f = th && th.frames[Math.min(th.frames.length - 1, Math.floor(secs / th.interval))];
          if (f) ctx.drawImage(f, x + tx, cy, tw, ch);
          else { ctx.fillStyle = '#ffffff10'; ctx.fillRect(x + tx + 1, cy, tw - 2, ch); }
        }
      }
    }
  }
  ctx.restore();
  roundRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, 4);
  ctx.strokeStyle = selected ? '#ffffff' : shade(color, 0.05);
  ctx.lineWidth = selected ? 2 : 1;
  ctx.stroke();
  ctx.restore();
}

function gridStep(p) {
  const beat = beatTicks(p);
  const bar = barTicks(p);
  if (bar * view.ppt < 24) return { major: bar * 4, minor: bar };
  if (beat * view.ppt < 10) return { major: bar, minor: bar };
  if (beat * view.ppt > 60) return { major: bar, minor: beat / 4, mid: beat };
  return { major: bar, minor: beat };
}

/** Points of a lane being drawn (the drag's working copy if it's being edited). */
function lanePoints(r) {
  if (drag?.mode === 'lane' && drag.laneId === r.lane.id) return drag.points;
  return r.lane.points;
}

function drawLane(ctx, r, w) {
  const y0 = rowTop(r);
  const info = targetInfo(r.lane.target, r.owner);
  const pts = lanePoints(r);
  ctx.fillStyle = '#121318';
  ctx.fillRect(0, y0, w, r.h);
  const yOf = (v) => y0 + 4 + (1 - valueToNorm(info, v)) * (r.h - 8);
  ctx.strokeStyle = r.lane.enabled ? r.color : '#555a6e';
  ctx.lineWidth = 1.5;
  ctx.setLineDash(pts.length ? [] : [4, 4]);
  ctx.beginPath();
  if (!pts.length) {
    ctx.moveTo(0, yOf(info.def));
    ctx.lineTo(w, yOf(info.def));
  } else {
    ctx.moveTo(0, yOf(pts[0].value));
    for (const pt of pts) ctx.lineTo(xAtTick(pt.tick), yOf(pt.value));
    ctx.lineTo(w, yOf(pts[pts.length - 1].value));
  }
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.fillStyle = r.lane.enabled ? '#ffffff' : '#8c91a6';
  for (const pt of pts) {
    const x = xAtTick(pt.tick);
    if (x < -6 || x > w + 6) continue;
    ctx.beginPath();
    ctx.arc(x, yOf(pt.value), 3.5, 0, Math.PI * 2);
    ctx.fill();
  }
  if (drag?.mode === 'lane' && drag.laneId === r.lane.id && drag.cur) {
    const pt = drag.cur;
    ctx.fillStyle = '#fff';
    ctx.font = '11px "Segoe UI"';
    ctx.fillText(info.fmt(pt.value), xAtTick(pt.tick) + 6, Math.max(y0 + 12, yOf(pt.value) - 4));
  }
}

function tempoPoints() {
  if (drag?.mode === 'tempo') return drag.points;
  const p = store.project;
  return [{ tick: 0, bpm: p.bpm, ramp: false, base: true }, ...p.tempoChanges.map((t) => ({ ...t }))];
}

const tempoY = (r, bpm) => rowTop(r) + 4 + (1 - (clamp(bpm, TEMPO_MIN, TEMPO_MAX) - TEMPO_MIN) / (TEMPO_MAX - TEMPO_MIN)) * (r.h - 8);
const tempoAtY = (r, y) => Math.round(TEMPO_MAX - ((y - rowTop(r) - 4) / (r.h - 8)) * (TEMPO_MAX - TEMPO_MIN));

function drawTempo(ctx, r, w) {
  const y0 = rowTop(r);
  ctx.fillStyle = '#10161a';
  ctx.fillRect(0, y0, w, r.h);
  const pts = tempoPoints().sort((a, b) => a.tick - b.tick);
  ctx.strokeStyle = '#3ee6ff';
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(xAtTick(0), tempoY(r, pts[0].bpm));
  for (let i = 1; i < pts.length; i++) {
    const x = xAtTick(pts[i].tick);
    if (pts[i].ramp) ctx.lineTo(x, tempoY(r, pts[i].bpm));
    else { ctx.lineTo(x, tempoY(r, pts[i - 1].bpm)); ctx.lineTo(x, tempoY(r, pts[i].bpm)); }
  }
  ctx.lineTo(w, tempoY(r, pts[pts.length - 1].bpm));
  ctx.stroke();
  ctx.font = '11px "Segoe UI"';
  for (const pt of pts) {
    const x = xAtTick(pt.tick);
    if (x < -40 || x > w + 6) continue;
    const y = tempoY(r, pt.bpm);
    ctx.fillStyle = pt.base ? '#3ee6ff' : '#ffffff';
    ctx.beginPath();
    ctx.arc(x, y, 3.5, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = '#c9ccd8';
    ctx.fillText(`${Math.round(pt.bpm * 10) / 10}${pt.ramp ? ' ↗' : ''}`, x + 5, Math.max(y0 + 11, y - 4));
  }
}

export function draw() {
  const p = store.project;
  if (!p) return;
  needsDraw = false;
  computeRows();
  const { ctx, w, h } = fitCanvas(canvas);
  ctx.fillStyle = '#131419';
  ctx.fillRect(0, 0, w, h);
  const t0 = view.scrollX;
  const t1 = tickAtX(w);

  for (const r of rows) {
    const y = rowTop(r);
    if (y > h || y + r.h < 0) continue;
    if (r.type === 'track') {
      const i = p.tracks.indexOf(r.track);
      ctx.fillStyle = r.track.id === store.selectedTrackId ? '#1f2130' : i % 2 ? '#16171e' : '#181920';
      ctx.fillRect(0, y, w, r.h);
    } else if (r.type === 'master') {
      ctx.fillStyle = '#1a1520';
      ctx.fillRect(0, y, w, r.h);
    }
  }

  if (p.loopEnabled && p.loopEnd > p.loopStart) {
    ctx.fillStyle = '#3ee6ff0d';
    ctx.fillRect(xAtTick(p.loopStart), 0, (p.loopEnd - p.loopStart) * view.ppt, h);
  }

  const g = gridStep(p);
  for (let t = Math.floor(t0 / g.minor) * g.minor; t < t1; t += g.minor) {
    const x = Math.round(xAtTick(t)) + 0.5;
    const major = Math.abs(t % g.major) < 0.5;
    const mid = g.mid && Math.abs(t % g.mid) < 0.5;
    ctx.fillStyle = major ? '#2f3344' : mid ? '#252836' : '#1d1f29';
    ctx.fillRect(x, 0, 1, Math.min(h, totalH - view.scrollY));
  }

  for (const r of rows) {
    const y = rowTop(r);
    if (y > h || y + r.h < 0) continue;
    if (r.type === 'lane') drawLane(ctx, r, w);
    else if (r.type === 'tempo') drawTempo(ctx, r, w);
    else if (r.type === 'track') {
      for (const c of r.track.clips) {
        if (drag?.mode !== 'lane' && drag?.clipId === c.id && !drag.copy && drag.moved) continue;
        const x = xAtTick(c.start);
        const cw = c.length * view.ppt;
        if (x > w || x + cw < 0) continue;
        const sel = store.selectedClip?.clipId === c.id;
        drawClip(ctx, r.track, c, x, y + 2, Math.max(cw, 3), r.h - 5, sel, false);
      }
    } else if (r.type === 'master') {
      ctx.fillStyle = '#8c91a6';
      ctx.font = '11px "Segoe UI"';
      ctx.fillText('Master bus (automation: A on the left)', 8, y + 19);
    }
    ctx.fillStyle = '#262938';
    ctx.fillRect(0, y + r.h - 1, w, 1);
  }

  if (drag && drag.clipId && drag.moved) {
    const pv = drag.preview;
    const r = rows.find((x) => x.type === 'track' && x.track.id === pv.trackId);
    if (r) {
      const c = { ...drag.clip, start: pv.start, length: pv.length, offset: pv.offset };
      drawClip(ctx, r.track, c, xAtTick(pv.start), rowTop(r) + 2, Math.max(pv.length * view.ppt, 3), r.h - 5, true, true);
    }
  }

  const ph = xAtTick(currentTick());
  if (ph >= 0 && ph <= w) {
    ctx.fillStyle = store.transport.recording ? '#ff3b4e' : '#3ee6ff';
    ctx.fillRect(Math.round(ph), 0, 1.5, h);
  }
  drawRuler();
}

function drawRuler() {
  const p = store.project;
  const { ctx, w, h } = fitCanvas(ruler);
  ctx.fillStyle = '#1c1e26';
  ctx.fillRect(0, 0, w, h);
  ctx.fillStyle = '#15161c';
  ctx.fillRect(0, 0, w, LOOP_H);
  const lp = rulerDrag?.mode === 'loop' ? rulerDrag : null;
  const ls = lp ? Math.min(lp.a, lp.b) : p.loopStart;
  const le = lp ? Math.max(lp.a, lp.b) : p.loopEnd;
  if (le > ls) {
    ctx.fillStyle = p.loopEnabled || lp ? '#3ee6ff' : '#3ee6ff44';
    ctx.fillRect(xAtTick(ls), 1, (le - ls) * view.ppt, LOOP_H - 2);
  }
  const bar = barTicks(p);
  const step = bar * view.ppt < 28 ? bar * 4 : bar;
  ctx.font = '11px "Segoe UI", sans-serif';
  for (let t = Math.floor(view.scrollX / step) * step; t < tickAtX(w); t += step) {
    const x = Math.round(xAtTick(t)) + 0.5;
    ctx.fillStyle = '#454a5e';
    ctx.fillRect(x, LOOP_H, 1, h - LOOP_H);
    ctx.fillStyle = '#c9ccd8';
    ctx.fillText(String(Math.round(t / bar) + 1), x + 3, h - 6);
  }
  const beat = beatTicks(p);
  if (beat * view.ppt > 8) {
    ctx.fillStyle = '#343848';
    for (let t = Math.floor(view.scrollX / beat) * beat; t < tickAtX(w); t += beat) {
      if (Math.abs(t % bar) < 0.5) continue;
      ctx.fillRect(Math.round(xAtTick(t)), h - 7, 1, 7);
    }
  }
  if (p.tempoChanges.length) {
    ctx.fillStyle = '#3ee6ff99';
    for (const tc of p.tempoChanges) ctx.fillRect(Math.round(xAtTick(tc.tick)), LOOP_H, 2, 6);
  }
  const ph = xAtTick(currentTick());
  ctx.fillStyle = store.transport.recording ? '#ff3b4e' : '#3ee6ff';
  ctx.beginPath();
  ctx.moveTo(ph - 6, LOOP_H);
  ctx.lineTo(ph + 6, LOOP_H);
  ctx.lineTo(ph, LOOP_H + 9);
  ctx.fill();
  ctx.fillRect(Math.round(ph), LOOP_H, 1.5, h);
  ctx.fillStyle = '#343848';
  ctx.fillRect(0, h - 1, w, 1);
}

let lastFrameTick = 0;
/** Called every animation frame by main.js. */
export function frame(tick) {
  if (!store.transport.playing && tick !== lastFrameTick && !drag && !rulerDrag) {
    // Playhead jumped while stopped (stop, Home, undo of a recording...): keep it in view.
    const x = xAtTick(tick);
    if (x < 0 || x > canvas.clientWidth) view.scrollX = Math.max(0, tick - 40 / view.ppt);
    needsDraw = true;
  }
  lastFrameTick = tick;
  if (store.transport.playing && !drag) {
    const w = canvas.clientWidth;
    const x = xAtTick(tick);
    if (x > w * 0.92 || x < 0) view.scrollX = Math.max(0, tick - (w * 0.08) / view.ppt);
    needsDraw = true;
  }
  if (needsDraw) draw();
}

// ---------- mouse ----------
function onWheel(e) {
  e.preventDefault();
  const r = canvas.getBoundingClientRect();
  const x = e.clientX - r.left;
  if (e.ctrlKey) {
    const tick = tickAtX(x);
    view.ppt = clamp(view.ppt * (e.deltaY < 0 ? 1.15 : 1 / 1.15), 0.01, 4);
    view.scrollX = Math.max(0, tick - x / view.ppt);
  } else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
    view.scrollX = Math.max(0, view.scrollX + (e.deltaX || e.deltaY) / view.ppt);
  } else {
    const max = Math.max(0, totalH - canvas.clientHeight + 40);
    view.scrollY = clamp(view.scrollY + e.deltaY, 0, max);
    inner.style.transform = `translateY(${-view.scrollY}px)`;
  }
  requestDraw();
}

function nearPoint(pts, r, x, y, yOf) {
  let best = -1, bd = 8;
  pts.forEach((pt, i) => {
    const d = Math.hypot(xAtTick(pt.tick) - x, yOf(pt) - y);
    if (d < bd) { bd = d; best = i; }
  });
  return best;
}

function onHover(e) {
  if (drag) return;
  const h = hit(e.offsetX, e.offsetY);
  const type = h.row?.type;
  if (type === 'lane' || type === 'tempo') { canvas.style.cursor = 'crosshair'; return; }
  canvas.style.cursor = h.c ? (h.edge ? 'ew-resize' : 'grab') : h.t?.kind.type === 'instrument' ? 'copy' : 'default';
}

async function onMouseDown(e) {
  store.focus = 'timeline';
  if (e.button !== 0) return;
  const p = store.project;
  const h = hit(e.offsetX, e.offsetY);
  if (h.row?.type === 'lane') return laneDown(e, h.row);
  if (h.row?.type === 'tempo') return tempoDown(e, h.row);
  if (h.t) selectTrack(h.t.id);
  if (h.c) {
    store.selectedClip = { trackId: h.t.id, clipId: h.c.id };
    if (h.c.content.type === 'pattern') selectPattern(h.c.content.patternId);
    else emit('selection');
    drag = {
      mode: h.edge === 'right' ? 'resize' : h.edge === 'left' ? 'trim' : 'move',
      trackId: h.t.id,
      clipId: h.c.id,
      clip: h.c,
      x0: e.clientX,
      y0: e.clientY,
      tick0: h.tick,
      copy: e.ctrlKey,
      moved: false,
      preview: { trackId: h.t.id, start: h.c.start, length: h.c.length, offset: h.c.offset },
    };
    requestDraw();
    return;
  }
  store.selectedClip = null;
  if (h.t?.kind.type === 'instrument') {
    // Paint the selected pattern, creating one if the project has none.
    let pid = store.selectedPatternId;
    if (!pattern(pid)) {
      pid = await apply({ type: 'addPattern', length: barTicks(p) });
      if (!pid) return;
      selectPattern(pid);
    }
    const pat = pattern(pid);
    const start = Math.max(0, snapFloor(h.tick));
    await apply({ type: 'addClip', trackId: h.t.id, start, length: pat.length, content: { type: 'pattern', patternId: pid } });
  }
  requestDraw();
}

function laneDown(e, r) {
  const info = targetInfo(r.lane.target, r.owner);
  const yOf = (pt) => rowTop(r) + 4 + (1 - valueToNorm(info, pt.value)) * (r.h - 8);
  const points = r.lane.points.map((pt) => ({ ...pt }));
  let idx = nearPoint(points, r, e.offsetX, e.offsetY, yOf);
  if (idx < 0) {
    const tick = Math.max(0, e.altKey ? Math.round(tickAtX(e.offsetX)) : snapRound(tickAtX(e.offsetX)));
    points.push({ tick, value: normToValue(info, 1 - (e.offsetY - rowTop(r) - 4) / (r.h - 8)) });
    idx = points.length - 1;
  }
  drag = { mode: 'lane', laneId: r.lane.id, owner: r.owner, row: r, info, points, cur: points[idx], changed: true };
  requestDraw();
}

function tempoDown(e, r) {
  const points = tempoPoints();
  let idx = nearPoint(points, r, e.offsetX, e.offsetY, (pt) => tempoY(r, pt.bpm));
  if (idx < 0) {
    const tick = Math.max(1, e.altKey ? Math.round(tickAtX(e.offsetX)) : snapRound(tickAtX(e.offsetX)));
    points.push({ tick, bpm: clamp(tempoAtY(r, e.offsetY), 20, 400), ramp: false });
    idx = points.length - 1;
  }
  drag = { mode: 'tempo', row: r, points, cur: points[idx], changed: true };
  requestDraw();
}

function onMouseMove(e) {
  if (rulerDrag) return onRulerMove(e);
  if (!drag) return;
  const p = store.project;
  const r = canvas.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  if (drag.mode === 'lane' || drag.mode === 'tempo') {
    const pt = drag.cur;
    const tick = Math.max(0, e.altKey ? Math.round(tickAtX(x)) : snapRound(tickAtX(x)));
    if (drag.mode === 'lane') {
      pt.tick = tick;
      pt.value = normToValue(drag.info, 1 - (y - rowTop(drag.row) - 4) / (drag.row.h - 8));
    } else {
      if (!pt.base) pt.tick = Math.max(1, tick);
      pt.bpm = clamp(tempoAtY(drag.row, y), 20, 400);
    }
    requestDraw();
    return;
  }
  if (!drag.moved && Math.abs(e.clientX - drag.x0) < 3 && Math.abs(e.clientY - drag.y0) < 3) return;
  drag.moved = true;
  const tick = tickAtX(x);
  const delta = tick - drag.tick0;
  const c = drag.clip;
  const s = e.altKey ? 1 : snap();
  const pv = drag.preview;
  if (drag.mode === 'move') {
    pv.start = Math.max(0, Math.round((c.start + delta) / s) * s);
    const row = rowAtY(y);
    const src = track(drag.trackId);
    if (row?.type === 'track' && src && row.track.kind.type === src.kind.type) pv.trackId = row.track.id;
  } else if (drag.mode === 'resize') {
    const end = Math.max(c.start + s, Math.round((c.start + c.length + delta) / s) * s);
    pv.length = Math.max(1, end - c.start);
  } else if (drag.mode === 'trim') {
    const end = c.start + c.length;
    let ns = Math.min(end - Math.max(1, s), Math.max(0, Math.round((c.start + delta) / s) * s));
    let off;
    if (c.content.type === 'pattern') {
      off = c.offset + (ns - c.start);
      if (off < 0) { ns -= off; off = 0; }
    } else {
      off = c.offset + tickToSecs(p, ns) - tickToSecs(p, c.start);
      if (off < 0) { ns = Math.max(0, Math.round(secsToTick(p, tickToSecs(p, c.start) - c.offset))); off = 0; }
    }
    pv.start = ns;
    pv.length = end - ns;
    pv.offset = off;
  }
  requestDraw();
}

async function onMouseUp(e) {
  if (rulerDrag) return onRulerUp(e);
  if (!drag) return;
  const d = drag;
  drag = null;
  if (d.mode === 'lane') {
    const points = d.points.map((pt) => ({ tick: Math.round(pt.tick), value: pt.value })).sort((a, b) => a.tick - b.tick);
    await apply({ type: 'setAutomation', trackId: d.owner, laneId: d.laneId, points });
    return requestDraw();
  }
  if (d.mode === 'tempo') return commitTempo(d.points);
  if (!d.moved) return requestDraw();
  const pv = d.preview;
  if (d.copy) {
    const id = await apply({ type: 'addClip', trackId: pv.trackId, start: pv.start, length: pv.length, offset: pv.offset, content: d.clip.content });
    if (id) store.selectedClip = { trackId: pv.trackId, clipId: id };
  } else {
    await apply({
      type: 'updateClip', trackId: d.trackId, clipId: d.clipId,
      start: pv.start, length: pv.length, offset: pv.offset,
      toTrackId: pv.trackId !== d.trackId ? pv.trackId : undefined,
    });
    if (pv.trackId !== d.trackId) store.selectedClip = { trackId: pv.trackId, clipId: d.clipId };
  }
  requestDraw();
}

function commitTempo(points) {
  const base = points.find((pt) => pt.base);
  const rest = points.filter((pt) => !pt.base).map((pt) => ({ tick: Math.round(pt.tick), bpm: pt.bpm, ramp: !!pt.ramp }));
  const edits = [];
  if (base && base.bpm !== store.project.bpm) edits.push({ type: 'setBpm', bpm: base.bpm });
  edits.push({ type: 'setTempo', points: rest });
  return apply({ type: 'batch', edits }).then(requestDraw);
}

function onDblClick(e) {
  const h = hit(e.offsetX, e.offsetY);
  if (h.row?.type === 'lane') {
    const r = h.row;
    const info = targetInfo(r.lane.target, r.owner);
    const idx = nearPoint(r.lane.points, r, e.offsetX, e.offsetY, (pt) => rowTop(r) + 4 + (1 - valueToNorm(info, pt.value)) * (r.h - 8));
    if (idx < 0) return;
    const v = prompt(`${info.label} value (${info.min} to ${info.max})`, String(Math.round(r.lane.points[idx].value * 1000) / 1000));
    if (v === null || !Number.isFinite(+v)) return;
    const points = r.lane.points.map((pt, i) => (i === idx ? { ...pt, value: clamp(+v, info.min, info.max) } : pt));
    apply({ type: 'setAutomation', trackId: r.owner, laneId: r.lane.id, points });
    return;
  }
  if (h.row?.type === 'tempo') {
    const r = h.row;
    const pts = tempoPoints();
    const idx = nearPoint(pts, r, e.offsetX, e.offsetY, (pt) => tempoY(r, pt.bpm));
    if (idx < 0) return;
    const pt = pts[idx];
    const v = prompt(pt.base ? 'Starting BPM' : 'BPM at this point. Add "ramp" to glide into it from the previous tempo (e.g. "90 ramp").', `${pt.bpm}${pt.ramp ? ' ramp' : ''}`);
    if (v === null) return;
    const bpm = parseFloat(v);
    if (!Number.isFinite(bpm)) return;
    pt.bpm = clamp(bpm, 20, 400);
    if (!pt.base) pt.ramp = /ramp/i.test(v);
    commitTempo(pts);
    return;
  }
  if (h.c?.content.type === 'pattern') {
    selectPattern(h.c.content.patternId);
    emit('open-tab', 'pianoroll');
  } else if (h.c?.content.type === 'audio' || h.c?.content.type === 'media') {
    emit('open-tab', 'mixer');
  }
}

function onContextMenu(e) {
  e.preventDefault();
  const h = hit(e.offsetX, e.offsetY);
  const p = store.project;
  if (h.row?.type === 'lane') {
    const r = h.row;
    const info = targetInfo(r.lane.target, r.owner);
    const idx = nearPoint(r.lane.points, r, e.offsetX, e.offsetY, (pt) => rowTop(r) + 4 + (1 - valueToNorm(info, pt.value)) * (r.h - 8));
    if (idx >= 0) {
      apply({ type: 'setAutomation', trackId: r.owner, laneId: r.lane.id, points: r.lane.points.filter((_, i) => i !== idx) });
    } else {
      showMenu(e.clientX, e.clientY, [
        { label: 'Clear lane', action: () => apply({ type: 'setAutomation', trackId: r.owner, laneId: r.lane.id, points: [] }) },
        { label: 'Remove lane', action: () => apply({ type: 'removeAutomationLane', trackId: r.owner, laneId: r.lane.id }) },
      ]);
    }
    return;
  }
  if (h.row?.type === 'tempo') {
    const pts = tempoPoints();
    const idx = nearPoint(pts, h.row, e.offsetX, e.offsetY, (pt) => tempoY(h.row, pt.bpm));
    if (idx >= 0 && !pts[idx].base) commitTempo(pts.filter((_, i) => i !== idx));
    else if (idx < 0) showMenu(e.clientX, e.clientY, [{ label: 'Remove all tempo changes', action: () => commitTempo([pts[0]]) }]);
    return;
  }
  if (h.row?.type === 'master') {
    addLaneMenu(null, e.clientX, e.clientY);
    return;
  }
  if (h.c) {
    const t = h.t, c = h.c;
    store.selectedClip = { trackId: t.id, clipId: c.id };
    requestDraw();
    const del = { label: 'Delete', key: 'Del', action: () => apply({ type: 'removeClip', trackId: t.id, clipId: c.id }) };
    const dup = { label: 'Duplicate after', key: 'Ctrl+D', action: () => duplicateClip(t, c) };
    const items = [];
    if (c.content.type === 'pattern') {
      const pat = pattern(c.content.patternId);
      items.push(
        { label: 'Edit in piano roll', action: () => { selectPattern(pat.id); emit('open-tab', 'pianoroll'); } },
        { label: 'Make unique (copy pattern)', action: () => makeUnique(t, c) },
        { label: 'Fit length to pattern', disabled: !pat, action: () => apply({ type: 'updateClip', trackId: t.id, clipId: c.id, length: pat.length }) },
      );
    } else if (c.content.type === 'audio') {
      items.push(
        { label: `Clip gain +3 dB (now ${gainToDb(c.gain)} dB)`, action: () => apply({ type: 'updateClip', trackId: t.id, clipId: c.id, gain: c.gain * 1.4125 }) },
        { label: 'Clip gain −3 dB', action: () => apply({ type: 'updateClip', trackId: t.id, clipId: c.id, gain: c.gain / 1.4125 }) },
        { label: 'Reset length to file', action: () => { const a = asset(c.content.assetId); if (a) apply({ type: 'updateClip', trackId: t.id, clipId: c.id, length: durationTicks(p, c.start, a.duration - c.offset) }); } },
      );
    } else if (c.content.type === 'media') {
      const a = asset(c.content.assetId);
      if (a?.kind === 'video') items.push({ label: "Extract video's audio to a new track", action: () => media.extractAudio(a.id) });
      if (a?.kind === 'video') items.push({ label: 'Reset length to video', action: () => apply({ type: 'updateClip', trackId: t.id, clipId: c.id, length: durationTicks(p, c.start, a.duration - c.offset) }) });
    }
    items.push('sep', dup, del);
    showMenu(e.clientX, e.clientY, items);
    return;
  }
  if (!h.t) return;
  const start = Math.max(0, snapFloor(h.tick));
  const type = h.t.kind.type;
  showMenu(e.clientX, e.clientY, [
    type === 'instrument' ? { label: 'New pattern here', action: () => newPatternClip(h.t.id, start) } : null,
    type === 'audio' ? { label: 'Import audio here…', action: () => importAudioAt(h.t.id, start) } : null,
    type === 'media' ? { label: 'Add video / image here…', action: () => media.pickAndAdd(h.t.id, start) } : null,
    type !== 'media' ? { label: 'Add automation lane…', action: () => addLaneMenu(h.t.id, e.clientX, e.clientY) } : null,
    { label: 'Set playhead here', action: () => seekTo(start) },
  ]);
}

export async function newPatternClip(trackId, start) {
  const p = store.project;
  const pid = await apply({ type: 'addPattern', length: barTicks(p) });
  if (!pid) return;
  selectPattern(pid);
  await apply({ type: 'addClip', trackId, start, length: barTicks(p), content: { type: 'pattern', patternId: pid } });
  emit('open-tab', 'pianoroll');
}

async function makeUnique(t, c) {
  const pid = await apply({ type: 'clonePattern', patternId: c.content.patternId });
  if (!pid) return;
  await apply({
    type: 'batch',
    edits: [
      { type: 'removeClip', trackId: t.id, clipId: c.id },
      { type: 'addClip', trackId: t.id, start: c.start, length: c.length, offset: c.offset, content: { type: 'pattern', patternId: pid } },
    ],
  });
  selectPattern(pid);
}

function duplicateClip(t, c) {
  apply({ type: 'addClip', trackId: t.id, start: c.start + c.length, length: c.length, offset: c.offset, content: c.content });
}

export function seekTo(tick) {
  tick = Math.max(0, tick);
  store.transport.tick = tick;
  store.transport.at = performance.now();
  invoke('seek', { tick }).catch(() => {});
  requestDraw();
  emit('seek', tick);
}

// ---------- ruler ----------
function onRulerDown(e) {
  if (e.button !== 0) return;
  const tick = Math.max(0, tickAtX(e.offsetX));
  if (e.offsetY < LOOP_H) {
    const s = snapRound(tick, Math.max(snap(), beatTicks(store.project)));
    rulerDrag = { mode: 'loop', a: s, b: s };
  } else {
    rulerDrag = { mode: 'seek' };
    seekTo(e.shiftKey ? tick : snapFloor(tick));
  }
  requestDraw();
}

let lastSeek = 0;
function onRulerMove(e) {
  const r = ruler.getBoundingClientRect();
  const tick = Math.max(0, tickAtX(e.clientX - r.left));
  if (rulerDrag.mode === 'loop') {
    rulerDrag.b = snapRound(tick, Math.max(snap(), beatTicks(store.project)));
  } else if (performance.now() - lastSeek > 30) {
    lastSeek = performance.now();
    seekTo(e.shiftKey ? tick : snapFloor(tick));
  }
  requestDraw();
}

function onRulerUp() {
  const d = rulerDrag;
  rulerDrag = null;
  if (d.mode === 'loop') {
    const p = store.project;
    if (d.a === d.b) apply({ type: 'setLoop', start: p.loopStart, end: p.loopEnd, enabled: !p.loopEnabled });
    else apply({ type: 'setLoop', start: Math.min(d.a, d.b), end: Math.max(d.a, d.b), enabled: true });
  }
  requestDraw();
}

// ---------- keyboard + drops ----------
export function onKey(e) {
  const sc = store.selectedClip;
  const t = sc && track(sc.trackId);
  const c = t?.clips.find((x) => x.id === sc.clipId);
  if ((e.key === 'Delete' || e.key === 'Backspace') && c) {
    apply({ type: 'removeClip', trackId: t.id, clipId: c.id });
    return true;
  }
  if (e.ctrlKey && e.key.toLowerCase() === 'd' && c) {
    duplicateClip(t, c);
    return true;
  }
  if (e.key === 'Escape') {
    store.selectedClip = null;
    requestDraw();
    return true;
  }
  return false;
}

/** Drop of a sidebar item (pattern or asset) at a screen position. */
export async function dropItem(item, clientX, clientY) {
  const loc = locate(clientX, clientY);
  if (!loc) return false;
  const p = store.project;
  const start = Math.max(0, snapFloor(loc.tick));
  if (item.type === 'pattern') {
    const pat = pattern(item.id);
    const t = loc.track?.kind.type === 'instrument' ? loc.track : track(store.selectedTrackId);
    if (!t || t.kind.type !== 'instrument') { toast('Drop patterns onto an instrument track', 'warn'); return true; }
    await apply({ type: 'addClip', trackId: t.id, start, length: pat.length, content: { type: 'pattern', patternId: pat.id } });
    return true;
  }
  if (item.type === 'asset') {
    const a = asset(item.id);
    if (!a) return true;
    if (a.kind === 'audio') {
      const length = durationTicks(p, start, a.duration);
      if (loc.track?.kind.type === 'audio') {
        await apply({ type: 'addClip', trackId: loc.track.id, start, length, content: { type: 'audio', assetId: a.id } });
      } else {
        const tid = await apply({ type: 'addTrack', kind: 'audio', name: a.name });
        if (tid) await apply({ type: 'addClip', trackId: tid, start, length, content: { type: 'audio', assetId: a.id } });
      }
    } else {
      const r = await command('add_media', { path: a.path, kind: a.kind, duration: a.duration, start, trackId: loc.track?.kind.type === 'media' ? loc.track.id : null });
      setResponse(r);
    }
    return true;
  }
  return false;
}

export function scrollToTick(tick) {
  const w = canvas.clientWidth;
  if (xAtTick(tick) < 0 || xAtTick(tick) > w) view.scrollX = Math.max(0, tick - 40 / view.ppt);
  requestDraw();
}

/** Current BPM at the playhead (for display). */
export const bpmNow = () => bpmAt(store.project, currentTick());
