import { invoke, listen } from './api.js';
import {
  store, on, emit, apply, command, setResponse, undo, redo, currentTick, selectTrack, selectPattern, selectedTrack, pattern, track,
} from './store.js';
import {
  el, toast, formatBars, formatTime, tickToSecs, snapTicks, NOTE_NAMES, SCALES, isTypingTarget, showMenu, clamp,
} from './util.js';
import * as timeline from './timeline.js';
import * as pianoroll from './pianoroll.js';
import * as stepseq from './stepseq.js';
import * as instrument from './instrument.js';
import * as mixer from './mixer.js';
import * as media from './media.js';
import * as dialogs from './dialogs.js';
import * as assistant from './assistant.js';

const $ = (id) => document.getElementById(id);

window.addEventListener('error', (e) => toast(`UI error: ${e.message}`, 'error'));
window.addEventListener('unhandledrejection', (e) => console.warn(e.reason));
document.addEventListener('contextmenu', (e) => {
  if (!e.target.closest('canvas, .th, input, textarea')) e.preventDefault();
});

async function boot() {
  const s = await invoke('get_state');
  store.presets = s.presets;
  store.templates = s.templates;
  store.metronome = s.metronome;
  timeline.init();
  pianoroll.init();
  stepseq.init();
  instrument.init();
  mixer.init();
  media.init();
  assistant.init();
  initToolbar();
  initSidebar();
  initTabs();
  initSplitter();
  initKeyboard();
  on('project', updateToolbar);
  on('selection', () => { updateTabContext(); renderPatternList(); renderPresetList(); });
  on('open-tab', openTab);
  on('presets', renderPresetList);
  on('assistant-busy', (b) => $('btn-ai').classList.toggle('busy', b));
  on('layout', () => { timeline.requestDraw(); pianoroll.requestDraw(); });
  setResponse(s.response);

  listen('transport', (e) => {
    const d = e.payload;
    const was = store.transport;
    store.transport = { tick: d.tick, playing: d.playing, recording: d.recording, at: performance.now() };
    store.meters = d.meters;
    store.master = d.master;
    store.input = d.input;
    store.load = d.load;
    if (was.playing !== d.playing || was.recording !== d.recording) updateTransportButtons();
    if (!d.playing && was.tick !== d.tick) timeline.requestDraw();
  });
  listen('tauri://drag-drop', (e) => onOsDrop(e.payload));
  listen('midi-note', () => {});
  // Changes made by the assistant or by Claude Code through the control API.
  listen('project-changed', (e) => {
    setResponse(e.payload.response);
    if (e.payload.message) toast(e.payload.message, 'ok', 3500);
  });
  listen('presets-changed', async () => {
    store.presets = await invoke('get_presets');
    emit('presets');
  });
  listen('assistant-notify', (e) => e.payload && toast(e.payload, 'info', 5000));

  invoke('audio_status').then((st) => {
    if (!st.running) toast(`Audio output couldn't start: ${st.error}\nOpen Audio settings to pick a device.`, 'error');
  });
  requestAnimationFrame(loop);
}

// ---------- render loop ----------
let lastLcd = '';
function loop() {
  const tick = currentTick();
  const p = store.project;
  if (p) {
    const lcd = formatBars(tick, p);
    if (lcd !== lastLcd) {
      lastLcd = lcd;
      $('pos-bars').textContent = lcd;
      $('pos-time').textContent = formatTime(tickToSecs(p, tick));
    }
  }
  timeline.frame(tick);
  pianoroll.frame();
  stepseq.frame();
  mixer.frame();
  media.frame();
  const m = store.master;
  const db = m > 0 ? 20 * Math.log10(m) : -90;
  $('master-meter').style.width = `${clamp((db + 60) / 60, 0, 1) * 100}%`;
  requestAnimationFrame(loop);
}
setInterval(() => { $('cpu').textContent = `DSP ${Math.round(store.load * 100)}%`; }, 500);

// ---------- toolbar ----------
async function transport(action) {
  try {
    const r = await command('transport', { action });
    if (r) {
      setResponse(r);
      toast('Recording added to the timeline (Ctrl+Z to undo)', 'ok', 2500);
    }
  } catch { /* toast shown */ }
}

