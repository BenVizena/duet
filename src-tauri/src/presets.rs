//! Built-in instrument presets and starter templates.

use crate::edit::{apply, Edit};
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

pub const USER_CATEGORY: &str = "My presets";

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub name: String,
    pub category: String,
    pub instrument: Instrument,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

fn synth(f: impl FnOnce(&mut SynthParams)) -> Instrument {
    let mut p = SynthParams::default();
    f(&mut p);
    Instrument::Synth(p)
}

fn drums(style: &str, gain: f32) -> Instrument {
    Instrument::Drums(DrumParams { style: style.into(), gain, ..DrumParams::default() })
}

pub fn default_instrument() -> Instrument {
    preset("Pulse Lead 12.5%").unwrap_or_else(|| synth(|_| {}))
}

pub fn preset(name: &str) -> Option<Instrument> {
    presets().into_iter().find(|p| p.name == name).map(|p| p.instrument)
}

/// Built-in or user preset, by name (case-insensitive).
pub fn preset_any(name: &str) -> Option<Instrument> {
    all_presets().into_iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.instrument)
}

// ---------- user presets (stored as JSON in the app data folder) ----------

static USER: Mutex<Option<(PathBuf, Vec<Preset>)>> = Mutex::new(None);

pub fn load_user_presets(data_dir: &std::path::Path) {
    let path = data_dir.join("user_presets.json");
    let list: Vec<Preset> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    *USER.lock().unwrap() = Some((path, list));
}

pub fn user_presets() -> Vec<Preset> {
    USER.lock().unwrap().as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
}

/// Built-in presets followed by the user's own.
pub fn all_presets() -> Vec<Preset> {
    let mut v = presets();
    v.extend(user_presets());
    v
}

fn save_user(list: &[Preset], path: &std::path::Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("Can't save presets: {e}"))
}

/// Saves (or overwrites) a user preset.
pub fn save_user_preset(name: &str, instrument: Instrument) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Preset name can't be empty".into());
    }
    if presets().iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        return Err(format!("'{name}' is a built-in preset name; pick another"));
    }
    let mut g = USER.lock().unwrap();
    let (path, list) = g.as_mut().ok_or("Presets not loaded")?;
    list.retain(|p| !p.name.eq_ignore_ascii_case(name));
    list.push(Preset { name: name.into(), category: USER_CATEGORY.into(), instrument });
    save_user(list, path)
}

pub fn delete_user_preset(name: &str) -> Result<(), String> {
    let mut g = USER.lock().unwrap();
    let (path, list) = g.as_mut().ok_or("Presets not loaded")?;
    let before = list.len();
    list.retain(|p| p.name != name);
    if list.len() == before {
        return Err(format!("No user preset named '{name}'"));
    }
    save_user(list, path)
}

