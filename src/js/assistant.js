// In-app AI assistant panel. Requests go to Claude Code (headless) via the
// backend; it edits the song through the same tools Claude Code uses, and the
// project updates live ("project-changed" events).
import { invoke, listen } from './api.js';
import { store, on, emit, track, pattern, hasMedia } from './store.js';
import { el, toast, barTicks } from './util.js';
import { selectedNoteIds } from './pianoroll.js';

let panel, log, input, sendBtn, stopBtn, statusEl;
let busy = false;
let available = null;
let currentBubble = null;

const SUGGESTIONS = [
  'Clean up the timing of the selected pattern a little',
  'Put in a dark cyberpunk-feeling chord progression',
  'Make the drums busier in bars 5–8',
  'How do I add automation to a filter?',
  'Write a short Game Boy style town theme',
  'Mix check: is anything too loud or too quiet?',
];

export function init() {
  panel = document.getElementById('panel-assistant');
  log = el('div', { class: 'chat-log' });
  input = el('textarea', {
    class: 'chat-input',
    rows: 2,
    placeholder: 'Ask anything: "how do I…", "clean up the spacing", "add a dark synthwave chord progression"…  (Enter to send, Shift+Enter for a new line)',
    onkeydown: (e) => {
      if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
      e.stopPropagation();
    },
  });
  sendBtn = el('button', { class: 'primary', onclick: () => send() }, 'Send');
  stopBtn = el('button', { onclick: () => invoke('assistant_cancel'), disabled: true }, 'Stop');
  statusEl = el('span', { class: 'muted small' });
  const newChat = el('button', {
    title: 'Start a fresh conversation (the assistant forgets this chat)',
    onclick: () => { invoke('assistant_reset'); log.innerHTML = ''; showWelcome(); },
  }, 'New chat');
  panel.append(log, el('div', { class: 'chat-bar' }, input, el('div', { class: 'chat-buttons' }, sendBtn, stopBtn, newChat)), el('div', { class: 'chat-status' }, statusEl));
  showWelcome();

  listen('assistant-event', (e) => onEvent(e.payload));
  on('selection', reportContext);
  on('project', reportContext);
  invoke('assistant_status').then((s) => { available = s.available; if (!available) showUnavailable(); });
}

function showWelcome() {
  const chips = SUGGESTIONS.concat(hasMedia() ? ['Score the video: match the mood and hit the scene cuts'] : [])
    .map((s) => el('button', { class: 'chip', onclick: () => { input.value = s; send(); } }, s));
  log.append(el('div', { class: 'chat-welcome' },
    el('div', {}, el('b', {}, 'Assistant. '), 'Ask how to do something, or ask it to change your song. It sees what you have selected, and every change it makes can be undone with Ctrl+Z.'),
    el('div', { class: 'chips' }, chips)));
}

function showUnavailable() {
  log.append(el('div', { class: 'bubble error' },
    'Claude Code wasn\'t found on this computer. Install it from claude.com/claude-code, run "claude" once in a terminal to sign in, then restart Duet.'));
}

/** Minimal markdown: **bold**, `code`, and "- " bullets (text is escaped first). */
function md(text) {
  const esc = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return esc
    .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/^[-*] /gm, '• ');
}

function bubble(kind, text) {
  const b = el('div', { class: `bubble ${kind}` });
  if (kind === 'assistant') b.innerHTML = md(text);
  else b.textContent = text;
  log.append(b);
  log.scrollTop = log.scrollHeight;
  return b;
}

/** Tells the backend what's selected, so the assistant knows what "this" means. */
let ctxTimer = null;
export function reportContext() {
  clearTimeout(ctxTimer);
  ctxTimer = setTimeout(() => invoke('set_ui_context', { context: buildContext() }).catch(() => {}), 150);
}

function buildContext() {
  const p = store.project;
  if (!p) return null;
  const bt = barTicks(p);
  const t = track(store.selectedTrackId);
  const pat = pattern(store.selectedPatternId);
  const sc = store.selectedClip;
  const clip = sc && track(sc.trackId)?.clips.find((c) => c.id === sc.clipId);
  return {
    openTab: store.tab,
    selectedTrack: t ? { id: t.id, name: t.name, kind: t.kind.type, instrument: t.kind.instrument?.type } : null,
    selectedPattern: pat ? { id: pat.id, name: pat.name, bars: pat.length / bt, noteCount: pat.notes.length } : null,
    selectedNoteIds: selectedNoteIds(),
    selectedClip: clip ? { trackId: sc.trackId, clipId: clip.id, startBar: clip.start / bt + 1, bars: clip.length / bt } : null,
    loop: p.loopEnabled ? { startBar: p.loopStart / bt + 1, endBar: p.loopEnd / bt + 1 } : null,
    snap: store.snap,
    hasMedia: hasMedia(),
  };
}

export async function send(text) {
  const prompt = (text ?? input.value).trim();
  if (!prompt || busy) return;
  input.value = '';
  log.querySelector('.chat-welcome')?.remove();
  bubble('user', prompt);
  setBusy(true, 'Thinking…');
  currentBubble = null;
  try {
    await invoke('set_ui_context', { context: buildContext() });
    await invoke('assistant_ask', { prompt });
  } catch (e) {
    bubble('error', String(e));
    setBusy(false);
  }
}

function setBusy(b, status = '') {
  busy = b;
  sendBtn.disabled = b;
  stopBtn.disabled = !b;
  statusEl.textContent = status;
  emit('assistant-busy', b);
}

function onEvent(ev) {
  switch (ev.kind) {
    case 'text':
      currentBubble = bubble('assistant', ev.text);
      break;
    case 'tool':
      statusEl.textContent = `Working… (${ev.text})`;
      log.append(el('div', { class: 'tool-chip' }, `↳ ${ev.text}`));
      log.scrollTop = log.scrollHeight;
      break;
    case 'done':
      setBusy(false, ev.text ? `Done · ~${ev.text} API-equivalent (on a subscription this counts toward your usage limits instead)` : 'Done');
      break;
    case 'error':
      bubble('error', ev.text);
      setBusy(false);
      break;
  }
}

export function focusInput() {
  input.focus();
}

export const isBusy = () => busy;
export const isAvailable = () => available;
