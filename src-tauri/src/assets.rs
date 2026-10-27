//! Decoded audio data, shared with the engine through `Arc`s.
//!
//! Assets stay in memory for the rest of the session once loaded, so the audio
//! thread never ends up dropping the last reference (which would free memory
//! on the real-time thread).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Stereo audio at its native sample rate. The engine resamples on playback.
pub struct AudioData {
    pub sr: u32,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl AudioData {
    pub fn frames(&self) -> usize {
        self.l.len()
    }
    pub fn duration(&self) -> f64 {
        self.l.len() as f64 / self.sr.max(1) as f64
    }
}

#[derive(Default)]
pub struct AssetStore {
    pub audio: HashMap<u64, Arc<AudioData>>,
    peaks: HashMap<u64, Arc<Vec<u8>>>,
}

pub const PEAK_FRAMES: usize = 256;

impl AssetStore {
    /// Cheap copy (shares the audio data) for background work like exporting.
    pub fn snapshot(&self) -> AssetStore {
        AssetStore { audio: self.audio.clone(), peaks: HashMap::new() }
    }

    pub fn get(&self, id: u64) -> Option<&Arc<AudioData>> {
        self.audio.get(&id)
    }

    pub fn insert(&mut self, id: u64, data: Arc<AudioData>) {
        self.peaks.remove(&id);
        self.audio.insert(id, data);
    }

    /// Min/max pairs (f32 little-endian) per PEAK_FRAMES frames of the mono mix.
    pub fn peaks(&mut self, id: u64) -> Option<Arc<Vec<u8>>> {
        if let Some(p) = self.peaks.get(&id) {
            return Some(p.clone());
        }
        let data = self.audio.get(&id)?;
        let bins = data.frames().div_ceil(PEAK_FRAMES);
        let mut out = Vec::with_capacity(bins * 8);
        for b in 0..bins {
            let s = b * PEAK_FRAMES;
            let e = (s + PEAK_FRAMES).min(data.frames());
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            for i in s..e {
                let v = 0.5 * (data.l[i] + data.r[i]);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            out.extend_from_slice(&lo.to_le_bytes());
            out.extend_from_slice(&hi.to_le_bytes());
        }
        let p = Arc::new(out);
        self.peaks.insert(id, p.clone());
        Some(p)
    }
}

/// Decodes any format symphonia understands (WAV, MP3, FLAC, OGG, AAC/MP4...) to stereo f32.
pub fn decode_file(path: &Path) -> Result<AudioData, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Can't open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Unsupported audio file {}: {e}", path.display()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or("No audio track found in file")?;
    let track_id = track.id;
    let mut sr = track.codec_params.sample_rate.unwrap_or(44100);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Unsupported codec: {e}"))?;

    let mut l = Vec::new();
    let mut r = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("Decode error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                sr = spec.rate;
                let ch = spec.channels.count().max(1);
                let needed = decoded.capacity() as u64;
                if buf.as_ref().map(|b| b.capacity() < needed as usize).unwrap_or(true) {
                    buf = Some(SampleBuffer::<f32>::new(needed, spec));
                }
                let b = buf.as_mut().unwrap();
                b.copy_interleaved_ref(decoded);
                let s = b.samples();
                for frame in s.chunks_exact(ch) {
                    l.push(frame[0]);
                    r.push(if ch > 1 { frame[1] } else { frame[0] });
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(format!("Decode error: {e}")),
        }
    }
    if l.is_empty() {
        return Err("File contains no audio".into());
    }
    Ok(AudioData { sr, l, r })
}

pub fn write_wav(path: &Path, l: &[f32], r: &[f32], sr: u32, bits: u16) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sr,
        bits_per_sample: bits,
        sample_format: if bits == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("Can't write {}: {e}", path.display()))?;
    let err = |e: hound::Error| format!("WAV write error: {e}");
    for i in 0..l.len() {
        match bits {
            32 => {
                w.write_sample(l[i]).map_err(err)?;
                w.write_sample(r[i]).map_err(err)?;
            }
            24 => {
                w.write_sample((l[i].clamp(-1.0, 1.0) * 8_388_607.0) as i32).map_err(err)?;
                w.write_sample((r[i].clamp(-1.0, 1.0) * 8_388_607.0) as i32).map_err(err)?;
            }
            _ => {
                w.write_sample((l[i].clamp(-1.0, 1.0) * 32767.0) as i16).map_err(err)?;
                w.write_sample((r[i].clamp(-1.0, 1.0) * 32767.0) as i16).map_err(err)?;
            }
        }
    }
    w.finalize().map_err(err)
}
