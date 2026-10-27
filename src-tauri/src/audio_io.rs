//! Audio device I/O with cpal. cpal streams aren't `Send`, so they live on a
//! dedicated "audio-host" thread that the rest of the app controls by message.

use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::{Engine, EngineMsg, Garbage, Shared};

/// The main thread's handle on a running engine.
pub struct AudioLink {
    pub tx: Producer<EngineMsg>,
    pub garbage: Consumer<Garbage>,
    pub shared: Arc<Shared>,
    pub sr: u32,
    /// Recorded input samples (stereo interleaved), filled while `shared.recording` is set.
    pub record: Option<Consumer<f32>>,
    pub input_rate: Option<u32>,
    pub output_name: String,
    pub input_name: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct OpenRequest {
    pub output: Option<String>,
    /// None / "none" = no input; "default" = system default; otherwise a device name.
    pub input: Option<String>,
    pub buffer: Option<u32>,
}

pub enum HostCmd {
    Open(OpenRequest, mpsc::Sender<Result<AudioLink, String>>),
}

pub fn spawn_host() -> mpsc::Sender<HostCmd> {
    let (tx, rx) = mpsc::channel::<HostCmd>();
    std::thread::Builder::new()
        .name("audio-host".into())
        .spawn(move || {
            let mut streams: Vec<Stream> = vec![];
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    HostCmd::Open(req, reply) => {
                        streams.clear();
                        match open(&req) {
                            Ok((s, link)) => {
                                streams = s;
                                let _ = reply.send(Ok(link));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                }
            }
        })
        .expect("spawn audio host thread");
    tx
}

pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let outs = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let ins = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    (outs, ins)
}

fn find_device(devs: Result<impl Iterator<Item = Device>, cpal::DevicesError>, name: &str) -> Option<Device> {
    devs.ok()?.find(|d| d.name().ok().as_deref() == Some(name))
}

fn open(req: &OpenRequest) -> Result<(Vec<Stream>, AudioLink), String> {
    let host = cpal::default_host();
    let out_dev = req
        .output
        .as_deref()
        .and_then(|n| find_device(host.output_devices(), n))
        .or_else(|| host.default_output_device())
        .ok_or("No audio output device found")?;
    let output_name = out_dev.name().unwrap_or_else(|_| "Output".into());
    let sup = out_dev.default_output_config().map_err(|e| format!("Output device config error: {e}"))?;
    let sr = sup.sample_rate().0;
    let fmt = sup.sample_format();
    let shared = Arc::new(Shared::new(sr));
    let mut streams = vec![];
    let mut warnings = vec![];

    // Input first, so its monitor feed can be handed to the engine.
    let mut monitor = None;
    let mut record = None;
    let mut input_rate = None;
    let mut input_name = None;
    if let Some(name) = req.input.as_deref().filter(|n| *n != "none") {
        match open_input(&host, name, shared.clone()) {
            Ok((stream, mon, rec, rate, n)) => {
                streams.push(stream);
                if rate == sr {
                    monitor = Some(mon);
                } else {
                    warnings.push(format!(
                        "Input runs at {rate} Hz but output at {sr} Hz: monitoring is off (recording still works)."
                    ));
                }
                record = Some(rec);
                input_rate = Some(rate);
                input_name = Some(n);
            }
            Err(e) => warnings.push(e),
        }
    }

    let mut cfg: StreamConfig = sup.config();
    let attempt = |cfg: &StreamConfig| -> Result<(Stream, Producer<EngineMsg>, Consumer<Garbage>), String> {
        let (tx, rx) = RingBuffer::<EngineMsg>::new(1024);
        let (gtx, grx) = RingBuffer::<Garbage>::new(256);
        let engine = Engine::new(sr as f64, shared.clone(), rx, gtx);
        let stream = match fmt {
            SampleFormat::F32 => build_output::<f32>(&out_dev, cfg, engine, shared.clone(), sr),
            SampleFormat::I16 => build_output::<i16>(&out_dev, cfg, engine, shared.clone(), sr),
            SampleFormat::U16 => build_output::<u16>(&out_dev, cfg, engine, shared.clone(), sr),
            SampleFormat::I32 => build_output::<i32>(&out_dev, cfg, engine, shared.clone(), sr),
            other => Err(format!("Unsupported output sample format {other:?}")),
        }?;
        Ok((stream, tx, grx))
    };
    let built = if let Some(b) = req.buffer {
        cfg.buffer_size = BufferSize::Fixed(b);
        match attempt(&cfg) {
            Ok(x) => Ok(x),
            Err(_) => {
                warnings.push(format!("Buffer size {b} isn't supported by this device; using the default."));
                cfg.buffer_size = BufferSize::Default;
                attempt(&cfg)
            }
        }
    } else {
        attempt(&cfg)
    };
    let (stream, mut tx, garbage) = built?;
    stream.play().map_err(|e| format!("Can't start audio output: {e}"))?;
    if let Some(m) = monitor {
        let _ = tx.push(EngineMsg::SetMonitorInput(Some(Box::new(m))));
    }
    streams.push(stream);
    Ok((
        streams,
        AudioLink { tx, garbage, shared, sr, record, input_rate, output_name, input_name, warnings },
    ))
}

fn build_output<T>(dev: &Device, cfg: &StreamConfig, mut engine: Engine, shared: Arc<Shared>, sr: u32) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = cfg.channels as usize;
    let mut scratch = vec![0f32; 16384];
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            let lat = ts
                .playback
                .duration_since(&ts.callback)
                .map(|d| (d.as_secs_f64() * sr as f64) as u32)
                .unwrap_or((data.len() / channels.max(1)) as u32);
            shared.latency_frames.store(lat, Ordering::Relaxed);
            if scratch.len() < data.len() {
                // Only if the device suddenly asks for a huge buffer.
                scratch.resize(data.len(), 0.0);
            }
            let buf = &mut scratch[..data.len()];
            engine.process(buf, channels);
            for (o, s) in data.iter_mut().zip(buf.iter()) {
                *o = T::from_sample(*s);
            }
        },
        |e| eprintln!("audio output error: {e}"),
        None,
    )
    .map_err(|e| format!("Can't open audio output: {e}"))
}

