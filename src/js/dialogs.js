import { listen } from './api.js';
import { store, command, setResponse, hasMedia } from './store.js';
import { el, toast, showMenu } from './util.js';

export function modal(title, body, buttons = [{ label: 'Close' }]) {
  const root = document.getElementById('modal-root');
  const back = el('div', { class: 'modal-back' });
  const close = () => back.remove();
  const foot = el('div', { class: 'mfoot' }, buttons.map((b) => el('button', {
    class: b.primary ? 'primary' : '',
    onclick: async () => { if (b.action && (await b.action()) === false) return; close(); },
  }, b.label)));
  const m = el('div', { class: 'modal' }, el('h2', {}, title), el('div', { class: 'mbody' }, body), foot);
  back.append(m);
  back.addEventListener('mousedown', (e) => { if (e.target === back) close(); });
  back.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); e.stopPropagation(); });
  root.append(back);
  return close;
}

function confirmDiscard() {
  return !store.dirty || confirm('You have unsaved changes. Discard them?');
}

export function newProjectDialog() {
  if (!confirmDiscard()) return;
  let close;
  const grid = el('div', { class: 'tpl-grid' }, store.templates.map((t) => el('div', {
    class: 'tpl',
    onclick: async () => {
      close();
      setResponse(await command('new_project', { template: t.id }));
      toast(`New project: ${t.name}`, 'ok', 2000);
    },
  }, el('b', {}, t.name), el('span', { class: 'muted' }, t.description))));
  close = modal('New project', [el('div', { class: 'muted' }, 'Pick a starting point. Templates are just a head start; everything can be changed.'), grid], [{ label: 'Cancel' }]);
}

export async function openProject(path = null) {
  if (!confirmDiscard()) return;
  const r = await command('open_project', { path });
  if (r) {
    setResponse(r);
    toast(`Opened ${r.project.name}`, 'ok', 2000);
  }
}

export async function saveProject(saveAs = false) {
  const r = await command('save_project', { saveAs });
  if (r) {
    setResponse(r);
    toast(`Saved to ${r.path}`, 'ok', 2500);
  }
}

let progressToast = null;
listen('export-progress', (e) => {
  if (progressToast) progressToast.textContent = `Rendering… ${Math.round(e.payload * 100)}%`;
});

async function runExport(cmd, what) {
  progressToast = el('div', { class: 'toast' }, `Rendering ${what}…`);
  document.getElementById('toasts').append(progressToast);
  try {
    const path = await command(cmd);
    if (path) toast(`Exported ${what}:\n${path}`, 'ok', 7000);
  } catch {
    // command() already showed the error
  } finally {
    progressToast.remove();
    progressToast = null;
  }
}

export function exportMenu(anchor) {
  const r = anchor.getBoundingClientRect();
  showMenu(r.left, r.bottom + 4, [
    { label: 'Export audio (WAV, 48 kHz / 24-bit)…', action: () => runExport('export_audio', 'audio') },
    {
      label: hasMedia() ? 'Export video with music (MP4)…' : 'Export video (add a video/image track first)',
      disabled: !hasMedia(),
      action: () => runExport('export_video', 'video'),
    },
  ]);
}