async function toggleRecord() {
  if (store.transport.recording) return transport('stop');
  try {
    await command('record_start');
    toast('Recording… press Space or ■ to stop', 'warn', 2500);
  } catch { /* toast shown */ }
}

function initToolbar() {
  $('btn-new').onclick = () => dialogs.newProjectDialog();
  $('btn-open').onclick = () => dialogs.openProject();
  $('btn-save').onclick = (e) => dialogs.saveProject(e.shiftKey);
  $('btn-export').onclick = (e) => dialogs.exportMenu(e.currentTarget);
  $('btn-undo').onclick = undo;
  $('btn-redo').onclick = redo;
  $('btn-home').onclick = () => timeline.seekTo(0);
  $('btn-play').onclick = () => transport('toggle');
  $('btn-stop').onclick = () => transport('stop');
  $('btn-rec').onclick = toggleRecord;
  $('btn-loop').onclick = toggleLoop;
  $('btn-metro').onclick = () => {
    store.metronome = !store.metronome;
    invoke('set_metronome', { on: store.metronome });
    updateToolbar();
  };
  $('btn-typing').onclick = () => { store.typingKeyboard = !store.typingKeyboard; updateToolbar(); };
  $('btn-settings').onclick = () => dialogs.settingsDialog();
  $('btn-help').onclick = () => dialogs.helpDialog();
  $('btn-ai').onclick = () => openTab('assistant');
  $('inp-bpm').onchange = (e) => apply({ type: 'setBpm', bpm: clamp(+e.target.value || 120, 20, 400) });
  $('sel-timesig').onchange = (e) => {
    const [num, den] = e.target.value.split('/').map(Number);
    apply({ type: 'setTimeSig', num, den });
  };
  const key = $('sel-key');
  NOTE_NAMES.forEach((n, i) => key.append(el('option', { value: i }, n)));
  const scale = $('sel-scale');
  for (const [k, s] of Object.entries(SCALES)) scale.append(el('option', { value: k }, s.name));
  const setKey = () => apply({ type: 'setKeyScale', root: +key.value, scale: scale.value });
  key.onchange = setKey;
  scale.onchange = setKey;
  $('sel-snap').value = store.snap;
  $('sel-snap').onchange = (e) => { store.snap = e.target.value; emit('selection'); };
}

function toggleLoop() {
  const p = store.project;
  let { loopStart: s, loopEnd: e } = p;
  if (e <= s) { s = 0; e = Math.max(p.tracks.flatMap((t) => t.clips).reduce((m, c) => Math.max(m, c.start + c.length), 0), 4 * 384); }
  apply({ type: 'setLoop', start: s, end: e, enabled: !p.loopEnabled });
}

function updateToolbar() {
  const p = store.project;
  if (!p) return;
  $('btn-undo').disabled = !store.canUndo;
  $('btn-redo').disabled = !store.canRedo;
  $('btn-loop').classList.toggle('on', p.loopEnabled);
  $('btn-metro').classList.toggle('on', store.metronome);
  $('btn-typing').classList.toggle('on', store.typingKeyboard);
  $('octave-label').textContent = `Oct ${store.octave}`;
  if (document.activeElement !== $('inp-bpm')) $('inp-bpm').value = Math.round(p.bpm * 100) / 100;
  $('sel-timesig').value = `${p.timeSigNum}/${p.timeSigDen}`;
  $('sel-key').value = p.keyRoot;
  $('sel-scale').value = p.scale in SCALES ? p.scale : 'major';
  const name = `${p.name}${store.dirty ? ' •' : ''}`;
  document.title = `${name} — Music Maker`;
  window.__TAURI__.window?.getCurrentWindow?.().setTitle(document.title).catch(() => {});
  renderPatternList();
  renderAssetList();
  renderPresetList();
  updateTabContext();
}

function updateTransportButtons() {
  $('btn-play').classList.toggle('on', store.transport.playing);
  $('btn-play').innerHTML = store.transport.playing ? '&#x23F8;' : '&#x25B6;';
  $('btn-rec').classList.toggle('on', store.transport.recording);
}

