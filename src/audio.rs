//! Microphone and system audio, denoised and mixed into 48 kHz stereo PCM in real time.
//!
//! The output is paced by the wall clock from the first video frame, so the encoder's
//! sample-count clock stays locked to the video: if a device delivers late, silence fills
//! the gap; if it runs fast, whole frames are dropped. No alignment pass after recording.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use nnnoiseless::DenoiseState;

use crate::clock::Clock;

pub const RATE: u32 = 48_000;
const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 samples = 10 ms
const SLACK: usize = RATE as usize / 10; // 100 ms before filling or dropping

pub fn input_names() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| it.filter_map(|d| d.description().ok().map(|x| x.name().to_string())).collect())
        .unwrap_or_default()
}

pub fn default_input_name() -> Option<String> {
    cpal::default_host().default_input_device()?.description().ok().map(|d| d.name().to_string())
}

fn find_input(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    host.input_devices().ok()?.find(|d| d.description().map(|x| x.name() == name).unwrap_or(false))
}

/// Peak level 0..1 as f32 bits, shared with the UI meter.
pub type Level = Arc<AtomicU32>;

fn store_peak(level: &Level, peak: f32) {
    let old = f32::from_bits(level.load(Ordering::Relaxed));
    level.store(peak.max(old * 0.85).to_bits(), Ordering::Relaxed);
}

/// Linear resampler, streaming, any channel count. Good enough for speech at 44.1/48 kHz.
struct Linear {
    step: f64,
    pos: f64,
    last: Vec<f32>,
}

impl Linear {
    fn new(from: u32, channels: usize) -> Self {
        Self { step: from as f64 / RATE as f64, pos: 0.0, last: vec![0.0; channels] }
    }