pub fn presets() -> Vec<Preset> {
    let mut v = Vec::new();
    let mut add = |cat: &str, name: &str, inst: Instrument| {
        v.push(Preset { name: name.into(), category: cat.into(), instrument: inst })
    };

    // --- Chip (Game Boy / NES) ---
    let chip_lead = |duty: f32| {
        synth(move |p| {
            p.wave = Wave::Pulse;
            p.duty = duty;
            p.decay = 0.4;
            p.sustain = 0.65;
            p.release = 0.06;
            p.vibrato_depth = 0.22;
            p.vibrato_rate = 5.8;
            p.vibrato_delay = 0.28;
            p.gain = 0.3;
        })
    };
    add("Chip", "Pulse Lead 12.5%", chip_lead(0.125));
    add("Chip", "Pulse Lead 25%", chip_lead(0.25));
    add("Chip", "Pulse Lead 50%", chip_lead(0.5));
    add(
        "Chip",
        "Pulse Harmony",
        synth(|p| {
            p.wave = Wave::Pulse;
            p.duty = 0.25;
            p.decay = 0.25;
            p.sustain = 0.45;
            p.release = 0.05;
            p.gain = 0.2;
        }),
    );
    add(
        "Chip",
        "Wave Bass",
        synth(|p| {
            p.wave = Wave::Wavetable;
            p.wavetable = vec![
                8, 10, 12, 13, 14, 15, 15, 15, 14, 13, 12, 10, 9, 8, 7, 7, 6, 5, 4, 3, 2, 1, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7,
            ];
            p.attack = 0.0;
            p.decay = 0.2;
            p.sustain = 1.0;
            p.release = 0.03;
            p.gain = 0.42;
        }),
    );
    add(
        "Chip",
        "Wave Soft Lead",
        synth(|p| {
            p.wave = Wave::Wavetable;
            p.wavetable = (0..32).map(|i| (7.5 + 7.5 * (i as f32 / 32.0 * std::f32::consts::TAU).sin()).round() as u8).collect();
            p.sustain = 0.8;
            p.release = 0.1;
            p.vibrato_depth = 0.25;
            p.vibrato_delay = 0.3;
            p.gain = 0.35;
        }),
    );
    add(
        "Chip",
        "Triangle Bass",
        synth(|p| {
            p.wave = Wave::Triangle;
            p.bits = 4;
            p.sustain = 1.0;
            p.release = 0.02;
            p.gain = 0.5;
        }),
    );
    add(
        "Chip",
        "Arp Major",
        synth(|p| {
            p.duty = 0.25;
            p.arp = vec![0, 4, 7, 12];
            p.arp_speed = 28.0;
            p.sustain = 0.6;
            p.gain = 0.22;
        }),
    );
    add(
        "Chip",
        "Arp Minor",
        synth(|p| {
            p.duty = 0.25;
            p.arp = vec![0, 3, 7, 12];
            p.arp_speed = 28.0;
            p.sustain = 0.6;
            p.gain = 0.22;
        }),
    );
    add(
        "Chip",
        "Blip / SFX",
        synth(|p| {
            p.duty = 0.5;
            p.sweep = 12.0;
            p.sweep_time = 0.04;
            p.decay = 0.12;
            p.sustain = 0.0;
            p.release = 0.05;
            p.gain = 0.25;
        }),
    );
    add(
        "Chip",
        "Noise Hit",
        synth(|p| {
            p.wave = Wave::Noise;
            p.decay = 0.15;
            p.sustain = 0.0;
            p.gain = 0.25;
        }),
    );
    add("Chip", "Chip Drum Kit", drums("chip", 0.55));

    // --- Orchestral (GBA / SNES era) ---
    add(
        "Orchestral",
        "Strings Ensemble",
        synth(|p| {
            p.wave = Wave::Saw;
            p.unison = 5;
            p.detune = 16.0;
            p.filter_on = true;
            p.cutoff = 3200.0;
            p.attack = 0.14;
            p.decay = 0.5;
            p.sustain = 0.9;
            p.release = 0.4;
            p.vibrato_depth = 0.08;
            p.vibrato_rate = 5.0;
            p.vibrato_delay = 0.3;
            p.gain = 0.2;
        }),
    );
    add(
        "Orchestral",
        "Brass Section",
        synth(|p| {
            p.wave = Wave::Saw;
            p.unison = 3;
            p.detune = 8.0;
            p.filter_on = true;
            p.cutoff = 800.0;
            p.filter_env = 2.4;
            p.filter_decay = 0.35;
            p.attack = 0.04;
            p.sustain = 0.8;
            p.release = 0.15;
            p.vibrato_depth = 0.1;
            p.vibrato_delay = 0.35;
            p.gain = 0.26;
        }),
    );
    add(
        "Orchestral",
        "Flute",
        synth(|p| {
            p.wave = Wave::Sine;
            p.attack = 0.06;
            p.sustain = 0.85;
            p.release = 0.12;
            p.vibrato_depth = 0.18;
            p.vibrato_rate = 5.2;
            p.vibrato_delay = 0.2;
            p.gain = 0.35;
        }),
    );
    add(
        "Orchestral",
        "Harp / Pizzicato",
        synth(|p| {
            p.wave = Wave::Triangle;
            p.filter_on = true;
            p.cutoff = 2500.0;
            p.decay = 0.45;
            p.sustain = 0.0;
            p.release = 0.3;
            p.gain = 0.4;
        }),
    );
    add(
        "Orchestral",
        "Choir Pad",
        synth(|p| {
            p.wave = Wave::Saw;
            p.unison = 4;
            p.detune = 12.0;
            p.filter_on = true;
            p.cutoff = 1300.0;
            p.attack = 0.4;
            p.sustain = 0.9;
            p.release = 0.8;
            p.vibrato_depth = 0.1;
            p.gain = 0.2;
        }),
    );
    add(
        "Orchestral",
        "Timpani",
        synth(|p| {
            p.wave = Wave::Sine;
            p.octave = -1;
            p.sweep = 3.0;
            p.sweep_time = 0.08;
            p.decay = 0.8;
            p.sustain = 0.0;
            p.release = 0.5;
            p.fm_amount = 0.6;
            p.fm_ratio = 1.5;
            p.fm_decay = 0.15;
            p.gain = 0.5;
        }),
    );
    add("Orchestral", "Orchestra Drums", drums("clean", 0.6));

    // --- Synthwave / Electronic ---
    add(
        "Synthwave",
        "Supersaw Lead",
        synth(|p| {
            p.wave = Wave::Saw;
            p.unison = 7;
            p.detune = 24.0;
            p.filter_on = true;
            p.cutoff = 6500.0;
            p.attack = 0.01;
            p.sustain = 0.85;
            p.release = 0.3;
            p.vibrato_depth = 0.1;
            p.vibrato_delay = 0.4;
            p.gain = 0.17;
        }),
    );
    add(
        "Synthwave",
        "Neon Pad",
        synth(|p| {
            p.wave = Wave::Saw;
            p.unison = 6;
            p.detune = 18.0;
            p.filter_on = true;
            p.cutoff = 1500.0;
            p.resonance = 0.2;
            p.lfo_rate = 0.25;
            p.lfo_sync = true;
            p.lfo_cutoff = 0.9;
            p.attack = 0.6;
            p.sustain = 0.9;
            p.release = 1.2;
            p.gain = 0.17;
        }),
    );
    add(
        "Synthwave",
        "Neon Arp Pluck",
        synth(|p| {
            p.wave = Wave::Saw;
            p.unison = 2;
            p.detune = 10.0;
            p.filter_on = true;
            p.cutoff = 450.0;
            p.resonance = 0.35;
            p.filter_env = 3.8;
            p.filter_decay = 0.2;
            p.decay = 0.25;
            p.sustain = 0.0;
            p.release = 0.2;
            p.gain = 0.3;
        }),
    );
    add(
        "Synthwave",
        "Synced Arp (16ths)",
        synth(|p| {
            p.wave = Wave::Pulse;
            p.duty = 0.35;
            p.filter_on = true;
            p.cutoff = 2200.0;
            p.arp = vec![0, 12, 7, 12];
            p.arp_speed = 4.0;
            p.arp_sync = true;
            p.sustain = 0.7;
            p.release = 0.1;
            p.gain = 0.2;
        }),
    );
    add(
        "Synthwave",
        "PWM Strings",
        synth(|p| {
            p.wave = Wave::Pulse;
            p.duty = 0.5;
            p.unison = 2;
            p.detune = 7.0;
            p.lfo_rate = 3.0;
            p.lfo_pwm = 0.3;
            p.filter_on = true;
            p.cutoff = 3500.0;
            p.attack = 0.3;
            p.sustain = 0.9;
            p.release = 0.6;
            p.gain = 0.2;
        }),
    );
    add(
        "Synthwave",
        "Outrun Bass",
        synth(|p| {
            p.wave = Wave::Saw;
            p.octave = -1;
            p.filter_on = true;
            p.cutoff = 260.0;
            p.resonance = 0.25;
            p.filter_env = 2.8;
            p.filter_decay = 0.12;
            p.decay = 0.2;
            p.sustain = 0.6;
            p.release = 0.05;
            p.gain = 0.38;
        }),
    );
    add(
        "Synthwave",
        "Sub Bass",
        synth(|p| {
            p.wave = Wave::Sine;
            p.octave = -1;
            p.sustain = 1.0;
            p.release = 0.06;
            p.gain = 0.55;
        }),
    );
    add("Synthwave", "Electro Drum Machine", drums("electro", 0.6));

    // --- Keys ---
    add(
        "Keys",
        "FM Electric Piano",
        synth(|p| {
            p.wave = Wave::Sine;
            p.fm_amount = 1.3;
            p.fm_ratio = 1.0;
            p.fm_decay = 0.6;
            p.decay = 1.2;
            p.sustain = 0.15;
            p.release = 0.35;
            p.vel_sens = 0.8;
            p.gain = 0.35;
        }),
    );
    add(
        "Keys",
        "Bell / Glockenspiel",
        synth(|p| {
            p.wave = Wave::Sine;
            p.octave = 1;
            p.fm_amount = 2.0;
            p.fm_ratio = 3.5;
            p.fm_decay = 0.4;
            p.decay = 1.0;
            p.sustain = 0.0;
            p.release = 0.8;
            p.gain = 0.28;
        }),
    );
    add(
        "Keys",
        "Organ",
        synth(|p| {
            p.wave = Wave::Sine;
            p.fm_amount = 0.6;
            p.fm_ratio = 2.0;
            p.sustain = 1.0;
            p.release = 0.05;
            p.lfo_rate = 6.0;
            p.lfo_amp = 0.12;
            p.gain = 0.3;
        }),
    );
    add("Keys", "Sampler (load a sample)", Instrument::Sampler(SamplerParams::default()));
    v
}