type InputParts = (Stream, Consumer<f32>, Consumer<f32>, u32, String);

fn open_input(host: &cpal::Host, name: &str, shared: Arc<Shared>) -> Result<InputParts, String> {
    let dev = if name == "default" {
        host.default_input_device()
    } else {
        find_device(host.input_devices(), name).or_else(|| host.default_input_device())
    }
    .ok_or("No audio input device found")?;
    let dname = dev.name().unwrap_or_else(|_| "Input".into());
    let sup = dev.default_input_config().map_err(|e| format!("Input device config error: {e}"))?;
    let rate = sup.sample_rate().0;
    let cfg: StreamConfig = sup.config();
    let (mon_tx, mon_rx) = RingBuffer::<f32>::new(rate as usize); // 0.5 s stereo
    let (rec_tx, rec_rx) = RingBuffer::<f32>::new(rate as usize * 2 * 4); // 4 s stereo
    let stream = match sup.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&dev, &cfg, mon_tx, rec_tx, shared, rate),
        SampleFormat::I16 => build_input::<i16>(&dev, &cfg, mon_tx, rec_tx, shared, rate),
        SampleFormat::U16 => build_input::<u16>(&dev, &cfg, mon_tx, rec_tx, shared, rate),
        SampleFormat::I32 => build_input::<i32>(&dev, &cfg, mon_tx, rec_tx, shared, rate),
        other => Err(format!("Unsupported input sample format {other:?}")),
    }?;
    stream.play().map_err(|e| format!("Can't start audio input: {e}"))?;
    Ok((stream, mon_rx, rec_rx, rate, dname))
}

fn build_input<T>(
    dev: &Device,
    cfg: &StreamConfig,
    mut mon: Producer<f32>,
    mut rec: Producer<f32>,
    shared: Arc<Shared>,
    rate: u32,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = (cfg.channels as usize).max(1);
    dev.build_input_stream(
        cfg,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let ts = info.timestamp();
            if let Some(d) = ts.callback.duration_since(&ts.capture) {
                shared.input_latency_frames.store((d.as_secs_f64() * rate as f64) as u32, Ordering::Relaxed);
            }
            let recording = shared.recording.load(Ordering::Acquire);
            let mut peak = 0.0f32;
            for frame in data.chunks(ch) {
                let l = f32::from_sample(frame[0]);
                let r = if ch > 1 { f32::from_sample(frame[1]) } else { l };
                peak = peak.max(l.abs()).max(r.abs());
                let _ = mon.push(l);
                let _ = mon.push(r);
                if recording {
                    let _ = rec.push(l);
                    let _ = rec.push(r);
                }
            }
            let prev = shared.input_meter();
            shared.set_input_meter((prev * 0.85).max(peak));
        },
        |e| eprintln!("audio input error: {e}"),
        None,
    )
    .map_err(|e| format!("Can't open audio input: {e}"))
}