export async function settingsDialog() {
  const [devs, status] = await Promise.all([command('audio_devices'), command('audio_status')]);
  const cfg = status.config;
  const opt = (v, label, cur) => el('option', { value: v, selected: v === (cur ?? '') }, label);
  const out = el('select', {}, opt('', 'System default', cfg.output), devs.outputs.map((d) => opt(d, d, cfg.output)));
  const inp = el('select', {}, opt('none', 'None (no recording)', cfg.input ?? 'none'), opt('default', 'System default', cfg.input), devs.inputs.map((d) => opt(d, d, cfg.input)));
  const buf = el('select', {}, ['', 64, 128, 256, 512, 1024, 2048].map((b) => el('option', { value: b, selected: String(b) === String(cfg.buffer ?? '') }, b ? `${b} frames` : 'Device default')));
  const lat = el('input', { type: 'number', step: 1, min: -500, max: 500, value: cfg.latencyOffsetMs });
  const midi = el('select', {
    onchange: async () => {
      await command('midi_connect', { port: midi.value || null });
      toast(midi.value ? `MIDI input: ${midi.value} (plays the selected track)` : 'MIDI input off', 'ok', 2000);
    },
  }, opt('', devs.midi.length ? 'None' : 'None found', status.midiPort), devs.midi.map((d) => opt(d, d, status.midiPort)));
  const statusBox = el('div', { class: 'muted small' });
  const showStatus = (s) => {
    statusBox.innerHTML = '';
    const lines = s.running
      ? [`Output: ${s.output} @ ${s.sampleRate} Hz, ~${(s.latencyMs ?? 0).toFixed(1)} ms output latency`, s.input ? `Input: ${s.input} @ ${s.inputRate} Hz` : 'Input: off (arming an audio track turns on the default input)']
      : [`Audio is not running: ${s.error || 'unknown error'}`];
    for (const w of s.warnings || []) lines.push(`⚠ ${w}`);
    lines.push(devs.ffmpeg ? 'ffmpeg: found (video export available)' : 'ffmpeg: not found (needed only for video export)');
    for (const l of lines) statusBox.append(el('div', {}, l));
  };
  showStatus(status);
  const row = (label, ctl, hint) => el('div', { class: 'frow' }, el('label', { title: hint || '' }, label), ctl);
  modal('Audio & MIDI settings', [
    row('Output device', out),
    row('Input device', inp, 'Microphone / line input for recording'),
    row('Buffer size', buf, 'Smaller = lower latency but more CPU. Not all devices accept every size.'),
    row('Recording offset (ms)', lat, 'Extra shift applied to recordings, if takes land early/late after automatic latency compensation'),
    row('MIDI keyboard', midi),
    statusBox,
  ], [
    { label: 'Close' },
    {
      label: 'Apply',
      primary: true,
      action: async () => {
        try {
          const s = await command('set_audio_config', {
            output: out.value || null,
            input: inp.value,
            buffer: buf.value ? +buf.value : null,
            latencyOffsetMs: +lat.value || 0,
          });
          showStatus(s);
          toast('Audio settings applied', 'ok', 1500);
        } catch {
          // error toast shown already
        }
        return false;
      },
    },
  ]);
}

export function helpDialog() {
  const rows = [
    ['Space', 'Play / stop (stop returns to where you started)'],
    ['Home', 'Go to the start'],
    ['Ctrl+R', 'Record armed tracks (press Space or ■ to finish)'],
    ['Ctrl+L', 'Toggle loop'],
    ['Ctrl+Z / Ctrl+Y', 'Undo / redo'],
    ['Ctrl+S / Ctrl+Shift+S', 'Save / save as'],
    ['Ctrl+N / Ctrl+O', 'New / open project'],
    ['Z S X D C V … / Q 2 W 3 E …', 'Typing keyboard: play the selected instrument (two octaves)'],
    ['[ and ]', 'Typing keyboard octave down / up'],
    ['Playlist: click lane', 'Place the selected pattern'],
    ['Playlist: drag clip / edges', 'Move / resize / trim (Ctrl = copy, Alt = no snap)'],
    ['Playlist: right-click', 'Clip menu (delete, make unique, extract video audio…)'],
    ['Ruler', 'Click to move the playhead; drag the top strip to set the loop'],
    ['Ctrl+wheel', 'Zoom; Shift+wheel scrolls sideways'],
    ['Piano roll', 'Click to add · drag to move · drag right edge to resize · right-click/double-click to delete'],
    ['Piano roll keys', 'Ctrl+A select all · Ctrl+drag box select · Ctrl+C/V/D · arrows move (Shift = octave/bar) · Ctrl+Q quantize'],
  ];
  modal('Shortcuts & tips', [
    el('table', { class: 'keys-table' }, rows.map(([k, v]) => el('tr', {}, el('td', {}, k), el('td', {}, v)))),
    el('div', { class: 'muted' },
      'Quick start: New → pick a template, press Space. Click an instrument preset on the left to put it on the selected track. ',
      'Video and images are optional: add a Video/Image track only when you want to score to picture.'),
  ]);
}