pub fn templates() -> Vec<TemplateInfo> {
    let t = |id: &str, name: &str, d: &str| TemplateInfo { id: id.into(), name: name.into(), description: d.into() };
    vec![
        t("empty", "Empty project", "One lead and one drum track."),
        t("chip-route", "Chip route theme", "Game Boy style: two pulse channels, wave bass, noise drums."),
        t("tactics-battle", "Tactics battle", "GBA orchestral style: brass, strings, harp, bass, drums."),
        t("neon-drive", "Neon drive", "Synthwave: supersaw lead, synced arp, neon pad, outrun bass, pumping drums."),
    ]
}

// ---------- template building helpers ----------

const STEP: u64 = PPQ / 4;

pub fn note_number(name: &str) -> Option<u8> {
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

/// "G5:4 D5:2 r:2" — note names with lengths in 16th-note steps; "r" is a rest.
fn melody(s: &str, vel: f32) -> Vec<Note> {
    let mut out = vec![];
    let mut t = 0;
    for tok in s.split_whitespace() {
        let (name, len) = tok.split_once(':').unwrap_or((tok, "1"));
        let len: u64 = len.parse().unwrap_or(1);
        if let Some(pitch) = note_number(name) {
            out.push(Note { id: 0, pitch, start: t * STEP, length: len * STEP - 2, velocity: vel });
        }
        t += len;
    }
    out
}

fn chord(name: &str) -> Vec<u8> {
    // Root, third, fifth in octave 4 for the chord symbols used by the templates.
    let (root, minor) = match name.strip_suffix('m') {
        Some(r) => (r, true),
        None => (name, false),
    };
    let r = note_number(&format!("{root}4")).unwrap_or(60);
    vec![r, r + if minor { 3 } else { 4 }, r + 7]
}

fn drum_steps(pitch: u8, steps: &[u64], bars: u64, vel: f32) -> Vec<Note> {
    let mut out = vec![];
    for b in 0..bars {
        for &s in steps {
            out.push(Note { id: 0, pitch, start: (b * 16 + s) * STEP, length: STEP, velocity: vel });
        }
    }
    out
}

struct B {
    p: Project,
}

impl B {
    fn new(name: &str, bpm: f64, key: &str, scale: &str) -> Self {
        let root = note_number(&format!("{key}4")).map(|n| n % 12).unwrap_or(0);
        B { p: Project { name: name.into(), bpm, key_root: root, scale: scale.into(), ..Project::default() } }
    }
    fn track(&mut self, name: &str, preset_name: &str, volume: f32) -> u64 {
        let instrument = preset(preset_name);
        let id = apply(&mut self.p, Edit::AddTrack { kind: "instrument".into(), name: Some(name.into()), instrument, index: None })
            .unwrap()
            .unwrap();
        self.p.track_mut(id).unwrap().volume = volume;
        id
    }
    fn pattern(&mut self, name: &str, bars: u64, notes: Vec<Note>) -> u64 {
        let len = self.p.bar_ticks() * bars;
        apply(&mut self.p, Edit::AddPattern { name: Some(name.into()), length: Some(len), notes: Some(notes), text: None })
            .unwrap()
            .unwrap()
    }
    fn clip(&mut self, track: u64, pattern: u64, start_bar: u64, bars: u64) {
        let bt = self.p.bar_ticks();
        apply(
            &mut self.p,
            Edit::AddClip {
                track_id: track,
                start: start_bar * bt,
                length: bars * bt,
                offset: None,
                content: ClipContent::Pattern { pattern_id: pattern },
            },
        )
        .unwrap();
    }
    fn fx(&mut self, track: Option<u64>, kind: EffectKind) {
        let id = self.p.alloc_id();
        let e = Effect { id, enabled: true, kind };
        match track {
            Some(t) => self.p.track_mut(t).unwrap().effects.push(e),
            None => self.p.master.effects.push(e),
        }
    }
    fn finish(mut self, loop_bars: u64) -> Project {
        self.p.loop_start = 0;
        self.p.loop_end = self.p.bar_ticks() * loop_bars;
        self.p.loop_enabled = true;
        self.p.master.volume = 0.7;
        self.p
    }
}

pub fn template(id: &str) -> Project {
    match id {
        "chip-route" => chip_route(),
        "tactics-battle" => tactics_battle(),
        "neon-drive" => neon_drive(),
        _ => empty(),
    }
}

fn empty() -> Project {
    let mut b = B::new("Untitled", 120.0, "C", "major");
    let lead = b.track("Lead", "Pulse Lead 12.5%", 0.8);
    let dr = b.track("Drums", "Chip Drum Kit", 0.8);
    let p1 = b.pattern("Pattern 1", 1, vec![]);
    b.clip(lead, p1, 0, 1);
    let _ = dr;
    let mut p = b.finish(4);
    p.loop_enabled = false;
    p
}

fn chip_route() -> Project {
    let mut b = B::new("Chip Route", 140.0, "G", "major");
    let lead = b.track("Pulse 1 (lead)", "Pulse Lead 12.5%", 0.85);
    let harm = b.track("Pulse 2 (harmony)", "Pulse Harmony", 0.8);
    let bass = b.track("Wave (bass)", "Wave Bass", 0.85);
    let drm = b.track("Noise (drums)", "Chip Drum Kit", 0.75);

    let mel = melody(
        "G5:4 D5:2 G5:2 A5:4 B5:4  C6:6 B5:2 A5:4 G5:4  E5:4 G5:2 E5:2 D5:4 B4:4  D5:12 r:4 \
         G5:4 D5:2 G5:2 A5:4 B5:4  C6:4 E6:4 D6:4 C6:2 B5:2  A5:4 F#5:2 G5:2 A5:4 D5:4  G5:12 r:4",
        0.85,
    );
    let chords = ["G", "C", "Em", "D", "G", "C", "D", "G"];
    let mut harmony = vec![];
    let mut bassline = vec![];
    for (bar, c) in chords.iter().enumerate() {
        let tones = chord(c);
        let order = [0usize, 1, 2, 1, 0, 1, 2, 1];
        for (k, &o) in order.iter().enumerate() {
            harmony.push(Note {
                id: 0,
                pitch: tones[o],
                start: (bar as u64 * 16 + k as u64 * 2) * STEP,
                length: 2 * STEP - 4,
                velocity: 0.7,
            });
        }
        let root = tones[0] - 24;
        for (k, off) in [0u8, 12, 7, 12, 0, 12, 7, 12].iter().enumerate() {
            bassline.push(Note {
                id: 0,
                pitch: root + off,
                start: (bar as u64 * 16 + k as u64 * 2) * STEP,
                length: 2 * STEP - 6,
                velocity: 0.8,
            });
        }
    }
    let mut dr = drum_steps(36, &[0, 8], 8, 0.9);
    dr.extend(drum_steps(38, &[4, 12], 8, 0.8));
    dr.extend(drum_steps(42, &[0, 2, 4, 6, 8, 10, 12, 14], 8, 0.5));

    let p_mel = b.pattern("Route melody", 8, mel);
    let p_har = b.pattern("Route harmony", 8, harmony);
    let p_bas = b.pattern("Route bass", 8, bassline);
    let p_dr = b.pattern("Route drums", 8, dr);
    b.clip(lead, p_mel, 0, 8);
    b.clip(harm, p_har, 0, 8);
    b.clip(bass, p_bas, 0, 8);
    b.clip(drm, p_dr, 0, 8);
    b.fx(Some(harm), EffectKind::Delay { time: 0.75, feedback: 0.25, mix: 0.2, ping_pong: true });
    b.fx(None, EffectKind::Reverb { size: 0.3, damping: 0.6, mix: 0.1, width: 1.0 });
    b.finish(8)
}

fn tactics_battle() -> Project {
    let mut b = B::new("Tactics Battle", 156.0, "A", "minor");
    let brass = b.track("Brass (melody)", "Brass Section", 0.85);
    let strings = b.track("Strings", "Strings Ensemble", 0.7);
    let harp = b.track("Harp arps", "Harp / Pizzicato", 0.55);
    let bass = b.track("Bass", "Outrun Bass", 0.75);
    let drm = b.track("Drums", "Orchestra Drums", 0.8);

    let mel = melody(
        "A4:6 B4:2 C5:4 E5:4  F5:6 E5:2 D5:4 C5:4  D5:6 C5:2 B4:4 G4:4  G#4:8 B4:4 E5:4 \
         A5:6 G5:2 E5:4 C5:4  F5:4 A5:4 C6:4 A5:4  D6:6 C6:2 A5:4 F5:4  E5:8 G#5:4 B5:4",
        0.85,
    );
    let chords: [&[u8]; 8] = [
        &[57, 60, 64],
        &[53, 57, 60],
        &[55, 59, 62],
        &[52, 56, 59],
        &[57, 60, 64],
        &[53, 57, 60],
        &[50, 53, 57],
        &[52, 56, 59],
    ];
    let mut pad = vec![];
    let mut arps = vec![];
    let mut bassline = vec![];
    for (bar, tones) in chords.iter().enumerate() {
        let bs = bar as u64 * 16;
        for &t in tones.iter() {
            pad.push(Note { id: 0, pitch: t, start: bs * STEP, length: 16 * STEP - 4, velocity: 0.7 });
        }
        let order = [0usize, 1, 2, 1];
        for k in 0..16u64 {
            let o = order[(k % 4) as usize];
            arps.push(Note {
                id: 0,
                pitch: tones[o] + 12 + if k % 8 >= 4 { 12 } else { 0 },
                start: (bs + k) * STEP,
                length: STEP,
                velocity: 0.6,
            });
        }
        let root = tones[0] - 24;
        for (k, off) in [0u8, 0, 12, 0, 0, 12, 0, 7].iter().enumerate() {
            bassline.push(Note { id: 0, pitch: root + off, start: (bs + k as u64 * 2) * STEP, length: 2 * STEP - 6, velocity: 0.85 });
        }
    }
    let mut dr = drum_steps(36, &[0, 7, 8], 8, 0.9);
    dr.extend(drum_steps(38, &[4, 12], 8, 0.85));
    dr.extend(drum_steps(42, &[0, 2, 4, 6, 8, 10, 12, 14], 8, 0.4));
    dr.extend(drum_steps(49, &[0], 1, 0.7));
    dr.extend(drum_steps(45, &[14], 8, 0.6));

    let p_mel = b.pattern("Battle melody", 8, mel);
    let p_pad = b.pattern("Battle strings", 8, pad);
    let p_arp = b.pattern("Battle harp", 8, arps);
    let p_bas = b.pattern("Battle bass", 8, bassline);
    let p_dr = b.pattern("Battle drums", 8, dr);
    b.clip(brass, p_mel, 0, 8);
    b.clip(strings, p_pad, 0, 8);
    b.clip(harp, p_arp, 0, 8);
    b.clip(bass, p_bas, 0, 8);
    b.clip(drm, p_dr, 0, 8);
    b.fx(None, EffectKind::Reverb { size: 0.55, damping: 0.5, mix: 0.18, width: 1.0 });
    b.fx(None, EffectKind::Compressor { threshold: -14.0, ratio: 3.0, attack: 8.0, release: 150.0, makeup: 2.0 });
    b.finish(8)
}

fn neon_drive() -> Project {
    let mut b = B::new("Neon Drive", 100.0, "A", "minor");
    let lead = b.track("Supersaw lead", "Supersaw Lead", 0.75);
    let arp = b.track("Neon arp", "Neon Arp Pluck", 0.7);
    let pad = b.track("Neon pad", "Neon Pad", 0.7);
    let bass = b.track("Outrun bass", "Outrun Bass", 0.8);
    let drm = b.track("Drum machine", "Electro Drum Machine", 0.85);

    let chords: [&[u8]; 4] = [&[57, 60, 64], &[53, 57, 60], &[48, 52, 55], &[55, 59, 62]];
    let mut pad_notes = vec![];
    let mut arp_notes = vec![];
    let mut bass_notes = vec![];
    for (bar, tones) in chords.iter().enumerate() {
        let bs = bar as u64 * 16;
        for &t in tones.iter() {
            pad_notes.push(Note { id: 0, pitch: t, start: bs * STEP, length: 16 * STEP - 4, velocity: 0.7 });
        }
        let seq = [tones[0], tones[1], tones[2], tones[0] + 12];
        for k in 0..16u64 {
            arp_notes.push(Note {
                id: 0,
                pitch: seq[(k % 4) as usize] + 12,
                start: (bs + k) * STEP,
                length: STEP - 2,
                velocity: if k % 4 == 0 { 0.85 } else { 0.65 },
            });
        }
        let root = tones[0] - 24;
        for k in 0..8u64 {
            bass_notes.push(Note { id: 0, pitch: root, start: (bs + k * 2) * STEP, length: 2 * STEP - 6, velocity: 0.85 });
        }
    }
    let mel = melody("E5:6 D5:2 C5:4 A4:4  C5:6 D5:2 E5:4 F5:4  G5:6 E5:2 C5:4 E5:4  D5:12 B4:4", 0.8);
    let mut dr = drum_steps(36, &[0, 4, 8, 12], 4, 0.95);
    dr.extend(drum_steps(38, &[4, 12], 4, 0.85));
    dr.extend(drum_steps(39, &[4, 12], 4, 0.6));
    dr.extend(drum_steps(42, &[2, 6, 10, 14], 4, 0.55));

    let p_pad = b.pattern("Neon chords", 4, pad_notes);
    let p_arp = b.pattern("Neon arp", 4, arp_notes);
    let p_bas = b.pattern("Neon bass", 4, bass_notes);
    let p_mel = b.pattern("Neon melody", 4, mel);
    let p_dr = b.pattern("Neon beat", 4, dr);
    b.clip(pad, p_pad, 0, 8);
    b.clip(arp, p_arp, 0, 8);
    b.clip(bass, p_bas, 0, 8);
    b.clip(drm, p_dr, 0, 8);
    b.clip(lead, p_mel, 4, 4);
    b.fx(Some(arp), EffectKind::Delay { time: 0.75, feedback: 0.4, mix: 0.3, ping_pong: true });
    b.fx(Some(pad), EffectKind::Pump { depth: 0.6, rate: 1.0, release: 0.6 });
    b.fx(Some(pad), EffectKind::Chorus { rate: 0.5, depth: 3.0, mix: 0.4 });
    b.fx(Some(bass), EffectKind::Pump { depth: 0.5, rate: 1.0, release: 0.5 });
    b.fx(Some(lead), EffectKind::Delay { time: 0.5, feedback: 0.3, mix: 0.2, ping_pong: true });
    b.fx(Some(drm), EffectKind::Reverb { size: 0.7, damping: 0.3, mix: 0.12, width: 1.0 });
    b.fx(None, EffectKind::Reverb { size: 0.75, damping: 0.4, mix: 0.15, width: 1.0 });
    b.fx(None, EffectKind::Compressor { threshold: -12.0, ratio: 2.5, attack: 10.0, release: 120.0, makeup: 2.0 });
    b.finish(8)
}
