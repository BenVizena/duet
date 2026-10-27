// Mixer: one strip per audio-producing track plus the master bus, each with an effect chain.
import { store, on, apply, applyLive } from './store.js';
import { el, gainToDb, deepClone } from './util.js';
import { range, choice, toggle, fmt, interacting } from './widgets.js';

const FX = {
  eq: {
    name: 'EQ',
    params: [
      { k: 'low', label: 'Low', min: -18, max: 18, step: 0.5, format: fmt.db },
      { k: 'mid', label: 'Mid', min: -18, max: 18, step: 0.5, format: fmt.db },
      { k: 'midFreq', label: 'Mid freq', min: 100, max: 8000, curve: 'log', format: fmt.hz },
      { k: 'high', label: 'High', min: -18, max: 18, step: 0.5, format: fmt.db },
    ],
  },
  filter: {
    name: 'Filter',
    params: [
      { k: 'mode', type: 'choice', label: 'Mode', options: [['lowpass', 'LP'], ['highpass', 'HP'], ['bandpass', 'BP']] },
      { k: 'cutoff', label: 'Cutoff', min: 20, max: 18000, curve: 'log', format: fmt.hz },
      { k: 'resonance', label: 'Resonance', min: 0, max: 1, format: fmt.pct },
    ],
  },
  delay: {
    name: 'Delay',
    params: [
      { k: 'time', type: 'choice', label: 'Time', options: [[0.25, '1/16'], [0.5, '1/8'], [0.75, '1/8.'], [1, '1/4'], [1.5, '1/4.'], [2, '1/2']] },
      { k: 'feedback', label: 'Feedback', min: 0, max: 0.95, format: fmt.pct },
      { k: 'mix', label: 'Mix', min: 0, max: 1, format: fmt.pct },
      { k: 'pingPong', type: 'toggle', label: 'Ping-pong' },
    ],
  },
  reverb: {
    name: 'Reverb',
    params: [
      { k: 'size', label: 'Size', min: 0, max: 1, format: fmt.pct },
      { k: 'damping', label: 'Damping', min: 0, max: 1, format: fmt.pct },
      { k: 'mix', label: 'Mix', min: 0, max: 1, format: fmt.pct },
      { k: 'width', label: 'Width', min: 0, max: 1, format: fmt.pct },
    ],
  },
  compressor: {
    name: 'Compressor',
    params: [
      { k: 'threshold', label: 'Threshold', min: -60, max: 0, step: 0.5, format: fmt.db },
      { k: 'ratio', label: 'Ratio', min: 1, max: 20, step: 0.1, format: (v) => `${(+v).toFixed(1)}:1` },
      { k: 'attack', label: 'Attack', min: 0.1, max: 100, curve: 'log', format: fmt.ms },
      { k: 'release', label: 'Release', min: 10, max: 1000, curve: 'log', format: fmt.ms },
      { k: 'makeup', label: 'Makeup', min: 0, max: 24, step: 0.5, format: fmt.db },
    ],
  },
  chorus: {
    name: 'Chorus',
    params: [
      { k: 'rate', label: 'Rate', min: 0.05, max: 5, curve: 'log', format: fmt.hz },
      { k: 'depth', label: 'Depth', min: 0.5, max: 10, step: 0.1, format: fmt.ms },
      { k: 'mix', label: 'Mix', min: 0, max: 1, format: fmt.pct },
    ],
  },
  bitcrusher: {
    name: 'Bitcrusher',
    params: [
      { k: 'bits', label: 'Bits', min: 1, max: 16, step: 1, format: fmt.int },
      { k: 'downsample', label: 'Downsample', min: 1, max: 32, step: 1, format: (v) => `÷${Math.round(v)}` },
      { k: 'mix', label: 'Mix', min: 0, max: 1, format: fmt.pct },
    ],
  },
  pump: {
    name: 'Pump',
    params: [
      { k: 'depth', label: 'Depth', min: 0, max: 1, format: fmt.pct },
      { k: 'rate', type: 'choice', label: 'Every', options: [[0.5, '1/8'], [1, '1/4'], [2, '1/2'], [4, 'bar']] },
      { k: 'release', label: 'Recovery', min: 0.05, max: 1, format: fmt.pct },
    ],
  },
};

