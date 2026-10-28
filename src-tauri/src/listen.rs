//! "Listening" to recorded audio for the assistant, which can't hear: finds note
//! onsets, the pitch of a hummed/sung line, a tempo estimate and a key guess, so a
//! recording can be turned into rhythm and melody the assistant can build on.

/// Analysis frame hop in seconds.
pub const HOP: f64 = 0.01;
const TARGET_SR: f32 = 16000.0;
const FMIN: f32 = 65.0;
const FMAX: f32 = 1100.0;

pub struct Event {
    /// Seconds from the start of the analysed audio.
    pub secs: f64,
    pub dur: f64,
    /// Median pitch as a fractional MIDI number; None for unpitched sounds (taps, consonants, beatbox).
    pub midi: Option<f32>,
    /// Peak frame level in dBFS.
    pub db: f32,
    /// Started by a pitch change inside a sustained sound rather than a new attack.
    pub pitch_change: bool,
}

pub struct Analysis {
    pub duration: f64,
    pub peak_db: f32,
    pub floor_db: f32,
    /// Share of sounding frames that have a clear pitch.
    pub voiced_ratio: f32,
    /// Level of each HOP frame in dBFS.
    pub frame_db: Vec<f32>,
    pub events: Vec<Event>,
    /// (bpm, confidence 0..1)
    pub tempo: Option<(f64, f64)>,
    /// Tuning offset of the singer in cents (pitches are rounded after removing it).
    pub tuning_cents: f32,
    /// (root 0-11, "major" | "minor")
    pub key: Option<(u8, &'static str)>,
}

fn db_of(x: f32) -> f32 {
    20.0 * x.max(1e-6).log10()
}

fn percentile(v: &[f32], q: f32) -> f32 {
    if v.is_empty() {
        return -120.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[((s.len() - 1) as f32 * q).round() as usize]
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Mono, roughly 16 kHz (box-filtered decimation; plenty for voice pitch and onsets), DC removed.
fn downmix(l: &[f32], r: &[f32], sr: u32) -> (Vec<f32>, f32) {
    let k = ((sr as f32 / TARGET_SR).round() as usize).max(1);
    let mut x: Vec<f32> = l
        .chunks(k)
        .zip(r.chunks(k))
        .map(|(a, b)| (a.iter().sum::<f32>() + b.iter().sum::<f32>()) / (2 * a.len()) as f32)
        .collect();
    let mean = x.iter().sum::<f32>() / x.len().max(1) as f32;
    for v in &mut x {
        *v -= mean;
    }
    (x, sr as f32 / k as f32)
}

/// YIN pitch detector. Returns (period in samples, aperiodicity) for `x[..w + tmax + 2]`.
fn yin(x: &[f32], w: usize, tmin: usize, tmax: usize, d: &mut Vec<f32>) -> Option<(f32, f32)> {
    d.clear();
    d.resize(tmax + 2, 0.0);
    for tau in 1..=tmax + 1 {
        let mut s = 0.0;
        for j in 0..w {
            let v = x[j] - x[j + tau];
            s += v * v;
        }
        d[tau] = s;
    }
    d[0] = 1.0;
    let mut run = 0.0;
    for tau in 1..=tmax + 1 {
        run += d[tau];
        d[tau] = if run > 0.0 { d[tau] * tau as f32 / run } else { 1.0 };
    }
    let mut found = None;
    let mut tau = tmin;
    while tau <= tmax {
        if d[tau] < 0.15 {
            while tau < tmax && d[tau + 1] < d[tau] {
                tau += 1;
            }
            found = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = match found {
        Some(t) => t,
        None => {
            let t = (tmin..=tmax).min_by(|a, b| d[*a].total_cmp(&d[*b]))?;
            if d[t] > 0.35 {
                return None;
            }
            t
        }
    };
    let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-1.0, 1.0) } else { 0.0 };
    Some((tau as f32 + shift, b))
}

pub fn analyze(l: &[f32], r: &[f32], sr: u32) -> Analysis {
    let (x, fs) = downmix(l, r, sr);
    let hop = (fs as f64 * HOP).round() as usize;
    let n = x.len() / hop.max(1);
    let duration = l.len() as f64 / sr.max(1) as f64;
    let peak_db = db_of(x.iter().fold(0f32, |m, v| m.max(v.abs())));

    // Level per frame (20 ms window centred on the frame).
    let frame_db: Vec<f32> = (0..n)
        .map(|i| {
            let s = (i * hop).saturating_sub(hop / 2);
            let e = (i * hop + hop * 3 / 2).min(x.len());
            let ms = x[s..e].iter().map(|v| v * v).sum::<f32>() / (e - s).max(1) as f32;
            10.0 * (ms + 1e-12).log10()
        })
        .collect();
    let floor_db = percentile(&frame_db, 0.1);
    let loud = percentile(&frame_db, 0.97);
    let gate = (floor_db + 15.0).min(loud - 15.0).max(-65.0);
    let active: Vec<bool> = frame_db.iter().map(|d| *d > gate).collect();

    // Pitch per sounding frame.
    let tmin = (fs / FMAX).floor().max(2.0) as usize;
    let tmax = (fs / FMIN).ceil() as usize;
    let w = (fs * 0.03) as usize;
    let mut raw: Vec<Option<f32>> = vec![None; n];
    let mut scratch = Vec::new();
    for i in 0..n {
        if !active[i] {
            continue;
        }
        let s = (i * hop).saturating_sub(w / 2);
        if s + w + tmax + 2 > x.len() {
            continue;
        }
        if let Some((tau, ap)) = yin(&x[s..], w, tmin, tmax, &mut scratch) {
            if ap < 0.25 {
                raw[i] = Some(69.0 + 12.0 * (fs / tau / 440.0).log2());
            }
        }
    }
    // Median-smooth; drop isolated voiced frames.
    let midi: Vec<Option<f32>> = (0..n)
        .map(|i| {
            raw[i]?;
            let mut win: Vec<f32> = raw[i.saturating_sub(2)..(i + 3).min(n)].iter().flatten().copied().collect();
            if win.len() < 3 {
                return None;
            }
            Some(median(&mut win))
        })
        .collect();
    let sounding = active.iter().filter(|a| **a).count();
    let voiced_ratio = midi.iter().filter(|m| m.is_some()).count() as f32 / sounding.max(1) as f32;

    // Onsets: level rises of 4+ dB over 20 ms, peak-picked, at least 70 ms apart.
    let odf: Vec<f32> = (0..n)
        .map(|i| if i < 2 || !active[i] { 0.0 } else { (frame_db[i] - frame_db[i - 2].max(gate - 6.0)).max(0.0) })
        .collect();
    let mut onsets: Vec<usize> = vec![];
    for i in 0..n {
        let v = odf[i];
        if v < 4.0 {
            continue;
        }
        let lo = i.saturating_sub(5);
        let hi = (i + 6).min(n);
        if odf[lo..i].iter().any(|u| *u >= v) || odf[i + 1..hi].iter().any(|u| *u > v) {
            continue;
        }
        let mut j = i;
        while j > 0 && i - j < 3 && odf[j - 1] > 1.0 {
            j -= 1;
        }
        if onsets.last().map(|l| j < l + 7).unwrap_or(false) {
            continue;
        }
        onsets.push(j);
    }
    // Sounds that fade in without a sharp attack still start a note.
    for i in 1..n {
        if active[i] && !active[i - 1] && active[i..(i + 5).min(n)].iter().all(|a| *a) && !onsets.iter().any(|o| o.abs_diff(i) < 8) {
            onsets.push(i);
        }
    }
    onsets.sort_unstable();

    // Pitch changes inside sustained sounds (legato humming) also start notes.
    let mut is_onset = vec![false; n];
    for o in &onsets {
        is_onset[*o] = true;
    }
    let mut splits = vec![];
    let (mut reference, mut run_start, mut count): (Option<f32>, usize, usize) = (None, 0, 0);
    for i in 0..n {
        if is_onset[i] || !active[i] {
            reference = None;
            count = 0;
        }
        let Some(m) = midi[i] else {
            count = 0;
            continue;
        };
        match reference {
            None => reference = Some(m),
            Some(rf) if (m - rf).abs() > 0.7 => {
                if count == 0 {
                    run_start = i;
                }
                count += 1;
                if count >= 5 {
                    if !onsets.iter().any(|o| o.abs_diff(run_start) < 8) {
                        splits.push(run_start);
                    }
                    reference = Some(m);
                    count = 0;
                }
            }
            Some(rf) => {
                count = 0;
                reference = Some(rf * 0.9 + m * 0.1);
            }
        }
    }

    let mut bounds: Vec<(usize, bool)> = onsets.iter().map(|o| (*o, false)).chain(splits.iter().map(|s| (*s, true))).collect();
    bounds.sort_unstable();
    let mut events = vec![];
    for (k, &(b, pitch_change)) in bounds.iter().enumerate() {
        let nb = bounds.get(k + 1).map(|x| x.0).unwrap_or(n);
        // Ends at the first 40 ms of silence (or the next note).
        let mut e = b + 1;
        let mut quiet = 0;
        for f in b..nb {
            if active[f] {
                e = f + 1;
                quiet = 0;
            } else {
                quiet += 1;
                if quiet >= 4 {
                    break;
                }
            }
        }
        if (e - b) < 5 {
            continue;
        }
        let mut pitches: Vec<f32> = midi[b..e].iter().flatten().copied().collect();
        let pitch = if pitches.len() >= 3.max((e - b) * 3 / 10) { Some(median(&mut pitches)) } else { None };
        let db = frame_db[b..e].iter().fold(-120f32, |m, v| m.max(*v));
        // A short wobble inside a held note is an ornament, not a new note.
        if let Some(prev) = events.last_mut().filter(|p: &&mut Event| pitch_change && e - b < 10 && (p.secs + p.dur - b as f64 * HOP).abs() < 0.015) {
            prev.dur = e as f64 * HOP - prev.secs;
            continue;
        }
        events.push(Event { secs: b as f64 * HOP, dur: (e - b) as f64 * HOP, midi: pitch, db, pitch_change });
    }

    let tuning_cents = tuning(&events);
    let key = guess_key(&events, tuning_cents);
    Analysis { duration, peak_db, floor_db, voiced_ratio, frame_db, tempo: estimate_tempo(&events, n), tuning_cents, key, events }
}

/// Rounds a detected pitch to a MIDI note after removing the singer's tuning offset.
pub fn note_of(midi: f32, tuning_cents: f32) -> u8 {
    (midi - tuning_cents / 100.0).round().clamp(0.0, 127.0) as u8
}

/// Average deviation from equal temperament (circular mean, so 49 and -49 cents don't cancel badly).
fn tuning(events: &[Event]) -> f32 {
    let (mut s, mut c) = (0f32, 0f32);
    for e in events {
        if let Some(m) = e.midi {
            let a = (m - m.round()) * std::f32::consts::TAU;
            s += a.sin() * e.dur as f32;
            c += a.cos() * e.dur as f32;
        }
    }
    if s == 0.0 && c == 0.0 {
        0.0
    } else {
        s.atan2(c) / std::f32::consts::TAU * 100.0
    }
}

/// Krumhansl-Kessler key profiles against the duration-weighted pitch classes.
fn guess_key(events: &[Event], tuning_cents: f32) -> Option<(u8, &'static str)> {
    const MAJ: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MIN: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut hist = [0f32; 12];
    let mut distinct = std::collections::HashSet::new();
    for e in events {
        if let Some(m) = e.midi {
            let pc = note_of(m, tuning_cents) % 12;
            hist[pc as usize] += e.dur as f32;
            distinct.insert(pc);
        }
    }
    if distinct.len() < 3 {
        return None;
    }
    let corr = |prof: &[f32; 12], root: usize| {
        let mh = hist.iter().sum::<f32>() / 12.0;
        let mp = prof.iter().sum::<f32>() / 12.0;
        let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let a = hist[(i + root) % 12] - mh;
            let b = prof[i] - mp;
            sxy += a * b;
            sxx += a * a;
            syy += b * b;
        }
        sxy / (sxx * syy).sqrt().max(1e-9)
    };
    let mut best = (f32::MIN, 0u8, "major");
    for root in 0..12 {
        for (prof, name) in [(&MAJ, "major"), (&MIN, "minor")] {
            let c = corr(prof, root);
            if c > best.0 {
                best = (c, root as u8, name);
            }
        }
    }
    Some((best.1, best.2))
}

/// Autocorrelation of a smoothed onset train, searched over 60-180 BPM with a mild
/// preference for moderate tempos (avoids picking half/double time).
fn estimate_tempo(events: &[Event], n: usize) -> Option<(f64, f64)> {
    if events.len() < 4 || n < 300 {
        return None;
    }
    let max_db = events.iter().fold(-120f32, |m, e| m.max(e.db));
    let mut env = vec![0f32; n + 4];
    for e in events {
        let c = (e.secs / HOP).round() as isize;
        let wgt = (1.0 + (e.db - max_db) / 30.0).max(0.3) * if e.pitch_change { 0.6 } else { 1.0 };
        for k in -4isize..=4 {
            let i = c + k;
            if i >= 0 && (i as usize) < env.len() {
                env[i as usize] += wgt * (-(k * k) as f32 / 8.0).exp();
            }
        }
    }
    let ac = |lag: usize| -> f32 {
        if lag >= env.len() {
            return 0.0;
        }
        (0..env.len() - lag).map(|i| env[i] * env[i + lag]).sum::<f32>() / (env.len() - lag) as f32
    };
    let zero = ac(0).max(1e-9);
    let lag_of = |bpm: f64| 60.0 / bpm / HOP;
    let (lo, hi) = (lag_of(180.0).floor() as usize, lag_of(60.0).ceil() as usize);
    let scores: Vec<f32> = (lo..=hi + 1)
        .map(|lag| {
            let bpm = 60.0 / (lag as f64 * HOP);
            let prior = (-0.5 * ((bpm / 110.0).log2() / 0.9).powi(2)).exp() as f32;
            (ac(lag) + 0.5 * ac(lag * 2) + 0.25 * ac(lag / 2)) * prior
        })
        .collect();
    let i = (1..scores.len() - 1).max_by(|a, b| scores[*a].total_cmp(&scores[*b]))?;
    let (a, b, c) = (scores[i - 1], scores[i], scores[i + 1]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
    let lag = (lo + i) as f64 + shift as f64;
    let conf = (ac(lag.round() as usize) / zero).clamp(0.0, 1.0) as f64;
    Some((60.0 / (lag * HOP), conf))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hum-like tone (fundamental plus a few harmonics) with short attacks.
    fn hum(notes: &[(f64, f64, f32)], sr: u32, len: f64) -> Vec<f32> {
        let mut out = vec![0f32; (len * sr as f64) as usize];
        for &(start, dur, midi) in notes {
            let f = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
            let s = (start * sr as f64) as usize;
            let e = ((start + dur) * sr as f64) as usize;
            for i in s..e.min(out.len()) {
                let t = (i - s) as f32 / sr as f32;
                let env = (t / 0.01).min(1.0) * (((e - i) as f32 / sr as f32) / 0.02).min(1.0);
                let ph = std::f32::consts::TAU * f * t;
                out[i] += 0.3 * env * (ph.sin() + 0.5 * (2.0 * ph).sin() + 0.25 * (3.0 * ph).sin());
            }
        }
        out
    }

    #[test]
    fn finds_hummed_notes_and_tempo() {
        // 120 BPM eighth notes (0.25 s apart), a C major figure.
        let pitches = [60.0, 64.0, 67.0, 64.0, 60.0, 62.0, 64.0, 65.0, 67.0, 67.0, 65.0, 64.0, 62.0, 60.0, 62.0, 60.0];
        let notes: Vec<(f64, f64, f32)> = pitches.iter().enumerate().map(|(i, m)| (0.5 + i as f64 * 0.25, 0.18, *m)).collect();
        let sr = 48000;
        let x = hum(&notes, sr, 5.0);
        let a = analyze(&x, &x, sr);
        assert_eq!(a.events.len(), notes.len(), "events: {:?}", a.events.iter().map(|e| e.secs).collect::<Vec<_>>());
        for (e, n) in a.events.iter().zip(&notes) {
            assert!((e.secs - n.0).abs() < 0.03, "onset {} vs {}", e.secs, n.0);
            assert_eq!(note_of(e.midi.expect("pitched"), a.tuning_cents), n.2 as u8);
        }
        let (bpm, _) = a.tempo.expect("tempo");
        assert!((bpm - 120.0).abs() < 4.0 || (bpm - 240.0).abs() < 8.0 || (bpm - 60.0).abs() < 2.0, "bpm {bpm}");
        assert_eq!(a.key, Some((0, "major")));
    }

    #[test]
    fn splits_legato_pitch_changes() {
        // One continuous hum that steps A3 -> C4 -> E4 without breaks.
        let sr = 44100;
        let len = 2.0;
        let mut x = vec![0f32; (len * sr as f64) as usize];
        let mut ph = 0f32;
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f32 / sr as f32;
            let midi = if t < 0.6 { 57.0 } else if t < 1.2 { 60.0 } else { 64.0 };
            let env = if (0.1..1.8).contains(&t) { 1.0 } else { 0.0 };
            ph += std::f32::consts::TAU * 440.0 * 2f32.powf((midi - 69.0) / 12.0) / sr as f32;
            *v = 0.3 * env * (ph.sin() + 0.4 * (2.0 * ph).sin());
        }
        let a = analyze(&x, &x, sr);
        let got: Vec<u8> = a.events.iter().filter_map(|e| e.midi.map(|m| note_of(m, a.tuning_cents))).collect();
        assert_eq!(got, vec![57, 60, 64]);
    }
}

