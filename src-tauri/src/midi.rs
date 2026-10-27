//! MIDI keyboard input.

use midir::{Ignore, MidiInput, MidiInputConnection};

pub fn ports() -> Vec<String> {
    match MidiInput::new("music-maker-list") {
        Ok(m) => m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect(),
        Err(_) => vec![],
    }
}

pub enum MidiEvent {
    NoteOn(u8, f32),
    NoteOff(u8),
}

pub fn parse(msg: &[u8]) -> Option<MidiEvent> {
    if msg.len() < 3 {
        return None;
    }
    match msg[0] & 0xf0 {
        0x90 if msg[2] > 0 => Some(MidiEvent::NoteOn(msg[1], msg[2] as f32 / 127.0)),
        0x90 | 0x80 => Some(MidiEvent::NoteOff(msg[1])),
        _ => None,
    }
}

pub fn connect(name: &str, mut on_event: impl FnMut(MidiEvent) + Send + 'static) -> Result<MidiInputConnection<()>, String> {
    let mut input = MidiInput::new("music-maker").map_err(|e| e.to_string())?;
    input.ignore(Ignore::All);
    let port = input
        .ports()
        .into_iter()
        .find(|p| input.port_name(p).ok().as_deref() == Some(name))
        .ok_or_else(|| format!("MIDI device '{name}' not found"))?;
    input
        .connect(
            &port,
            "music-maker-in",
            move |_, msg, _| {
                if let Some(e) = parse(msg) {
                    on_event(e);
                }
            },
            (),
        )
        .map_err(|e| format!("Can't open MIDI device: {e}"))
}
