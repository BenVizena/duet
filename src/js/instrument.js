// Instrument editor for the selected track.
import { invoke } from './api.js';
import { store, on, emit, apply, applyLive, command, setResponse, selectedTrack, asset } from './store.js';
import { el, deepClone, noteName, clamp, toast } from './util.js';
import { range, choice, toggle, fmt, interacting } from './widgets.js';

let panel;
let signature = null;
let local = null; // working copy of the instrument params
let controls = []; // { key, ctl, show }

export function init() {
  panel = document.getElementById('panel-instrument');
  on('project', render);
  on('selection', render);
  on('presets', () => { signature = null; render(); });
}

const WAVES = [['pulse', 'Pulse'], ['triangle', 'Triangle'], ['saw', 'Saw'], ['sine', 'Sine/FM'], ['wavetable', 'Wave (GB)'], ['noise', 'Noise']];

const SYNTH = [
  {
    title: 'Oscillator',
    controls: [
      { k: 'wave', type: 'choice', label: 'Wave', options: WAVES },
      { k: 'duty', type: 'choice', label: 'Duty', options: [[0.125, '12.5%'], [0.25, '25%'], [0.5, '50%'], [0.75, '75%']], show: (p) => p.wave === 'pulse' },
      { k: 'duty', label: 'Width', min: 0.02, max: 0.98, step: 0.01, format: fmt.pct, show: (p) => p.wave === 'pulse' },
      { k: 'noiseShort', type: 'toggle', label: 'Metallic', show: (p) => p.wave === 'noise' },
      { k: 'wavetable', type: 'wavetable', show: (p) => p.wave === 'wavetable' },
      { k: 'octave', label: 'Octave', min: -3, max: 3, step: 1, format: (v) => (v > 0 ? '+' : '') + v },
      { k: 'fine', label: 'Fine', min: -100, max: 100, step: 1, format: fmt.cents },
      { k: 'unison', label: 'Unison', min: 1, max: 8, step: 1, format: fmt.int, show: (p) => p.wave !== 'noise' },
      { k: 'detune', label: 'Spread', min: 0, max: 60, step: 0.5, format: fmt.cents, show: (p) => p.unison > 1 },
      { k: 'bits', type: 'choice', label: 'Bit depth', options: [[0, 'Off'], [3, '3'], [4, '4 (GB)'], [6, '6'], [8, '8']] },
    ],
  },
  {
    title: 'Envelope',
    controls: [
      { k: 'attack', label: 'Attack', min: 0, max: 4, curve: 'sq', format: fmt.secs },
      { k: 'decay', label: 'Decay', min: 0.001, max: 4, curve: 'sq', format: fmt.secs },
      { k: 'sustain', label: 'Sustain', min: 0, max: 1, format: fmt.pct },
      { k: 'release', label: 'Release', min: 0.002, max: 5, curve: 'sq', format: fmt.secs },
      { k: 'velSens', label: 'Velocity', min: 0, max: 1, format: fmt.pct },
      { k: 'gain', label: 'Volume', min: 0, max: 1, format: fmt.pct },
    ],
  },
  {
    title: 'Filter',
    controls: [
      { k: 'filterOn', type: 'toggle', label: 'Lowpass' },
      { k: 'cutoff', label: 'Cutoff', min: 30, max: 18000, curve: 'log', format: fmt.hz, show: (p) => p.filterOn },
      { k: 'resonance', label: 'Resonance', min: 0, max: 1, format: fmt.pct, show: (p) => p.filterOn },
      { k: 'filterEnv', label: 'Env amount', min: -4, max: 7, step: 0.05, format: fmt.oct, show: (p) => p.filterOn },
      { k: 'filterDecay', label: 'Env decay', min: 0.005, max: 3, curve: 'sq', format: fmt.secs, show: (p) => p.filterOn },
    ],
  },
  {
    title: 'Vibrato & pitch',
    controls: [
      { k: 'vibratoDepth', label: 'Vibrato', min: 0, max: 1.5, step: 0.01, format: fmt.st },
      { k: 'vibratoRate', label: 'Rate', min: 0.5, max: 12, step: 0.1, format: fmt.hz },
      { k: 'vibratoDelay', label: 'Delay', min: 0, max: 1.5, curve: 'sq', format: fmt.secs },
      { k: 'sweep', label: 'Pitch sweep', min: -24, max: 24, step: 0.5, format: fmt.st },
      { k: 'sweepTime', label: 'Sweep time', min: 0.005, max: 1, curve: 'sq', format: fmt.secs, show: (p) => p.sweep !== 0 },
      { k: 'mono', type: 'toggle', label: 'Mono/legato' },
      { k: 'glide', label: 'Glide', min: 0, max: 1, curve: 'sq', format: fmt.secs, show: (p) => p.mono },
    ],
  },
  {
    title: 'Arpeggio',
    controls: [
      { k: 'arp', type: 'arp', label: 'Notes' },
      { k: 'arpSync', type: 'toggle', label: 'Tempo sync' },
      { k: 'arpSpeed', label: 'Speed', min: 0.5, max: 60, step: 0.5, format: (v) => `${v}/s`, show: (p) => !p.arpSync },
      { k: 'arpSpeed', type: 'choice', label: 'Rate', options: [[1, '1/4'], [2, '1/8'], [3, '1/8T'], [4, '1/16'], [6, '1/16T'], [8, '1/32']], show: (p) => p.arpSync },
    ],
  },
  {
    title: 'LFO',
    controls: [
      { k: 'lfoSync', type: 'toggle', label: 'Tempo sync' },
      { k: 'lfoRate', label: 'Rate', min: 0.05, max: 20, curve: 'log', format: fmt.hz, show: (p) => !p.lfoSync },
      { k: 'lfoRate', type: 'choice', label: 'Cycle', options: [[0.125, '2 bars'], [0.25, '1 bar'], [0.5, '1/2'], [1, '1/4'], [2, '1/8'], [4, '1/16']], show: (p) => p.lfoSync },
      { k: 'lfoCutoff', label: '→ Cutoff', min: -3, max: 3, step: 0.05, format: fmt.oct },
      { k: 'lfoPwm', label: '→ Pulse width', min: 0, max: 0.45, step: 0.01, format: fmt.pct, show: (p) => p.wave === 'pulse' },
      { k: 'lfoAmp', label: '→ Volume', min: 0, max: 1, format: fmt.pct },
    ],
  },
  {
    title: 'FM (sine wave)',
    show: (p) => p.wave === 'sine',
    controls: [
      { k: 'fmAmount', label: 'Amount', min: 0, max: 8, step: 0.05, format: fmt.num },
      { k: 'fmRatio', label: 'Ratio', min: 0.5, max: 8, step: 0.5, format: fmt.x },
      { k: 'fmDecay', label: 'Decay', min: 0, max: 3, curve: 'sq', format: (v) => (v === 0 ? 'none' : fmt.secs(v)) },
    ],
  },
];