let panel;
let signature = null;
let strips = []; // { trackId (null = master), refresh(), meter canvas, meterIndex }

export function init() {
  panel = document.getElementById('panel-mixer');
  on('project', render);
}

function sig(p) {
  const fx = (list) => list.map((e) => `${e.id}:${e.kind.type}`).join(',');
  return p.tracks.filter((t) => t.kind.type !== 'media').map((t) => `${t.id}/${t.name}/${t.color}/${fx(t.effects)}`).join('|') + '#' + fx(p.master.effects);
}

function render() {
  const p = store.project;
  if (!p) return;
  const s = sig(p);
  if (s === signature) {
    if (!interacting()) for (const st of strips) st.refresh();
    return;
  }
  signature = s;
  panel.innerHTML = '';
  strips = [];
  p.tracks.forEach((t, i) => {
    if (t.kind.type === 'media') return;
    strips.push(strip(t, i));
  });
  strips.push(strip(null, -1));
  for (const st of strips) panel.append(st.el);
}

function strip(t, index) {
  const isMaster = !t;
  const id = t?.id ?? null;
  const get = () => (isMaster ? store.project.master : store.project.tracks.find((x) => x.id === id));
  const cur = get();
  const vol = cur.volume;
  const fader = el('input', { type: 'range', class: 'fader', min: 0, max: 1.5, step: 0.005, value: vol, title: 'Volume (double-click: 0 dB)' });
  const volLabel = el('span', {}, `${gainToDb(vol)} dB`);
  let faderActive = false;
  fader.addEventListener('pointerdown', () => { faderActive = true; });
  fader.addEventListener('pointerup', () => { faderActive = false; });
  const setVol = (v) => {
    volLabel.textContent = `${gainToDb(v)} dB`;
    if (isMaster) applyLive('master', { type: 'setMasterVolume', volume: v });
    else applyLive('mix:' + id, { type: 'updateTrack', trackId: id, volume: v });
  };
  fader.addEventListener('input', () => setVol(+fader.value));
  fader.addEventListener('dblclick', () => { fader.value = 1; setVol(1); });
  const meter = el('canvas', { class: 'meter', width: 12, height: 120 });

  let pan = null, panLabel = null, msrow = null;
  if (!isMaster) {
    panLabel = el('span', {}, panText(t.pan));
    pan = el('input', { type: 'range', min: -1, max: 1, step: 0.01, value: t.pan, title: 'Pan (double-click: center)' });
    pan.addEventListener('input', () => {
      panLabel.textContent = panText(+pan.value);
      applyLive('mix:' + id, { type: 'updateTrack', trackId: id, pan: +pan.value });
    });
    pan.addEventListener('dblclick', () => { pan.value = 0; panLabel.textContent = 'C'; apply({ type: 'updateTrack', trackId: id, pan: 0 }); });
    const m = el('button', { class: 'm' + (t.mute ? ' on' : ''), onclick: () => apply({ type: 'updateTrack', trackId: id, mute: !get().mute }) }, 'Mute');
    const s = el('button', { class: 's' + (t.solo ? ' on' : ''), onclick: () => apply({ type: 'updateTrack', trackId: id, solo: !get().solo }) }, 'Solo');
    msrow = el('div', { class: 'msrow' }, m, s);
  }

  const fxList = el('div', { class: 'fx-list' });
  const fxCtls = [];
  for (const [i, e] of cur.effects.entries()) {
    const card = fxCard(id, e, i, cur.effects.length, fxCtls);
    fxList.append(card);
  }
  const add = el('select', {
    class: 'add-fx',
    onchange: (ev) => {
      if (ev.target.value) apply({ type: 'addEffect', trackId: id, effect: ev.target.value });
      ev.target.value = '';
    },
  }, el('option', { value: '' }, '+ Add effect…'), Object.entries(FX).map(([k, d]) => el('option', { value: k }, d.name)));

  const root = el('div', { class: 'strip' + (isMaster ? ' master' : '') },
    el('div', { class: 'sname', style: { borderColor: isMaster ? 'var(--accent)' : t.color }, title: isMaster ? 'Master' : t.name }, isMaster ? 'Master' : t.name),
    el('div', { class: 'fader-row' }, meter, fader, el('div', { class: 'fader-info' }, volLabel, el('span', { class: 'small' }, isMaster ? 'limiter on' : ''))),
    pan ? el('div', { class: 'pan' }, 'Pan', pan, panLabel) : null,
    msrow,
    fxList,
    add,
  );

  const refresh = () => {
    const c = get();
    if (!c) return;
    if (!faderActive) { fader.value = c.volume; volLabel.textContent = `${gainToDb(c.volume)} dB`; }
    if (pan) { pan.value = c.pan; panLabel.textContent = panText(c.pan); }
    if (msrow) {
      msrow.children[0].classList.toggle('on', c.mute);
      msrow.children[1].classList.toggle('on', c.solo);
    }
    for (const f of fxCtls) f.refresh(c.effects.find((e) => e.id === f.id));
  };
  return { el: root, refresh, meter, index };
}

