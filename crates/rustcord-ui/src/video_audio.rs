use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::video::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE};

const CALLBACK_FRAMES: u32 = 960;

const MAX_QUEUED_SECS: f64 = 4.0;
const MAX_QUEUED_SAMPLES: usize = (MAX_QUEUED_SECS * AUDIO_SAMPLE_RATE as f64) as usize * AUDIO_CHANNELS as usize;

struct Shared {
    queue: Mutex<VecDeque<f32>>,
    samples_consumed: AtomicU64,
    output_latency_us: AtomicU64,
    paused: AtomicBool,
    volume: Mutex<f32>,
    started_at_secs: Mutex<f64>,
}

pub struct VideoAudio {
    stream: cpal::Stream,
    shared: Arc<Shared>,
}

impl VideoAudio {
    pub fn start() -> Option<Self> {
        let host = audio_host();
        let device = host.default_output_device()?;
        let config = cpal::StreamConfig {
            channels: AUDIO_CHANNELS,
            sample_rate: AUDIO_SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Fixed(CALLBACK_FRAMES),
        };

        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            samples_consumed: AtomicU64::new(0),
            output_latency_us: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            volume: Mutex::new(1.0),
            started_at_secs: Mutex::new(0.0),
        });

        let cb_shared = shared.clone();
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let ts = info.timestamp();
                    let latency = ts.playback.duration_since(ts.callback);
                    cb_shared.output_latency_us.store(latency.as_micros() as u64, Ordering::Relaxed);
                    if cb_shared.paused.load(Ordering::Relaxed) {
                        out.fill(0.0);
                        return;
                    }
                    let gain = *cb_shared.volume.lock().expect("volume mutex");
                    let mut queue = cb_shared.queue.lock().expect("queue mutex");
                    let mut filled = 0usize;
                    for sample in out.iter_mut() {
                        match queue.pop_front() {
                            Some(value) => {
                                *sample = value * gain;
                                filled += 1;
                            }
                            None => *sample = 0.0,
                        }
                    }
                    cb_shared
                        .samples_consumed
                        .fetch_add((filled / AUDIO_CHANNELS as usize) as u64, Ordering::Relaxed);
                },
                |e| tracing::warn!("video audio stream error: {e}"),
                None,
            )
            .ok()?;

        stream.play().ok()?;
        Some(Self { stream, shared })
    }

    pub fn push(&self, samples: &[f32]) {
        let mut queue = self.shared.queue.lock().expect("queue mutex");
        queue.extend(samples.iter().copied());
        if queue.len() > MAX_QUEUED_SAMPLES {
            let excess = queue.len() - MAX_QUEUED_SAMPLES;
            queue.drain(..excess);
        }
    }

    pub fn buffered_secs(&self) -> f64 {
        let queued = self.shared.queue.lock().expect("queue mutex").len() / AUDIO_CHANNELS as usize;
        queued as f64 / f64::from(AUDIO_SAMPLE_RATE)
    }

    pub fn clock_secs(&self) -> f64 {
        let consumed = self.shared.samples_consumed.load(Ordering::Relaxed) as f64 / f64::from(AUDIO_SAMPLE_RATE);
        let latency = self.shared.output_latency_us.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let start = *self.shared.started_at_secs.lock().expect("start mutex");
        start + audible_position(consumed, latency)
    }

    pub fn reset_clock(&self, position_secs: f64) {
        self.shared.queue.lock().expect("queue mutex").clear();
        self.shared.samples_consumed.store(0, Ordering::Relaxed);
        *self.shared.started_at_secs.lock().expect("start mutex") = position_secs;
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
        let result = if paused { self.stream.pause() } else { self.stream.play() };
        if let Err(e) = result {
            tracing::warn!("video audio pause/resume failed: {e}");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        *self.shared.volume.lock().expect("volume mutex") = volume.clamp(0.0, 1.0);
    }
}

fn audio_host() -> cpal::Host {
    #[cfg(target_os = "linux")]
    {
        if let Ok(host) = cpal::host_from_id(cpal::HostId::PulseAudio) {
            return host;
        }
    }
    cpal::default_host()
}

fn audible_position(consumed_secs: f64, output_latency_secs: f64) -> f64 {
    (consumed_secs - output_latency_secs).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_lags_consumption_by_the_output_latency() {
        assert!((audible_position(2.0, 0.5) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn the_clock_does_not_run_backwards_at_startup() {
        assert_eq!(audible_position(0.1, 0.5), 0.0);
    }

    #[test]
    fn a_zero_latency_device_reports_consumption_directly() {
        assert!((audible_position(3.0, 0.0) - 3.0).abs() < f64::EPSILON);
    }
}