const SAMPLER = [
  {
    title: 'Sample',
    controls: [
      { k: 'sample', type: 'sample' },
      { k: 'root', label: 'Root note', min: 24, max: 96, step: 1, format: (v) => noteName(Math.round(v)) },
      { k: 'fine', label: 'Fine', min: -100, max: 100, step: 1, format: fmt.cents },
      { k: 'looped', type: 'toggle', label: 'Loop' },
    ],
  },
  {
    title: 'Envelope',
    controls: [
      { k: 'attack', label: 'Attack', min: 0.0005, max: 2, curve: 'sq', format: fmt.secs },
      { k: 'release', label: 'Release', min: 0.002, max: 4, curve: 'sq', format: fmt.secs },
      { k: 'gain', label: 'Volume', min: 0, max: 1.5, format: fmt.pct },
    ],
  },
];

const DRUMS = [
  {
    title: 'Drum kit',
    controls: [
      { k: 'style', type: 'choice', label: 'Style', options: [['chip', 'Chip (GB/NES)'], ['clean', 'Acoustic-ish'], ['electro', '80s machine']] },
      { k: 'tune', label: 'Tune', min: -12, max: 12, step: 0.5, format: fmt.st },
      { k: 'decay', label: 'Decay', min: 0.2, max: 3, step: 0.05, format: fmt.x },
      { k: 'gain', label: 'Volume', min: 0, max: 1, format: fmt.pct },
    ],
  },
];