// ---------- sidebar ----------
function initSidebar() {
  document.querySelectorAll('[data-add-track]').forEach((b) => {
    b.onclick = async () => {
      const kind = b.dataset.addTrack;
      if (kind === 'media') {
        const existing = store.project.tracks.find((t) => t.kind.type === 'media');
        if (existing) { media.pickAndAdd(existing.id, 0); return; }
        const id = await apply({ type: 'addTrack', kind: 'media', index: 0 });
        if (id) { selectTrack(id); media.showPreview(); media.pickAndAdd(id, 0); }
        return;
      }
      const id = await apply({ type: 'addTrack', kind });
      if (id) selectTrack(id);
    };
  });
  $('preset-filter').oninput = renderPresetList;
  $('btn-new-pattern').onclick = async () => {
    const id = await apply({ type: 'addPattern' });
    if (id) { selectPattern(id); openTab('pianoroll'); }
  };
  $('btn-import').onclick = () => timeline.importAudioAt(null, 0);
  $('btn-import-media').onclick = () => media.pickAndAdd(store.project.tracks.find((t) => t.kind.type === 'media')?.id ?? null, 0);
}

function renderPresetList() {
  const list = $('preset-list');
  const q = $('preset-filter').value.toLowerCase();
  list.innerHTML = '';
  const t = selectedTrack();
  const canSet = t?.kind.type === 'instrument';
  let cat = null;
  for (const p of store.presets) {
    if (q && !`${p.name} ${p.category}`.toLowerCase().includes(q)) continue;
    if (p.category !== cat) { cat = p.category; list.append(el('div', { class: 'cat' }, cat)); }
    const mine = p.category === 'My presets';
    list.append(el('div', {
      class: 'item',
      title: (canSet ? `Click: use on "${t.name}"` : 'Click to add as a new track') + (mine ? ' · right-click to delete' : ''),
      oncontextmenu: mine ? (e) => {
        e.preventDefault();
        showMenu(e.clientX, e.clientY, [{
          label: `Delete preset "${p.name}"`,
          action: async () => { store.presets = await command('delete_user_preset', { name: p.name }); emit('presets'); },
        }]);
      } : undefined,
      onclick: async () => {
        if (canSet) {
          await apply({ type: 'setInstrument', trackId: t.id, instrument: structuredClone(p.instrument) });
          toast(`${t.name} → ${p.name}`, 'ok', 1200);
        } else addPresetTrack(p);
      },
    }, el('span', { class: 'name' }, p.name), el('button', {
      title: 'Add as a new track',
      onclick: (e) => { e.stopPropagation(); addPresetTrack(p); },
    }, '+')));
  }
}

async function addPresetTrack(p) {
  const id = await apply({ type: 'addTrack', kind: 'instrument', name: p.name, instrument: structuredClone(p.instrument) });
  if (id) selectTrack(id);
}

function renderPatternList() {
  const list = $('pattern-list');
  list.innerHTML = '';
  for (const pat of store.project.patterns) {
    const item = el('div', {
      class: 'item' + (pat.id === store.selectedPatternId ? ' sel' : ''),
      title: 'Click to edit · drag onto an instrument lane to place',
      onmousedown: (e) => startItemDrag(e, { type: 'pattern', id: pat.id }, pat.name, () => { selectPattern(pat.id); openTab(store.tab === 'stepseq' ? 'stepseq' : 'pianoroll'); }),
      oncontextmenu: (e) => {
        e.preventDefault();
        showMenu(e.clientX, e.clientY, [
          { label: 'Rename', action: () => { const n = prompt('Pattern name', pat.name); if (n) apply({ type: 'updatePattern', patternId: pat.id, name: n }); } },
          { label: 'Clone', action: () => apply({ type: 'clonePattern', patternId: pat.id }).then((id) => id && selectPattern(id)) },
          { label: 'Delete (and its clips)', action: () => apply({ type: 'removePattern', patternId: pat.id }) },
        ]);
      },
    }, el('span', { class: 'swatch', style: { background: pat.color } }), el('span', { class: 'name' }, pat.name),
    el('span', { class: 'muted small' }, `${pat.notes.length}`));
    list.append(item);
  }
}