function panText(v) {
  if (Math.abs(v) < 0.01) return 'C';
  return `${Math.round(Math.abs(v) * 100)}${v < 0 ? 'L' : 'R'}`;
}

function fxCard(trackId, e, i, count, fxCtls) {
  const def = FX[e.kind.type];
  let local = deepClone(e.kind);
  const send = () => applyLive('fx:' + e.id, { type: 'updateEffect', trackId, effectId: e.id, kind: deepClone(local) });
  const params = el('div', {});
  const ctls = [];
  for (const prm of def.params) {
    const onv = (v) => { local[prm.k] = v; send(); };
    const c = prm.type === 'choice'
      ? choice({ ...prm, value: local[prm.k], onChange: onv, compact: true })
      : prm.type === 'toggle'
        ? toggle({ ...prm, value: local[prm.k], onChange: onv, compact: true })
        : range({ ...prm, value: local[prm.k], onInput: onv, compact: true });
    ctls.push({ k: prm.k, c });
    params.append(c.el);
  }
  let open = true;
  const head = el('div', { class: 'fx-head' },
    el('input', { type: 'checkbox', checked: e.enabled, title: 'Enable', onchange: (ev) => apply({ type: 'updateEffect', trackId, effectId: e.id, enabled: ev.target.checked }) }),
    el('span', { class: 'n', title: 'Click to collapse', onclick: () => { open = !open; params.style.display = open ? '' : 'none'; } }, def.name),
    el('button', { disabled: i === 0, title: 'Move up', onclick: () => apply({ type: 'moveEffect', trackId, effectId: e.id, index: i - 1 }) }, '▲'),
    el('button', { disabled: i === count - 1, title: 'Move down', onclick: () => apply({ type: 'moveEffect', trackId, effectId: e.id, index: i + 1 }) }, '▼'),
    el('button', { title: 'Remove', onclick: () => apply({ type: 'removeEffect', trackId, effectId: e.id }) }, '✕'),
  );
  const card = el('div', { class: 'fx' + (e.enabled ? '' : ' off') }, head, params);
  fxCtls.push({
    id: e.id,
    refresh: (ne) => {
      if (!ne) return;
      card.classList.toggle('off', !ne.enabled);
      head.querySelector('input').checked = ne.enabled;
      if (ctls.some((x) => x.c.isActive())) return;
      local = deepClone(ne.kind);
      for (const { k, c } of ctls) c.set(local[k]);
    },
  });
  return card;
}

/** Draws level meters; called every animation frame while the mixer is visible. */
export function frame() {
  if (store.tab !== 'mixer') return;
  for (const s of strips) {
    const v = s.index < 0 ? store.master : store.meters[s.index] || 0;
    const ctx = s.meter.getContext('2d');
    const h = s.meter.height, w = s.meter.width;
    ctx.fillStyle = '#0b0c10';
    ctx.fillRect(0, 0, w, h);
    const db = v > 0 ? 20 * Math.log10(v) : -90;
    const frac = Math.max(0, Math.min(1, (db + 60) / 60));
    const g = ctx.createLinearGradient(0, h, 0, 0);
    g.addColorStop(0, '#67e08a');
    g.addColorStop(0.75, '#ffb547');
    g.addColorStop(1, '#ff5a6a');
    ctx.fillStyle = g;
    ctx.fillRect(1, h - frac * h, w - 2, frac * h);
  }
}