const DRUM_MAP_HINT =
  'Keys: C2 kick · C#2 rim · D2 snare · D#2 clap · F#2 closed hat · A#2 open hat · F2/A2/C3 toms · C#3 crash · D#3 ride. ' +
  'Any other key plays a pitched blip. Use the step sequencer for quick beats.';

function send() {
  const t = selectedTrack();
  if (!t) return;
  applyLive('inst:' + t.id, { type: 'setInstrument', trackId: t.id, instrument: deepClone(local) });
}

let lastLocalEdit = 0;
function setParam(k, v) {
  lastLocalEdit = performance.now();
  local[k] = v;
  send();
  refreshVisibility();
}

function buildCtl(c) {
  const value = local[c.k];
  if (c.type === 'choice') return choice({ label: c.label, options: c.options, value, onChange: (v) => setParam(c.k, v) });
  if (c.type === 'toggle') return toggle({ label: c.label, value, onChange: (v) => setParam(c.k, v) });
  if (c.type === 'wavetable') return wavetableEditor();
  if (c.type === 'arp') return arpEditor();
  if (c.type === 'sample') return sampleCtl();
  return range({ ...c, value, onInput: (v) => setParam(c.k, v) });
}

function render() {
  const t = selectedTrack();
  const inst = t?.kind.type === 'instrument' ? t.kind.instrument : null;
  const sig = t ? `${t.id}|${t.kind.type}|${inst?.type || ''}` : 'none';
  if (sig === signature && inst) {
    // Don't let an in-flight response overwrite edits the user just made.
    if (!interacting() && performance.now() - lastLocalEdit > 700) {
      local = deepClone(inst);
      for (const c of controls) if (!c.ctl.isActive() && c.key) c.ctl.set(local[c.key]);
      refreshVisibility();
    }
    return;
  }
  signature = sig;
  panel.innerHTML = '';
  controls = [];
  if (!t) {
    panel.append(el('div', { class: 'inst-body' }, el('div', { class: 'hint' }, 'Select a track to edit its instrument.')));
    return;
  }
  if (t.kind.type !== 'instrument') {
    const msg = t.kind.type === 'audio'
      ? [el('b', {}, 'Audio track. '), 'Drop audio files on it, or arm it (●) and press record to capture your mic or line input. Use the 🎧 button to hear the input while recording. Effects and volume live in the Mixer tab.']
      : [el('b', {}, 'Video / image track (optional). '), 'Drop MP4/WebM videos or images on it. The preview panel on the right follows the playhead, so you can score to picture. Right-click a video clip to pull its audio onto its own track.'];
    panel.append(el('div', { class: 'inst-body' }, el('div', { class: 'hint' }, ...msg)));
    return;
  }
  local = deepClone(inst);
  panel.append(header(t));
  const body = el('div', { class: 'inst-body' });
  const sections = inst.type === 'synth' ? SYNTH : inst.type === 'sampler' ? SAMPLER : DRUMS;
  for (const s of sections) {
    const card = el('div', { class: 'card' }, el('h4', {}, s.title));
    if (s.show) controls.push({ key: null, ctl: { set() {}, isActive: () => false }, show: s.show, el: card });
    for (const c of s.controls) {
      const ctl = buildCtl(c);
      card.append(ctl.el);
      controls.push({ key: c.type === 'sample' ? null : c.k, ctl, show: c.show, el: ctl.el });
    }
    body.append(card);
  }
  if (inst.type === 'drums') body.append(el('div', { class: 'hint' }, DRUM_MAP_HINT));
  if (inst.type === 'synth') {
    body.append(el('div', { class: 'hint' },
      el('b', {}, 'Tips: '),
      'Chip leads: Pulse 12.5–25% with delayed vibrato. GB bass: Wave + draw your own table. ',
      'Synthwave: Saw + Unison 5–7 + Spread ~20 ct, add Chorus/Delay/Reverb and a Pump in the mixer. ',
      'GBA strings/brass: Saw + Lowpass, slow attack; brass gets filter env. Double-click a slider to reset it.'));
  }
  panel.append(body);
  refreshVisibility();
}