function renderAssetList() {
  const list = $('asset-list');
  list.innerHTML = '';
  const icon = { audio: '♪', video: '▶', image: '▣' };
  for (const a of store.project.assets) {
    list.append(el('div', {
      class: 'item',
      title: `${a.path}\nDrag onto the timeline to place`,
      onmousedown: (e) => startItemDrag(e, { type: 'asset', id: a.id }, a.name, () => {}),
    }, el('span', { class: 'muted' }, icon[a.kind] || '·'), el('span', { class: 'name' }, a.name)));
  }
  if (!store.project.assets.length) list.append(el('div', { class: 'muted small' }, 'Imported audio, recordings and media show up here.'));
}

/** Mouse-driven drag from the sidebar (HTML5 drag-and-drop is taken by the OS file drop on Windows). */
function startItemDrag(e, item, label, onClick) {
  if (e.button !== 0) return;
  const x0 = e.clientX, y0 = e.clientY;
  let ghost = null;
  const move = (ev) => {
    if (!ghost && Math.hypot(ev.clientX - x0, ev.clientY - y0) > 5) {
      ghost = el('div', { class: 'toast', style: { position: 'fixed', pointerEvents: 'none', zIndex: 90, padding: '3px 8px' } }, label);
      document.body.append(ghost);
    }
    if (ghost) { ghost.style.left = ev.clientX + 12 + 'px'; ghost.style.top = ev.clientY + 8 + 'px'; }
  };
  const up = async (ev) => {
    window.removeEventListener('mousemove', move);
    window.removeEventListener('mouseup', up);
    if (!ghost) return onClick();
    ghost.remove();
    if (!(await timeline.dropItem(item, ev.clientX, ev.clientY))) toast('Drop it on the playlist', 'warn', 1500);
  };
  window.addEventListener('mousemove', move);
  window.addEventListener('mouseup', up);
}

// ---------- tabs / splitter ----------
function initTabs() {
  document.querySelectorAll('#tabs [data-tab]').forEach((b) => { b.onclick = () => openTab(b.dataset.tab); });
}

function openTab(name) {
  store.tab = name;
  document.querySelectorAll('#tabs [data-tab]').forEach((b) => b.classList.toggle('active', b.dataset.tab === name));
  document.querySelectorAll('#bottom .panel').forEach((p) => p.classList.toggle('active', p.id === `panel-${name}`));
  pianoroll.requestDraw();
  stepseq.requestDraw();
  updateTabContext();
  if (name === 'assistant') setTimeout(() => assistant.focusInput(), 0);
}

function updateTabContext() {
  const t = selectedTrack();
  const pat = pattern(store.selectedPatternId);
  const parts = [];
  if (t) parts.push(`Track: ${t.name}`);
  if (pat && (store.tab === 'pianoroll' || store.tab === 'stepseq')) parts.push(`Pattern: ${pat.name}`);
  $('tab-context').textContent = parts.join(' · ');
}

