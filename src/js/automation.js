// Automation target metadata: display names, value ranges and formatting.
import { store, track } from './store.js';
import { gainToDb } from './util.js';

const FX_NAMES = {
  eq: 'EQ', filter: 'Filter', delay: 'Delay', reverb: 'Reverb', compressor: 'Compressor', chorus: 'Chorus', bitcrusher: 'Bitcrusher', pump: 'Pump',
};

function effectsOf(ownerId) {
  return ownerId === null ? store.project.master.effects : track(ownerId)?.effects || [];
}

/** { label, min, max, log?, fmt(v), def } for a target on a track (ownerId) or the master (null). */
export function targetInfo(target, ownerId) {
  switch (target) {
    case 'volume': return { label: 'Volume', min: 0, max: 1.5, fmt: (v) => `${gainToDb(v)} dB`, def: ownerId === null ? store.project.master.volume : track(ownerId)?.volume ?? 0.8 };
    case 'pan': return { label: 'Pan', min: -1, max: 1, fmt: (v) => (Math.abs(v) < 0.01 ? 'C' : `${Math.round(Math.abs(v) * 100)}${v < 0 ? 'L' : 'R'}`), def: track(ownerId)?.pan ?? 0 };
    case 'cutoff': return { label: 'Filter cutoff', min: -4, max: 4, fmt: (v) => `${v > 0 ? '+' : ''}${v.toFixed(2)} oct`, def: 0 };
    case 'pitch': return { label: 'Pitch bend', min: -12, max: 12, fmt: (v) => `${v > 0 ? '+' : ''}${v.toFixed(2)} st`, def: 0 };
  }
  const m = /^fx:(\d+):(mix|cutoff)$/.exec(target);
  if (m) {
    const fx = effectsOf(ownerId).find((e) => e.id === +m[1]);
    const name = fx ? FX_NAMES[fx.kind.type] || fx.kind.type : 'Missing effect';
    if (m[2] === 'mix') return { label: `${name} mix`, min: 0, max: 1, fmt: (v) => `${Math.round(v * 100)}%`, def: 1 };
    return { label: `${name} cutoff`, min: 20, max: 18000, log: true, fmt: (v) => (v >= 1000 ? `${(v / 1000).toFixed(1)} kHz` : `${Math.round(v)} Hz`), def: fx?.kind.cutoff ?? 2000 };
  }
  return { label: target, min: 0, max: 1, fmt: (v) => v.toFixed(2), def: 0 };
}

/** Targets that can be automated on a track (or master when ownerId is null). */
export function availableTargets(ownerId) {
  const out = [{ target: 'volume', label: 'Volume' }];
  const t = ownerId === null ? null : track(ownerId);
  if (t) {
    out.push({ target: 'pan', label: 'Pan' });
    if (t.kind.type === 'instrument') {
      out.push({ target: 'pitch', label: 'Pitch bend' });
      if (t.kind.instrument.type === 'synth') out.push({ target: 'cutoff', label: 'Filter cutoff (synth filter must be on)' });
    }
  }
  for (const e of effectsOf(ownerId)) {
    const name = FX_NAMES[e.kind.type] || e.kind.type;
    out.push({ target: `fx:${e.id}:mix`, label: `${name} mix` });
    if (e.kind.type === 'filter') out.push({ target: `fx:${e.id}:cutoff`, label: `${name} cutoff` });
  }
  return out;
}

export function valueToNorm(info, v) {
  if (info.log) return (Math.log(Math.max(v, info.min)) - Math.log(info.min)) / (Math.log(info.max) - Math.log(info.min));
  return (v - info.min) / (info.max - info.min);
}

export function normToValue(info, n) {
  n = Math.min(1, Math.max(0, n));
  if (info.log) return Math.exp(Math.log(info.min) + n * (Math.log(info.max) - Math.log(info.min)));
  return info.min + n * (info.max - info.min);
}
