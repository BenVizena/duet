//! Compact text notation for notes, used by the control API (and handy for templates).
//!
//! Lengths are in 16th-note steps (fractions allowed: 1.333 for 16th triplets).
//! Lines (or `;`-separated parts) are independent voices that each start at step 0.
//!
//! Token lines:  `G5:4 D5:2 r:2 C4+E4+G4:8@0.6 | kick:4`
//!   - `NAME:LEN` note, `NAME:LEN@VEL` with velocity 0..1, `r:LEN` rest, `|` ignored
//!   - chords join names with `+`; a missing `:LEN` reuses the previous length
//!   - names: C4 = middle C (60), sharps `F#3`, flats `Bb2`, MIDI numbers `60`, or drum names
//! Grid lines (best for drums):  `kick: x...x...x...x...`
//!   - one character per 16th: `x` hit, `X` accent, `o` soft, `.` or `-` rest; `|` and spaces ignored

use crate::model::{Note, PPQ};

const STEP: f64 = (PPQ / 4) as f64;

pub const DRUM_NAMES: [(&str, u8); 20] = [
    ("kick", 36),
    ("bd", 36),
    ("rim", 37),
    ("snare", 38),
    ("sd", 38),
    ("clap", 39),
    ("hat", 42),
    ("hh", 42),
    ("chh", 42),
    ("phh", 44),
    ("ohat", 46),
    ("oh", 46),
    ("tom1", 50),
    ("htom", 50),
    ("tom2", 45),
    ("mtom", 45),
    ("tom3", 41),
    ("ltom", 41),
    ("crash", 49),
    ("ride", 51),
];

pub fn pitch_of(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    if let Some((_, p)) = DRUM_NAMES.iter().find(|(n, _)| *n == lower) {
        return Some(*p);
    }
    if let Ok(n) = name.parse::<u8>() {
        return (n <= 127).then_some(n);
    }
    let mut chars = name.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let mut pc: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let mut digits = rest.as_str();
    if let Some(r) = digits.strip_prefix('#') {
        pc += 1;
        digits = r;
    } else if let Some(r) = digits.strip_prefix('b') {
        pc -= 1;
        digits = r;
    }
    let oct: i32 = digits.parse().ok()?;
    let n = (oct + 1) * 12 + pc;
    (0..=127).contains(&n).then_some(n as u8)
}

pub fn note_name(p: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[(p % 12) as usize], p as i32 / 12 - 1)
}

fn is_grid(body: &str) -> bool {
    !body.trim().is_empty() && body.chars().all(|c| matches!(c, 'x' | 'X' | 'o' | '.' | '-' | '|' | ' '))
}

/// Parses notation into notes. Errors name the offending token.
pub fn parse(text: &str) -> Result<Vec<Note>, String> {
    let mut out = vec![];
    for (li, line) in text.split(['\n', ';']).enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some((name, body)) = line.split_once(':') {
            if is_grid(body) && !name.trim().contains(' ') {
                let pitch = pitch_of(name.trim()).ok_or_else(|| format!("line {}: unknown note or drum '{}'", li + 1, name.trim()))?;
                let mut step = 0.0;
                for c in body.chars() {
                    let vel = match c {
                        'x' => Some(0.85),
                        'X' => Some(1.0),
                        'o' => Some(0.55),
                        '.' | '-' => None,
                        _ => continue,
                    };
                    if let Some(v) = vel {
                        out.push(Note { id: 0, pitch, start: (step * STEP) as u64, length: STEP as u64, velocity: v });
                    }
                    step += 1.0;
                }
                continue;
            }
        }
        let mut step = 0.0f64;
        let mut len = 4.0f64;
        for tok in line.split_whitespace() {
            if tok == "|" {
                continue;
            }
            let (body, vel) = match tok.split_once('@') {
                Some((b, v)) => (b, v.parse::<f32>().map_err(|_| format!("bad velocity in '{tok}'"))?.clamp(0.0, 1.0)),
                None => (tok, 0.8),
            };
            let (names, l) = match body.split_once(':') {
                Some((n, l)) => (n, Some(l)),
                None => (body, None),
            };
            if let Some(l) = l {
                len = parse_len(l).ok_or_else(|| format!("bad length in '{tok}'"))?;
            }
            if names.eq_ignore_ascii_case("r") || names == "-" {
                step += len;
                continue;
            }
            for n in names.split('+') {
                let pitch = pitch_of(n).ok_or_else(|| format!("unknown note '{n}' in '{tok}'"))?;
                out.push(Note {
                    id: 0,
                    pitch,
                    start: (step * STEP).round() as u64,
                    length: ((len * STEP).round() as u64).max(1),
                    velocity: vel,
                });
            }
            step += len;
        }
    }
    out.sort_by_key(|n| (n.start, n.pitch));
    Ok(out)
}

fn parse_len(s: &str) -> Option<f64> {
    let v = match s.split_once('/') {
        Some((a, b)) => a.parse::<f64>().ok()? / b.parse::<f64>().ok()?,
        None => s.parse::<f64>().ok()?,
    };
    (v > 0.0 && v.is_finite()).then_some(v)
}

/// End of the last note, in ticks.
pub fn end_tick(notes: &[Note]) -> u64 {
    notes.iter().map(|n| n.start + n.length).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tokens_chords_and_grids() {
        let n = parse("C4:4 r:2 E4+G4:2@0.5 A4\nkick: x...X...").unwrap();
        let s: Vec<_> = n.iter().map(|n| (n.pitch, n.start, n.length)).collect();
        assert!(s.contains(&(60, 0, 96)));
        assert!(s.contains(&(64, 144, 48)));
        assert!(s.contains(&(67, 144, 48)));
        assert!(s.contains(&(69, 192, 48)));
        assert!(s.contains(&(36, 0, 24)));
        assert!(s.contains(&(36, 96, 24)));
        assert_eq!(n.iter().find(|n| n.start == 96 && n.pitch == 36).unwrap().velocity, 1.0);
        assert!(parse("H4:2").is_err());
    }
}
