export const PPQ = 96;
export const STEP = PPQ / 4;

export const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
export const noteName = (p) => NOTE_NAMES[p % 12] + (Math.floor(p / 12) - 1);
export const isBlack = (p) => [1, 3, 6, 8, 10].includes(p % 12);

export const SCALES = {
  major: { name: 'Major', steps: [0, 2, 4, 5, 7, 9, 11] },
  minor: { name: 'Minor', steps: [0, 2, 3, 5, 7, 8, 10] },
  harmonicMinor: { name: 'Harmonic minor', steps: [0, 2, 3, 5, 7, 8, 11] },
  dorian: { name: 'Dorian', steps: [0, 2, 3, 5, 7, 9, 10] },
  phrygian: { name: 'Phrygian', steps: [0, 1, 3, 5, 7, 8, 10] },
  lydian: { name: 'Lydian', steps: [0, 2, 4, 6, 7, 9, 11] },
  mixolydian: { name: 'Mixolydian', steps: [0, 2, 4, 5, 7, 9, 10] },
  majorPent: { name: 'Major pentatonic', steps: [0, 2, 4, 7, 9] },
  minorPent: { name: 'Minor pentatonic', steps: [0, 3, 5, 7, 10] },
  blues: { name: 'Blues', steps: [0, 3, 5, 6, 7, 10] },
  chromatic: { name: 'Chromatic', steps: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] },
};

export function inScale(pitch, root, scale) {
  const s = SCALES[scale] || SCALES.major;
  return s.steps.includes((((pitch - root) % 12) + 12) % 12);
}

export const beatTicks = (p) => (PPQ * 4) / (p.timeSigDen || 4);
export const barTicks = (p) => beatTicks(p) * (p.timeSigNum || 4);
// Tempo map (mirrors model.rs TempoMap): constant segments, ramps split into short steps.
const tempoCache = new WeakMap();
export function tempoMap(p) {
  let m = tempoCache.get(p);
  if (m) return m;
  const base = clamp(p.bpm, 20, 400);
  const raw = [[0, base]];
  let pt = 0, pb = base;
  for (const x of [...(p.tempoChanges || [])].sort((a, b) => a.tick - b.tick)) {
    const b = clamp(x.bpm, 20, 400);
    if (x.ramp && x.tick > pt) {
      const n = Math.max(1, Math.ceil((x.tick - pt) / (PPQ / 8)));
      for (let k = 0; k < n; k++) raw.push([pt + ((x.tick - pt) * k) / n, pb + (b - pb) * ((k + 0.5) / n)]);
    }
    raw.push([x.tick, b]);
    pt = x.tick;
    pb = b;
  }
  const segs = [];
  for (const [t, b] of raw) {
    const last = segs[segs.length - 1];
    if (last && Math.abs(last.tick - t) < 1e-9) last.bpm = b;
    else if (last) segs.push({ tick: t, bpm: b, secs: last.secs + ((t - last.tick) * 60) / (last.bpm * PPQ) });
    else segs.push({ tick: t, bpm: b, secs: 0 });
  }
  m = { segs };
  tempoCache.set(p, m);
  return m;
}
function segAt(segs, key, v) {
  let lo = 0, hi = segs.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (segs[mid][key] <= v) lo = mid; else hi = mid - 1;
  }
  return segs[lo];
}
export const bpmAt = (p, tick) => segAt(tempoMap(p).segs, 'tick', tick).bpm;
export function tickToSecs(p, tick) {
  const s = segAt(tempoMap(p).segs, 'tick', tick);
  return s.secs + ((tick - s.tick) * 60) / (s.bpm * PPQ);
}
export function secsToTick(p, secs) {
  const s = segAt(tempoMap(p).segs, 'secs', secs);
  return s.tick + ((secs - s.secs) * s.bpm * PPQ) / 60;
}
/** Ticks spanned by `secs` of real time starting at `start`. */
export const durationTicks = (p, start, secs) => Math.max(1, Math.round(secsToTick(p, tickToSecs(p, start) + secs) - start));

