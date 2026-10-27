// Optional video/image support: preview panel synced to the playhead, filmstrip
// thumbnails and image cache for the timeline. Nothing here is needed for audio work.
import { fileSrc } from './api.js';
import { store, on, emit, command, setResponse, asset, currentTick, hasMedia } from './store.js';
import { durationTicks, tickToSecs, toast } from './util.js';

const videos = new Map(); // assetId -> HTMLVideoElement (preview players)
const images = new Map(); // assetId -> HTMLImageElement | 'loading'
const thumbs = new Map(); // assetId -> { frames: [canvas], interval, done }
let stage, emptyMsg, info, panel;
let userHidden = false;
let active = null; // { kind, el, assetId }

export function init() {
  panel = document.getElementById('preview');
  stage = document.getElementById('preview-stage');
  emptyMsg = document.getElementById('preview-empty');
  info = document.getElementById('preview-info');
  document.getElementById('btn-preview-hide').onclick = () => { userHidden = true; updateVisibility(); };
  on('project', updateVisibility);
}

export function showPreview() {
  userHidden = false;
  updateVisibility();
}

function updateVisibility() {
  const show = hasMedia() && !userHidden;
  if (panel.hidden === !show) return;
  panel.hidden = !show;
  if (!show) for (const v of videos.values()) v.pause();
  emit('layout');
}

function mediaAt(tick) {
  const p = store.project;
  for (const t of p.tracks) {
    if (t.kind.type !== 'media') continue;
    for (const c of t.clips) {
      if (tick >= c.start && tick < c.start + c.length) return { clip: c, asset: asset(c.content.assetId) };
    }
  }
  return null;
}

function videoFor(a) {
  let v = videos.get(a.id);
  if (!v) {
    v = document.createElement('video');
    v.muted = true;
    v.preload = 'auto';
    v.playsInline = true;
    v.src = fileSrc(a.path);
    v.hidden = true;
    v.addEventListener('error', () => toast(`Can't play ${a.name}. WebView2 supports MP4 (H.264/AAC) and WebM; other formats may need converting.`, 'warn'));
    stage.append(v);
    videos.set(a.id, v);
  }
  return v;
}

export function getImage(a) {
  const img = images.get(a.id);
  if (img instanceof HTMLImageElement) return img.complete && img.naturalWidth ? img : null;
  if (!img) {
    const im = new Image();
    im.onload = () => emit('thumbs');
    im.src = fileSrc(a.path);
    images.set(a.id, im);
  }
  return null;
}

/** Filmstrip thumbnails, generated in the background with a hidden video element. */
export function getThumbs(a) {
  let t = thumbs.get(a.id);
  if (!t) {
    t = { frames: [], interval: Math.max(0.5, (a.duration || 1) / 80), done: false };
    thumbs.set(a.id, t);
    generateThumbs(a, t);
  }
  return t;
}

async function generateThumbs(a, t) {
  const v = document.createElement('video');
  v.muted = true;
  v.preload = 'auto';
  v.src = fileSrc(a.path);
  try {
    await new Promise((res, rej) => { v.onloadeddata = res; v.onerror = rej; });
  } catch {
    t.done = true;
    return;
  }
  const dur = v.duration || a.duration || 0;
  const n = Math.max(1, Math.ceil(dur / t.interval));
  const w = 96, h = 54;
  for (let i = 0; i < n; i++) {
    v.currentTime = Math.min(dur - 0.05, i * t.interval + 0.01);
    await new Promise((res) => { v.onseeked = res; setTimeout(res, 1500); });
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    const ctx = c.getContext('2d');
    const ar = v.videoWidth / v.videoHeight || 16 / 9;
    const dw = ar > w / h ? w : h * ar;
    const dh = ar > w / h ? w / ar : h;
    ctx.fillStyle = '#000';
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(v, (w - dw) / 2, (h - dh) / 2, dw, dh);
    t.frames.push(c);
    if (i % 6 === 5) emit('thumbs');
  }
  t.done = true;
  v.removeAttribute('src');
  v.load();
  emit('thumbs');
}