function refreshVisibility() {
  for (const c of controls) if (c.show) c.el.style.display = c.show(local) ? '' : 'none';
}

function header(t) {
  const inst = t.kind.instrument;
  const groups = {};
  for (const p of store.presets) (groups[p.category] ||= []).push(p);
  const presetSel = el('select', {
    onchange: (e) => {
      const p = store.presets.find((x) => x.name === e.target.value);
      if (p) apply({ type: 'setInstrument', trackId: t.id, instrument: deepClone(p.instrument) });
      e.target.value = '';
    },
  }, el('option', { value: '' }, 'Load preset…'),
  Object.entries(groups).map(([cat, list]) => el('optgroup', { label: cat }, list.map((p) => el('option', { value: p.name }, p.name)))));
  const typeBtns = choice({
    label: 'Type',
    options: [['synth', 'Synth'], ['sampler', 'Sampler'], ['drums', 'Drum kit']],
    value: inst.type,
    onChange: (v) => {
      const def = { synth: 'Pulse Lead 12.5%', sampler: 'Sampler (load a sample)', drums: 'Chip Drum Kit' }[v];
      const p = store.presets.find((x) => x.name === def);
      if (p) apply({ type: 'setInstrument', trackId: t.id, instrument: deepClone(p.instrument) });
    },
  });
  const test = el('button', { title: 'Hold to hear the instrument' }, '▶ Test C4');
  const pitch = inst.type === 'drums' ? 36 : 60;
  test.addEventListener('pointerdown', () => invoke('note_on', { trackId: t.id, pitch, vel: 0.85 }).catch(() => {}));
  const off = () => invoke('note_off', { trackId: t.id, pitch }).catch(() => {});
  test.addEventListener('pointerup', off);
  test.addEventListener('pointerleave', off);
  const save = el('button', {
    title: 'Save this instrument as your own preset',
    onclick: async () => {
      const name = prompt('Preset name', `${t.name} preset`);
      if (!name) return;
      try {
        store.presets = await command('save_user_preset', { name, trackId: t.id });
        emit('presets');
        toast(`Saved preset "${name}" (under My presets)`, 'ok', 2500);
      } catch { /* toast shown */ }
    },
  }, 'Save preset…');
  return el('div', { class: 'inst-head' }, el('span', { class: 'title' }, t.name), typeBtns.el, presetSel, save, test,
    el('span', { class: 'muted small' }, 'Play it with the typing keyboard (Z–M, Q–U) or a MIDI keyboard.'));
}