    fn push(&mut self, input: &[f32], ch: usize, out: &mut VecDeque<f32>) {
        let frames = input.len() / ch;
        if (self.step - 1.0).abs() < 1e-9 {
            out.extend(input.iter().copied());
            return;
        }
        while self.pos < frames as f64 {
            let i = self.pos.floor() as isize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let a = if i <= 0 { self.last[c] } else { input[(i as usize - 1) * ch + c] };
                let b = input[i.max(0) as usize * ch + c];
                out.push_back(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        if frames > 0 {
            self.last.copy_from_slice(&input[(frames - 1) * ch..frames * ch]);
        }
    }
}

/// Opens a capture stream that delivers resampled audio into `ring` (mono or stereo).
fn open(
    device: &cpal::Device,
    loopback: bool,
    out_channels: usize,
    ring: Arc<Mutex<VecDeque<f32>>>,
    gate: Arc<AtomicBool>,
    level: Option<Level>,
) -> Option<cpal::Stream> {
    let config = if loopback { device.default_output_config() } else { device.default_input_config() }.ok()?;
    let ch = config.channels() as usize;
    let mut rs = Linear::new(config.sample_rate(), out_channels);
    let mut tmp: Vec<f32> = Vec::new();
    let stream = device
        .build_input_stream::<f32, _, _>(
            config.into(),
            move |data: &[f32], _| {
                if let Some(level) = &level {
                    store_peak(level, data.iter().fold(0.0f32, |m, &x| m.max(x.abs())));
                }
                if !gate.load(Ordering::Relaxed) {
                    return;
                }
                tmp.clear();
                for f in data.chunks_exact(ch) {
                    if out_channels == 1 {
                        tmp.push(f.iter().sum::<f32>() / ch as f32);
                    } else {
                        tmp.push(f[0]);
                        tmp.push(if ch > 1 { f[1] } else { f[0] });
                    }
                }
                let mut ring = ring.lock().unwrap();
                rs.push(&tmp, out_channels, &mut ring);
            },
            |_| {},
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(stream)
}

/// Mic level meter for the idle UI; drop to close the device.
pub struct Meter {
    _stream: Option<cpal::Stream>,
}

impl Meter {
    pub fn open(name: &str, level: Level) -> Self {
        let stream = find_input(name).and_then(|d| {
            let ring = Arc::new(Mutex::new(VecDeque::new()));
            open(&d, false, 1, ring, Arc::new(AtomicBool::new(false)), Some(level))
        });
        Self { _stream: stream }
    }
}

pub struct Options {
    pub mic: Option<String>,
    pub gain: f32,
    pub denoise: bool,
    pub system: bool,
}

pub struct Pipe {
    streams: Vec<cpal::Stream>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Pipe {
    /// `clock` starts with the first video frame and stops while paused; `sink` receives PCM16 stereo.
    pub fn start(
        opts: Options,
        clock: Arc<Clock>,
        level: Level,
        mut sink: impl FnMut(&[u8]) + Send + 'static,
    ) -> Option<Self> {
        let gate = Arc::new(AtomicBool::new(false));
        let mic_ring = Arc::new(Mutex::new(VecDeque::<f32>::new()));
        let loop_ring = Arc::new(Mutex::new(VecDeque::<f32>::new()));
        let mut streams = Vec::new();

        let mic = opts.mic.as_deref().and_then(find_input);
        let has_mic = mic.is_some();
        if let Some(d) = &mic
            && let Some(s) = open(d, false, 1, mic_ring.clone(), gate.clone(), Some(level))
        {
            streams.push(s);
        }
        let mut has_loop = false;
        if opts.system
            && let Some(d) = cpal::default_host().default_output_device()
            && let Some(s) = open(&d, true, 2, loop_ring.clone(), gate.clone(), None)
        {
            streams.push(s);
            has_loop = true;
        }
        if !has_mic && !has_loop {
            return None;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn({
            let stop = stop.clone();
            move || {
                let mut denoiser = opts.denoise.then(DenoiseState::new);
                let mut voice = [0f32; FRAME];
                let mut clean = [0f32; FRAME];
                let mut pcm = vec![0u8; FRAME * 4];
                while clock.active().is_none() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                gate.store(true, Ordering::Relaxed);
                let mut produced: u64 = 0;

                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                    if clock.paused() {
                        // What is said while paused is dropped, not delayed.
                        mic_ring.lock().unwrap().clear();
                        loop_ring.lock().unwrap().clear();
                        continue;
                    }
                    let target = (clock.active().unwrap_or_default().as_secs_f64() * RATE as f64) as u64;
                    while produced + FRAME as u64 <= target {
                        let behind = (target - produced) as usize;
                        // Voice: take a frame, wait for a late device, or fill silence once it is too late.
                        {
                            let mut ring = mic_ring.lock().unwrap();
                            while ring.len() > SLACK * 2 {
                                ring.drain(..FRAME);
                            }
                            if ring.len() >= FRAME {
                                for v in voice.iter_mut() {
                                    *v = ring.pop_front().unwrap();
                                }
                            } else if !has_mic || behind > SLACK {
                                voice.fill(0.0);
                            } else {
                                break;
                            }
                        }
                        if let Some(d) = denoiser.as_mut() {
                            for v in voice.iter_mut() {
                                *v *= 32768.0;
                            }
                            d.process_frame(&mut clean, &voice);
                            for (v, c) in voice.iter_mut().zip(clean.iter()) {
                                *v = c / 32768.0;
                            }
                        }
                        let mut ring = loop_ring.lock().unwrap();
                        while ring.len() > SLACK * 4 {
                            ring.drain(..FRAME * 2);
                        }
                        let take = ring.len().min(FRAME * 2) / 2;
                        for i in 0..FRAME {
                            let (l, r) = if i < take {
                                (ring.pop_front().unwrap(), ring.pop_front().unwrap())
                            } else {
                                (0.0, 0.0)
                            };
                            let v = voice[i] * opts.gain;
                            let l = ((v + l).clamp(-1.0, 1.0) * 32767.0) as i16;
                            let r = ((v + r).clamp(-1.0, 1.0) * 32767.0) as i16;
                            pcm[i * 4..i * 4 + 2].copy_from_slice(&l.to_le_bytes());
                            pcm[i * 4 + 2..i * 4 + 4].copy_from_slice(&r.to_le_bytes());
                        }
                        drop(ring);
                        sink(&pcm);
                        produced += FRAME as u64;
                    }
                }
            }
        });
        Some(Self { streams, stop, worker: Some(worker) })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        self.streams.clear();
    }
}