function initSplitter() {
  const sp = $('splitter');
  sp.addEventListener('mousedown', (e) => {
    e.preventDefault();
    const move = (ev) => {
      const h = clamp(window.innerHeight - ev.clientY, 140, window.innerHeight - 220);
      document.body.style.setProperty('--bottom-h', `${h}px`);
    };
    const up = () => { window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', up); emit('layout'); };
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
  });
}

// ---------- keyboard ----------
const KEYMAP = {
  KeyZ: 0, KeyS: 1, KeyX: 2, KeyD: 3, KeyC: 4, KeyV: 5, KeyG: 6, KeyB: 7, KeyH: 8, KeyN: 9, KeyJ: 10, KeyM: 11,
  Comma: 12, KeyL: 13, Period: 14, Semicolon: 15, Slash: 16,
  KeyQ: 12, Digit2: 13, KeyW: 14, Digit3: 15, KeyE: 16, KeyR: 17, Digit5: 18, KeyT: 19, Digit6: 20, KeyY: 21, Digit7: 22, KeyU: 23,
  KeyI: 24, Digit9: 25, KeyO: 26, Digit0: 27, KeyP: 28,
};
const held = new Map(); // code -> { trackId, pitch }

function playTrack() {
  const t = selectedTrack();
  if (t?.kind.type === 'instrument') return t;
  return store.project.tracks.find((x) => x.kind.type === 'instrument');
}

function initKeyboard() {
  window.addEventListener('keydown', (e) => {
    if (isTypingTarget(e) || document.querySelector('.modal-back')) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    if (ctrl && k === 'z' && !e.shiftKey) { e.preventDefault(); undo(); return; }
    if (ctrl && (k === 'y' || (k === 'z' && e.shiftKey))) { e.preventDefault(); redo(); return; }
    if (ctrl && k === 's') { e.preventDefault(); dialogs.saveProject(e.shiftKey); return; }
    if (ctrl && k === 'o') { e.preventDefault(); dialogs.openProject(); return; }
    if (ctrl && k === 'n') { e.preventDefault(); dialogs.newProjectDialog(); return; }
    if (ctrl && k === 'r') { e.preventDefault(); toggleRecord(); return; }
    if (ctrl && k === 'l') { e.preventDefault(); toggleLoop(); return; }
    if (ctrl && k === 'k') { e.preventDefault(); openTab('assistant'); return; }
    if (e.code === 'Space' || e.key === ' ') {
      e.preventDefault();
      // A focused button would also "click" on keyup.
      if (document.activeElement?.tagName === 'BUTTON') document.activeElement.blur();
      if (!e.repeat) transport('toggle');
      return;
    }
    if (e.key === 'Home') { timeline.seekTo(0); return; }
    // Editing shortcuts go to the focused panel first.
    const handler = store.focus === 'pianoroll' && store.tab === 'pianoroll' ? pianoroll.onKey : store.focus === 'timeline' ? timeline.onKey : null;
    if (handler && handler(e)) { e.preventDefault(); return; }
    if (ctrl || e.altKey) return;
    if (store.typingKeyboard) {
      if (e.code === 'BracketLeft') { store.octave = clamp(store.octave - 1, 0, 8); updateToolbar(); return; }
      if (e.code === 'BracketRight') { store.octave = clamp(store.octave + 1, 0, 8); updateToolbar(); return; }
      const off = KEYMAP[e.code];
      if (off !== undefined) {
        e.preventDefault();
        if (e.repeat || held.has(e.code)) return;
        const t = playTrack();
        if (!t) return;
        const pitch = clamp((store.octave + 1) * 12 + off, 0, 127);
        held.set(e.code, { trackId: t.id, pitch });
        invoke('note_on', { trackId: t.id, pitch, vel: 0.8 }).catch(() => {});
      }
    }
  });
  window.addEventListener('keyup', (e) => {
    const h = held.get(e.code);
    if (h) {
      held.delete(e.code);
      invoke('note_off', { trackId: h.trackId, pitch: h.pitch }).catch(() => {});
    }
  });
  window.addEventListener('blur', () => {
    for (const h of held.values()) invoke('note_off', { trackId: h.trackId, pitch: h.pitch }).catch(() => {});
    held.clear();
  });
}

// ---------- OS file drops ----------
async function onOsDrop(payload) {
  const paths = payload?.paths || [];
  if (!paths.length) return;
  const dpr = window.devicePixelRatio || 1;
  const x = (payload.position?.x ?? 0) / dpr;
  const y = (payload.position?.y ?? 0) / dpr;
  const files = await command('classify_files', { paths });
  const proj = files.find((f) => f.kind === 'project');
  if (proj) return dialogs.openProject(proj.path);
  const loc = timeline.locate(x, y);
  const s = snapTicks(store.snap, store.project);
  const start = loc ? Math.floor(loc.tick / s) * s : 0;
  const audio = files.filter((f) => f.kind === 'audio').map((f) => f.path);
  if (audio.length) {
    toast(`Importing ${audio.length} file${audio.length > 1 ? 's' : ''}…`, 'info', 1500);
    try {
      const r = await command('import_audio', { paths: audio, trackId: loc?.track?.kind.type === 'audio' ? loc.track.id : null, start });
      if (r) setResponse(r);
    } catch { /* toast shown */ }
  }
  const med = files.filter((f) => f.kind === 'video' || f.kind === 'image');
  if (med.length) await media.addMediaFiles(med, loc?.track?.kind.type === 'media' ? loc.track.id : null, start);
  const unknown = files.filter((f) => f.kind === 'unknown');
  if (unknown.length) toast(`Can't use: ${unknown.map((f) => f.path.split(/[\\/]/).pop()).join(', ')}`, 'warn');
}

boot().catch((e) => {
  console.error(e);
  document.body.innerHTML = `<pre style="padding:20px;color:#ff5a6a">Failed to start: ${e}</pre>`;
});