function wavetableEditor() {
  const cv = el('canvas', { class: 'wt-canvas', width: 640, height: 224 });
  const draw = () => {
    const ctx = cv.getContext('2d');
    const w = cv.width, h = cv.height;
    ctx.fillStyle = '#0b0c10';
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = '#1d1f29';
    for (let i = 0; i <= 16; i++) ctx.fillRect(0, Math.round((i * h) / 16), w, 1);
    const cw = w / 32;
    ctx.fillStyle = '#3ee6ff';
    (local.wavetable || []).forEach((v, i) => {
      const y = h - ((v + 1) * h) / 16;
      ctx.fillRect(i * cw + 1, y, cw - 2, h / 16);
      ctx.fillStyle = '#3ee6ff33';
      ctx.fillRect(i * cw + 1, y, cw - 2, h - y);
      ctx.fillStyle = '#3ee6ff';
    });
  };
  let drawing = false;
  const setAt = (e) => {
    const r = cv.getBoundingClientRect();
    const i = clamp(Math.floor(((e.clientX - r.left) / r.width) * 32), 0, 31);
    const v = clamp(Math.floor((1 - (e.clientY - r.top) / r.height) * 16), 0, 15);
    if (local.wavetable[i] !== v) {
      lastLocalEdit = performance.now();
      local.wavetable[i] = v;
      draw();
      send();
    }
  };
  cv.addEventListener('pointerdown', (e) => { drawing = true; cv.setPointerCapture(e.pointerId); setAt(e); });
  cv.addEventListener('pointermove', (e) => drawing && setAt(e));
  cv.addEventListener('pointerup', () => { drawing = false; });
  const shapes = {
    Triangle: (i) => (i < 16 ? i : 31 - i),
    Saw: (i) => Math.floor(i / 2),
    Square: (i) => (i < 16 ? 15 : 0),
    Sine: (i) => Math.round(7.5 + 7.5 * Math.sin((i / 32) * Math.PI * 2)),
    Organ: (i) => Math.round(7.5 + 3.8 * Math.sin((i / 32) * Math.PI * 2) + 3.7 * Math.sin((i / 32) * Math.PI * 4)),
    Random: () => Math.floor(Math.random() * 16),
  };
  const btns = el('div', { class: 'choices' }, Object.entries(shapes).map(([n, f]) => el('button', {
    onclick: () => { local.wavetable = Array.from({ length: 32 }, (_, i) => f(i)); draw(); send(); },
  }, n)));
  draw();
  const root = el('div', { style: { display: 'flex', flexDirection: 'column', gap: '4px' } },
    el('span', { class: 'muted small' }, 'Draw the 32-step, 4-bit wave'), cv, btns);
  return { el: root, set: () => draw(), isActive: () => drawing };
}

function arpEditor() {
  const input = el('input', {
    type: 'text', placeholder: 'e.g. 0 4 7 12 (empty = off)', style: { width: '100%' },
    title: 'Semitone offsets cycled while a note is held',
    onchange: () => {
      const vals = input.value.split(/[\s,]+/).filter(Boolean).map(Number).filter((n) => Number.isFinite(n)).map((n) => clamp(Math.round(n), -48, 48));
      setParam('arp', vals);
    },
  });
  const quick = el('div', { class: 'choices' }, [['Off', []], ['Maj', [0, 4, 7]], ['Min', [0, 3, 7]], ['Oct', [0, 12]], ['Maj+8', [0, 4, 7, 12]], ['Min+8', [0, 3, 7, 12]], ['Pump', [0, 12, 7, 12]]]
    .map(([n, v]) => el('button', { onclick: () => { input.value = v.join(' '); setParam('arp', v); } }, n)));
  const set = (v) => { input.value = (v || []).join(' '); };
  set(local.arp);
  return { el: el('div', { class: 'ctl wide' }, el('label', {}, 'Notes'), el('div', {}, input, quick)), set, isActive: () => document.activeElement === input };
}

function sampleCtl() {
  const t = selectedTrack();
  const a = local.assetId ? asset(local.assetId) : null;
  const load = el('button', {
    onclick: async () => {
      const r = await command('load_sample', { trackId: t.id, path: null });
      if (r) setResponse(r);
    },
  }, a ? 'Replace sample…' : 'Load sample…');
  return {
    el: el('div', { class: 'ctl wide' }, el('label', {}, 'File'), el('div', {}, el('div', { class: 'small' }, a ? `${a.name} (${a.duration.toFixed(2)} s)` : 'No sample loaded'), load)),
    set() {},
    isActive: () => false,
  };
}