/** Reads a video's duration (and checks WebView2 can play it). */
export function probeDuration(path) {
  return new Promise((resolve, reject) => {
    const v = document.createElement('video');
    v.preload = 'metadata';
    v.muted = true;
    v.onloadedmetadata = () => { resolve(v.duration); v.removeAttribute('src'); v.load(); };
    v.onerror = () => reject(new Error('unsupported'));
    v.src = fileSrc(path);
  });
}

export async function addMediaFiles(files, trackId, start) {
  for (const f of files) {
    let duration = 0;
    if (f.kind === 'video') {
      try {
        duration = await probeDuration(f.path);
      } catch {
        toast(`Can't play ${f.path.split(/[\\/]/).pop()}: use MP4 (H.264/AAC) or WebM.`, 'error');
        continue;
      }
    }
    const r = await command('add_media', { path: f.path, kind: f.kind, duration, start, trackId });
    setResponse(r);
    trackId = r.project.tracks.find((t) => t.kind.type === 'media')?.id ?? trackId;
    if (f.kind === 'video') start += durationTicks(store.project, start, duration);
  }
  showPreview();
}

export async function pickAndAdd(trackId, start) {
  const paths = await command('pick_media');
  if (!paths?.length) return;
  const files = await command('classify_files', { paths });
  await addMediaFiles(files.filter((f) => f.kind === 'video' || f.kind === 'image'), trackId, start);
}

export async function extractAudio(assetId) {
  toast('Extracting audio…');
  const r = await command('extract_video_audio', { assetId });
  setResponse(r);
  toast('Audio extracted to a new track. Mute it, or use it in the mix.', 'ok');
}

function setActive(kind, el, assetId) {
  if (active?.el === el) return;
  if (active?.el) {
    active.el.hidden = true;
    if (active.kind === 'video') active.el.pause();
  }
  active = el ? { kind, el, assetId } : null;
  if (el) el.hidden = false;
  emptyMsg.hidden = !!el;
}

/** Keeps the preview locked to the transport. Called every animation frame. */
export function frame() {
  if (panel.hidden || !store.project) return;
  const tick = currentTick();
  const m = mediaAt(tick);
  if (!m || !m.asset) {
    setActive(null, null);
    info.textContent = '';
    return;
  }
  const a = m.asset;
  info.textContent = a.name;
  if (a.kind === 'image') {
    let img = stage.querySelector(`img[data-asset="${a.id}"]`);
    if (!img) {
      img = document.createElement('img');
      img.dataset.asset = a.id;
      img.src = fileSrc(a.path);
      stage.append(img);
    }
    setActive('image', img, a.id);
    return;
  }
  const v = videoFor(a);
  setActive('video', v, a.id);
  const p = store.project;
  const target = m.clip.offset + tickToSecs(p, tick) - tickToSecs(p, m.clip.start);
  if (v.readyState < 1) return;
  if (target >= v.duration) {
    if (!v.paused) v.pause();
    return;
  }
  const playing = store.transport.playing;
  const drift = v.currentTime - target;
  if (playing) {
    if (v.paused) {
      v.currentTime = target;
      v.play().catch(() => {});
    } else if (Math.abs(drift) > 0.25) {
      if (!v.seeking) v.currentTime = target;
    } else if (Math.abs(drift) > 1 / 30) {
      // Nudge the rate rather than seeking: seeks stutter, small rate changes don't.
      v.playbackRate = Math.min(1.1, Math.max(0.9, 1 - drift * 1.5));
    } else if (v.playbackRate !== 1) {
      v.playbackRate = 1;
    }
  } else {
    if (!v.paused) v.pause();
    if (Math.abs(drift) > 0.02 && !v.seeking) v.currentTime = target;
  }
}
