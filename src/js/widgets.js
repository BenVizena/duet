// Reusable parameter controls. Each returns { el, set(value), isActive() }.
import { el, clamp } from './util.js';

let activeCount = 0;
/** True while the user is dragging any slider (so re-renders don't fight the drag). */
export const interacting = () => activeCount > 0;

export const fmt = {
  secs: (v) => (v < 1 ? `${Math.round(v * 1000)} ms` : `${v.toFixed(2)} s`),
  hz: (v) => (v >= 1000 ? `${(v / 1000).toFixed(1)} kHz` : `${v < 10 ? v.toFixed(2) : Math.round(v)} Hz`),
  st: (v) => `${v > 0 ? '+' : ''}${(+v).toFixed(2)} st`,
  int: (v) => String(Math.round(v)),
  pct: (v) => `${Math.round(v * 100)}%`,
  db: (v) => `${v > 0 ? '+' : ''}${(+v).toFixed(1)} dB`,
  cents: (v) => `${Math.round(v)} ct`,
  oct: (v) => `${v > 0 ? '+' : ''}${(+v).toFixed(2)} oct`,
  x: (v) => `${(+v).toFixed(2)}×`,
  ms: (v) => `${v < 10 ? (+v).toFixed(1) : Math.round(v)} ms`,
  num: (v) => (+v).toFixed(2),
};

function mapper(min, max, curve) {
  if (curve === 'log') {
    const a = Math.log(min), b = Math.log(max);
    return { toPos: (v) => (Math.log(clamp(v, min, max)) - a) / (b - a), toVal: (t) => Math.exp(a + t * (b - a)) };
  }
  if (curve === 'sq') {
    return { toPos: (v) => Math.sqrt(clamp((v - min) / (max - min), 0, 1)), toVal: (t) => min + (max - min) * t * t };
  }
  return { toPos: (v) => (clamp(v, min, max) - min) / (max - min), toVal: (t) => min + (max - min) * t };
}

export function range({ label, min, max, step = 0, curve, value, format = fmt.num, onInput, title, compact }) {
  const m = mapper(min, max, curve);
  const input = el('input', { type: 'range', min: 0, max: 1000, step: 1, title: title || label });
  const val = el('span', { class: 'val' });
  const quant = (v) => (step ? Math.round(v / step) * step : v);
  let active = false;
  const set = (v) => {
    input.value = Math.round(m.toPos(v) * 1000);
    val.textContent = format(v);
    lbl && (lbl.title = `${label}: ${format(v)}`);
  };
  input.addEventListener('pointerdown', () => { active = true; activeCount++; });
  const end = () => { if (active) { active = false; activeCount--; } };
  input.addEventListener('pointerup', end);
  input.addEventListener('lostpointercapture', end);
  input.addEventListener('input', () => {
    const v = quant(m.toVal(input.value / 1000));
    val.textContent = format(v);
    onInput(v);
  });
  input.addEventListener('dblclick', () => {
    // Double-click resets to the value the control was built with.
    set(value);
    onInput(value);
  });
  let lbl = null;
  let root;
  if (compact) {
    lbl = el('label', {}, el('span', {}, label), val);
    root = el('div', { class: 'p' }, lbl, input);
  } else {
    lbl = el('label', {}, label);
    root = el('div', { class: 'ctl' }, lbl, input, val);
  }
  set(value);
  return { el: root, set, isActive: () => active };
}

export function choice({ label, options, value, onChange, compact }) {
  const wrap = el('div', { class: 'choices' });
  const btns = options.map(([v, l]) => {
    const b = el('button', { onclick: () => { set(v); onChange(v); } }, l);
    b.dataset.v = String(v);
    wrap.append(b);
    return b;
  });
  const set = (v) => btns.forEach((b) => b.classList.toggle('on', b.dataset.v === String(v)));
  set(value);
  const root = compact
    ? el('div', { class: 'p' }, el('label', {}, label), wrap)
    : el('div', { class: 'ctl wide' }, el('label', {}, label), wrap);
  return { el: root, set, isActive: () => false };
}

export function toggle({ label, value, onChange, compact }) {
  return choice({ label, options: [[false, 'Off'], [true, 'On']], value, onChange, compact });
}
