use crate::error::ClientError;
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::{Application, Channels, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SAMPLES: usize = 960;

fn low_latency_buffer_size() -> cpal::BufferSize {
    cpal::BufferSize::Fixed(FRAME_SAMPLES as u32 * 2)
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

pub struct MicCapture {
    _stream: cpal::Stream,
    pub muted: Arc<AtomicBool>,
}

impl MicCapture {
    pub fn start(
        device_name: Option<&str>,
        muted: Arc<AtomicBool>,
        mut on_frame: impl FnMut(Vec<i16>) + Send + 'static,
    ) -> Result<Self, ClientError> {
        let host = audio_host();
        let device = find_device(&host, device_name, true)
            .ok_or_else(|| ClientError::GatewayClosed("voice audio: no input device available".into()))?;

        let config = StreamConfig { channels: CHANNELS, sample_rate: SAMPLE_RATE, buffer_size: low_latency_buffer_size() };

        let muted_cb = muted.clone();
        let mut pending: Vec<i16> = Vec::with_capacity(FRAME_SAMPLES);

        const GATE_THRESHOLD: i64 = 200;
        const GATE_HANG_FRAMES: u32 = 15;
        let mut gate_hang_remaining: u32 = 0;

        let err_fn = |e: cpal::Error| tracing::warn!("voice audio: input stream error: {e}");
        let stream = device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    if muted_cb.load(Ordering::Relaxed) {
                        return;
                    }
                    pending.extend_from_slice(data);
                    while pending.len() >= FRAME_SAMPLES {
                        let frame: Vec<i16> = pending.drain(..FRAME_SAMPLES).collect();
                        let sum_sq: i64 = frame.iter().map(|&s| (s as i64) * (s as i64)).sum();
                        let rms = ((sum_sq / frame.len() as i64) as f64).sqrt() as i64;
                        if rms >= GATE_THRESHOLD {
                            gate_hang_remaining = GATE_HANG_FRAMES;
                        } else if gate_hang_remaining > 0 {
                            gate_hang_remaining -= 1;
                        } else {
                            continue;
                        }
                        on_frame(frame);
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: build_input_stream failed: {e}")))?;

        stream.play().map_err(|e| ClientError::GatewayClosed(format!("voice audio: stream play failed: {e}")))?;

        Ok(Self { _stream: stream, muted })
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
}

pub struct SpeakerPlayback {
    _stream: cpal::Stream,
    tx: mpsc::UnboundedSender<(u32, Vec<i16>)>,
    pub deafened: Arc<AtomicBool>,
}

impl SpeakerPlayback {
    pub fn start(device_name: Option<&str>, deafened: Arc<AtomicBool>) -> Result<Self, ClientError> {
        let host = audio_host();
        let device = find_device(&host, device_name, false)
            .ok_or_else(|| ClientError::GatewayClosed("voice audio: no output device available".into()))?;

        let config = StreamConfig { channels: CHANNELS, sample_rate: SAMPLE_RATE, buffer_size: low_latency_buffer_size() };

        let (tx, mut rx) = mpsc::unbounded_channel::<(u32, Vec<i16>)>();
        let deafened_cb = deafened.clone();
        let mut pending: std::collections::HashMap<u32, std::collections::VecDeque<i16>> = std::collections::HashMap::new();
        let mut primed: std::collections::HashSet<u32> = std::collections::HashSet::new();
        const PREBUFFER_SAMPLES: usize = FRAME_SAMPLES * CHANNELS as usize * 2;
        const MAX_QUEUED_SAMPLES: usize = SAMPLE_RATE as usize * CHANNELS as usize / 5;

        let err_fn = |e: cpal::Error| tracing::warn!("voice audio: output stream error: {e}");
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [i16], _| {
                    if deafened_cb.load(Ordering::Relaxed) {
                        out.fill(0);
                        return;
                    }
                    while let Ok((ssrc, frame)) = rx.try_recv() {
                        let queue = pending.entry(ssrc).or_default();
                        queue.extend(frame);
                        if queue.len() > MAX_QUEUED_SAMPLES {
                            let drop = queue.len() - MAX_QUEUED_SAMPLES;
                            queue.drain(..drop);
                        }
                    }
                    out.fill(0);
                    for (ssrc, queue) in pending.iter_mut() {
                        if !primed.contains(ssrc) {
                            if queue.len() < PREBUFFER_SAMPLES {
                                continue;
                            }
                            primed.insert(*ssrc);
                        }
                        if queue.is_empty() {
                            primed.remove(ssrc);
                            continue;
                        }
                        for sample in out.iter_mut() {
                            let Some(mixed) = queue.pop_front() else { break };
                            *sample = sample.saturating_add(mixed);
                        }
                    }
                    pending.retain(|_, queue| !queue.is_empty());
                },
                err_fn,
                None,
            )
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: build_output_stream failed: {e}")))?;

        stream.play().map_err(|e| ClientError::GatewayClosed(format!("voice audio: stream play failed: {e}")))?;

        Ok(Self { _stream: stream, tx, deafened })
    }

    pub fn push_frame(&self, ssrc: u32, pcm: Vec<i16>) {
        let _ = self.tx.send((ssrc, pcm));
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Relaxed);
    }
}

pub struct AudioCodec {
    encoder: OpusEncoder,
    // Opus decoders carry per-stream state (packet loss concealment,
    // continuity); sharing one across multiple remote speakers corrupts
    // that state and causes audible glitches when people talk at once.
    decoders: std::collections::HashMap<u32, OpusDecoder>,
}

impl AudioCodec {
    pub fn new() -> Result<Self, ClientError> {
        let channels = if CHANNELS == 2 { Channels::Stereo } else { Channels::Mono };
        let encoder = OpusEncoder::new(SampleRate::Hz48000, channels, Application::Voip)
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: opus encoder init failed: {e}")))?;
        Ok(Self { encoder, decoders: std::collections::HashMap::new() })
    }

    pub fn encode(&self, pcm: &[i16]) -> Result<Vec<u8>, ClientError> {
        let mut out = [0u8; 4000];
        let len = self
            .encoder
            .encode(pcm, &mut out)
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: opus encode failed: {e}")))?;
        Ok(out[..len].to_vec())
    }

    pub fn decode(&mut self, ssrc: u32, opus: &[u8]) -> Result<Vec<i16>, ClientError> {
        let channels = if CHANNELS == 2 { Channels::Stereo } else { Channels::Mono };
        let decoder = match self.decoders.entry(ssrc) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let decoder = OpusDecoder::new(SampleRate::Hz48000, channels)
                    .map_err(|e| ClientError::GatewayClosed(format!("voice audio: opus decoder init failed: {e}")))?;
                entry.insert(decoder)
            }
        };
        let mut out = vec![0i16; FRAME_SAMPLES * CHANNELS as usize];
        let packet: audiopus::packet::Packet =
            opus.try_into().map_err(|e| ClientError::GatewayClosed(format!("voice audio: invalid opus packet: {e}")))?;
        let signals: audiopus::MutSignals<i16> =
            out.as_mut_slice().try_into().map_err(|e| ClientError::GatewayClosed(format!("voice audio: invalid output buffer: {e}")))?;
        let samples = decoder
            .decode(Some(packet), signals, false)
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: opus decode failed: {e}")))?;
        out.truncate(samples * CHANNELS as usize);
        Ok(out)
    }
}

pub fn list_input_devices() -> Vec<String> {
    list_devices(true)
}

pub fn list_output_devices() -> Vec<String> {
    list_devices(false)
}

fn list_devices(input: bool) -> Vec<String> {
    let host = audio_host();
    let Ok(devices) = host.devices() else { return Vec::new() };
    devices
        .filter(|d| if input { d.supports_input() } else { d.supports_output() })
        .filter_map(|d| d.description().ok().map(|desc| desc.to_string()))
        .collect()
}

fn find_device(host: &cpal::Host, name: Option<&str>, input: bool) -> Option<cpal::Device> {
    if let Some(name) = name {
        let Ok(devices) = host.devices() else { return None };
        let found = devices
            .filter(|d| if input { d.supports_input() } else { d.supports_output() })
            .find(|d| d.description().map(|desc| desc.to_string() == name).unwrap_or(false));
        if found.is_some() {
            return found;
        }
    }
    if input { host.default_input_device() } else { host.default_output_device() }
}