export function snapTicks(snap, p) {
  const beat = beatTicks(p);
  switch (snap) {
    case 'bar': return barTicks(p);
    case 'beat': return beat;
    case '1/2': return beat / 2;
    case '1/3': return beat / 3;
    case '1/4': return beat / 4;
    case '1/6': return beat / 6;
    case '1/8': return beat / 8;
    default: return 1;
  }
}

export function formatBars(tick, p) {
  const bar = barTicks(p);
  const beat = beatTicks(p);
  const t = Math.max(0, Math.floor(tick));
  const b = Math.floor(t / bar);
  const be = Math.floor((t % bar) / beat);
  const tk = Math.floor(t % beat);
  return `${b + 1}.${be + 1}.${String(tk).padStart(2, '0')}`;
}

export function formatTime(secs) {
  secs = Math.max(0, secs);
  const m = Math.floor(secs / 60);
  const s = secs - m * 60;
  return `${m}:${s.toFixed(1).padStart(4, '0')}`;
}

export const clamp = (v, a, b) => Math.min(b, Math.max(a, v));

export function gainToDb(g) {
  return g <= 0.00001 ? '-inf' : (20 * Math.log10(g)).toFixed(1);
}

/** Tiny DOM builder: el('div', {class: 'x', onclick}, child, 'text') */
export function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else if (k === 'class') e.className = v;
    else if (k === 'style' && typeof v === 'object') Object.assign(e.style, v);
    else if (k === 'dataset') Object.assign(e.dataset, v);
    else if (k in e && k !== 'list') e[k] = v;
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    e.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return e;
}

export function toast(msg, kind = 'info', ms = 4500) {
  const t = el('div', { class: `toast ${kind}` }, msg);
  document.getElementById('toasts').append(t);
  setTimeout(() => t.remove(), kind === 'error' ? ms * 2 : ms);
}

/** Context menu. items: [{label, action, key, disabled}] or 'sep' */
export function showMenu(x, y, items) {
  closeMenu();
  const root = document.getElementById('menu-root');
  const m = el('div', { class: 'menu' });
  for (const it of items) {
    if (it === 'sep') { m.append(el('div', { class: 'msep' })); continue; }
    if (!it) continue;
    m.append(el('div', {
      class: 'mi' + (it.disabled ? ' disabled' : ''),
      onclick: () => { closeMenu(); it.action(); },
    }, el('span', {}, it.label), it.key ? el('span', { class: 'k' }, it.key) : null));
  }
  root.append(m);
  const r = m.getBoundingClientRect();
  m.style.left = Math.min(x, window.innerWidth - r.width - 4) + 'px';
  m.style.top = Math.min(y, window.innerHeight - r.height - 4) + 'px';
  setTimeout(() => window.addEventListener('mousedown', onAway, { capture: true }), 0);
}
function onAway(e) {
  if (!e.target.closest('.menu')) closeMenu();
}
export function closeMenu() {
  document.getElementById('menu-root').innerHTML = '';
  window.removeEventListener('mousedown', onAway, { capture: true });
}

/** Resizes a canvas to its CSS size × devicePixelRatio; returns the 2D context scaled to CSS px. */
export function fitCanvas(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
  }
  const ctx = canvas.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h };
}

export function roundRect(ctx, x, y, w, h, r) {
  r = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

export const deepClone = (o) => JSON.parse(JSON.stringify(o));

export function isTypingTarget(e) {
  const t = e.target;
  return t && (t.tagName === 'INPUT' && !['range', 'checkbox', 'button'].includes(t.type) || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
}

/** Lightens/darkens a #rrggbb color by amount (-1..1). */
export function shade(hex, amt) {
  const n = parseInt((hex || '#888888').slice(1), 16);
  let r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255;
  const f = (c) => Math.round(amt >= 0 ? c + (255 - c) * amt : c * (1 + amt));
  r = f(r); g = f(g); b = f(b);
  return `rgb(${r},${g},${b})`;
}
