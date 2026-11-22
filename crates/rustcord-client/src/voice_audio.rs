use crate::error::ClientError;
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::{Application, Channels, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SAMPLES: usize = 960;

fn low_latency_buffer_size() -> cpal::BufferSize {
    cpal::BufferSize::Fixed(FRAME_SAMPLES as u32 * 2)
}

pub(crate) fn audio_host() -> cpal::Host {
    #[cfg(target_os = "linux")]
    {
        if let Ok(host) = cpal::host_from_id(cpal::HostId::PulseAudio) {
            return host;
        }
    }
    cpal::default_host()
}

pub fn slider_gain(slider: f32) -> f32 {
    if slider <= 0.0 {
        0.0
    } else if slider <= 1.0 {
        slider.powf(2.8)
    } else {
        10f32.powf(6.0 * (slider.min(2.0) - 1.0) / 20.0)
    }
}

#[derive(Clone)]
pub struct MicProcessing {
    gain_bits: Arc<std::sync::atomic::AtomicU32>,
    pub denoise: Arc<AtomicBool>,
}

impl Default for MicProcessing {
    fn default() -> Self {
        Self { gain_bits: Arc::new(std::sync::atomic::AtomicU32::new(1.0f32.to_bits())), denoise: Arc::new(AtomicBool::new(false)) }
    }
}

impl MicProcessing {
    pub fn set_gain(&self, gain: f32) {
        self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }
}

fn process_mic_frame(frame: &mut [i16], processing: &MicProcessing, denoiser: &mut Option<Box<nnnoiseless::DenoiseState<'static>>>) {
    let gain = processing.gain();
    let denoise = processing.denoise.load(Ordering::Relaxed);
    if !denoise && gain == 1.0 {
        return;
    }
    const RNN_FRAME: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
    if denoise {
        let state = denoiser.get_or_insert_with(nnnoiseless::DenoiseState::new);
        for chunk in frame.chunks_exact_mut(RNN_FRAME) {
            let mut input = [0f32; RNN_FRAME];
            let mut output = [0f32; RNN_FRAME];
            for (dst, &s) in input.iter_mut().zip(chunk.iter()) {
                *dst = s as f32;
            }
            state.process_frame(&mut output, &input);
            for (dst, &s) in chunk.iter_mut().zip(output.iter()) {
                *dst = (s * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            }
        }
    } else {
        for s in frame.iter_mut() {
            *s = (*s as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }
}

const OPUS_SILENCE: [u8; 3] = [0xf8, 0xff, 0xfe];
const TRAILING_SILENCE_FRAMES: u32 = 5;
const RING_CAPACITY: usize = SAMPLE_RATE as usize / 5;
const PLAYOUT_QUEUE_FRAMES: usize = 3;
const QUIET_RMS: f64 = 103.6;
const MIXER_WAKE: std::time::Duration = std::time::Duration::from_millis(5);

pub(crate) fn raise_thread_priority(thread: &str, priority: i32) {
    if std::env::var_os("RUSTCORD_VOICE_NO_PRIORITY").is_some() {
        tracing::info!(thread, "voice: audio thread stays at normal priority (RUSTCORD_VOICE_NO_PRIORITY)");
        return;
    }
    #[cfg(target_os = "linux")]
    {
        let param = libc::sched_param { sched_priority: priority };
        let rc = unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
        if rc == 0 {
            tracing::info!(thread, priority, "voice: audio thread raised to real-time priority");
        } else {
            tracing::info!(thread, error = %std::io::Error::from_raw_os_error(rc), "voice: audio thread stays at normal priority");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = priority;
        tracing::debug!(thread, "voice: raising audio thread priority is not implemented on this platform");
    }
}

pub enum MicAudio {
    Voice(Vec<i16>),
    Silence,
}

impl MicAudio {
    pub fn opus(&self, codec: &AudioCodec) -> Result<Vec<u8>, ClientError> {
        match self {
            MicAudio::Voice(pcm) => codec.encode(pcm),
            MicAudio::Silence => Ok(OPUS_SILENCE.to_vec()),
        }
    }

    pub fn is_silence(&self) -> bool {
        matches!(self, MicAudio::Silence)
    }
}

pub struct MicFrame {
    pub audio: MicAudio,
    pub frames_since_last: Option<u32>,
}

struct VoiceGate {
    hang_remaining: u32,
    open: bool,
    trailing_silence: u32,
    skipped: u32,
    sent_any: bool,
}

impl VoiceGate {
    const THRESHOLD: f64 = 200.0;
    const HANG_FRAMES: u32 = 15;

    fn new() -> Self {
        Self { hang_remaining: 0, open: false, trailing_silence: 0, skipped: 0, sent_any: false }
    }

    fn passes(&mut self, frame: Option<&[i16]>) -> bool {
        let Some(frame) = frame else {
            self.hang_remaining = 0;
            return false;
        };
        if rms(frame) >= Self::THRESHOLD {
            self.hang_remaining = Self::HANG_FRAMES;
            true
        } else if self.hang_remaining > 0 {
            self.hang_remaining -= 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self, frame: Option<Vec<i16>>) -> Option<MicFrame> {
        let audio = if self.passes(frame.as_deref()) {
            self.open = true;
            self.trailing_silence = 0;
            MicAudio::Voice(frame.unwrap_or_default())
        } else {
            if self.open {
                self.open = false;
                self.trailing_silence = TRAILING_SILENCE_FRAMES;
            }
            if self.trailing_silence == 0 {
                self.skipped = self.skipped.saturating_add(1);
                return None;
            }
            self.trailing_silence -= 1;
            MicAudio::Silence
        };
        let frames_since_last = self.sent_any.then_some(self.skipped + 1);
        self.sent_any = true;
        self.skipped = 0;
        Some(MicFrame { audio, frames_since_last })
    }
}

fn rms(frame: &[i16]) -> f64 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: i64 = frame.iter().map(|&s| i64::from(s) * i64::from(s)).sum();
    (sum_sq as f64 / frame.len() as f64).sqrt()
}

pub struct MicCapture {
    _stream: cpal::Stream,
    pub muted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: std::thread::Thread,
}

impl MicCapture {
    pub fn start(
        device_name: Option<&str>,
        muted: Arc<AtomicBool>,
        processing: MicProcessing,
        mut on_frame: impl FnMut(MicFrame) + Send + 'static,
    ) -> Result<Self, ClientError> {
        let host = audio_host();
        let device = find_device(&host, device_name, true)
            .ok_or_else(|| ClientError::GatewayClosed("voice audio: no input device available".into()))?;

        let config = StreamConfig { channels: CHANNELS, sample_rate: SAMPLE_RATE, buffer_size: low_latency_buffer_size() };
        let (mut producer, mut consumer) = rtrb::RingBuffer::<i16>::new(RING_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));

        let worker = {
            let stop = stop.clone();
            let muted = muted.clone();
            std::thread::Builder::new()
                .name("rustcord-voice-mic".into())
                .spawn(move || {
                    raise_thread_priority("microphone", 9);
                    let mut gate = VoiceGate::new();
                    let mut denoiser: Option<Box<nnnoiseless::DenoiseState<'static>>> = None;
                    while !stop.load(Ordering::Relaxed) {
                        let Ok(chunk) = consumer.read_chunk(FRAME_SAMPLES) else {
                            std::thread::park_timeout(std::time::Duration::from_millis(20));
                            continue;
                        };
                        let (head, tail) = chunk.as_slices();
                        let mut frame = Vec::with_capacity(FRAME_SAMPLES);
                        frame.extend_from_slice(head);
                        frame.extend_from_slice(tail);
                        chunk.commit_all();
                        let frame = if muted.load(Ordering::Relaxed) {
                            None
                        } else {
                            process_mic_frame(&mut frame, &processing, &mut denoiser);
                            Some(frame)
                        };
                        if let Some(out) = gate.next(frame) {
                            on_frame(out);
                        }
                    }
                })
                .map_err(|e| ClientError::GatewayClosed(format!("voice audio: failed to start microphone thread: {e}")))?
                .thread()
                .clone()
        };

        let err_fn = |e: cpal::Error| tracing::warn!("voice audio: input stream error: {e}");
        let wake = worker.clone();
        let stream = device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let (written, _) = producer.push_partial_slice(data);
                    if written.len() < data.len() {
                        tracing::trace!("voice audio: microphone ring full, dropped {} samples", data.len() - written.len());
                    }
                    wake.unpark();
                },
                err_fn,
                None,
            )
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: build_input_stream failed: {e}")))?;

        if let Err(e) = stream.play() {
            stop.store(true, Ordering::Relaxed);
            worker.unpark();
            return Err(ClientError::GatewayClosed(format!("voice audio: stream play failed: {e}")));
        }

        Ok(Self { _stream: stream, muted, stop, worker })
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.unpark();
    }
}

enum PlayoutCommand {
    Packet { ssrc: u32, timestamp: u32, payload: Vec<u8>, gain: f32 },
    Forget(u32),
}

struct RemoteSpeaker {
    jitter: crate::voice_playout::JitterBuffer,
    decoder: OpusDecoder,
}

impl RemoteSpeaker {
    fn new() -> Option<Self> {
        let channels = if CHANNELS == 2 { Channels::Stereo } else { Channels::Mono };
        match OpusDecoder::new(SampleRate::Hz48000, channels) {
            Ok(decoder) => Some(Self { jitter: Default::default(), decoder }),
            Err(e) => {
                tracing::warn!("voice audio: opus decoder init failed: {e}");
                None
            }
        }
    }

    fn decode(&mut self, payload: Option<&[u8]>, fec: bool, out: &mut [i16]) -> bool {
        let packet = match payload.map(audiopus::packet::Packet::try_from).transpose() {
            Ok(packet) => packet,
            Err(e) => {
                tracing::debug!("voice audio: invalid opus packet: {e}");
                return false;
            }
        };
        let Ok(signals) = audiopus::MutSignals::try_from(&mut *out) else { return false };
        match self.decoder.decode(packet, signals, fec) {
            Ok(_) => true,
            Err(e) => {
                tracing::debug!("voice audio: opus decode failed: {e}");
                false
            }
        }
    }

    fn mix_into(&mut self, mix: &mut [i32], scratch: &mut [i16], now: std::time::Instant) {
        use crate::voice_playout::{Playout, Skip};
        let mut playout = self.jitter.tick(now);
        let gain = loop {
            match playout {
                Playout::Idle => return,
                Playout::Decode { payload, gain } => {
                    if !self.decode(Some(&payload), false, scratch) {
                        return;
                    }
                    let skip = self.jitter.skip();
                    if skip == Skip::Always || (skip == Skip::IfQuiet && rms(scratch) < QUIET_RMS) {
                        self.jitter.skipped();
                        let following = self.jitter.tick(now);
                        if following != Playout::Idle {
                            playout = following;
                            continue;
                        }
                    }
                    break gain;
                }
                Playout::Recover { next_payload, gain } => {
                    if !self.decode(Some(&next_payload), true, scratch) {
                        return;
                    }
                    break gain;
                }
                Playout::Conceal { gain } => {
                    if !self.decode(None, false, scratch) {
                        return;
                    }
                    break gain;
                }
            }
        };
        for (acc, &sample) in mix.iter_mut().zip(scratch.iter()) {
            *acc += if gain == 1.0 { i32::from(sample) } else { (f32::from(sample) * gain) as i32 };
        }
    }
}

fn run_mixer(
    commands: std::sync::mpsc::Receiver<PlayoutCommand>,
    mut producer: rtrb::Producer<i16>,
    deafened: Arc<AtomicBool>,
    device_request: Arc<std::sync::atomic::AtomicUsize>,
) {
    raise_thread_priority("speakers", 10);
    let mut speakers: std::collections::HashMap<u32, RemoteSpeaker> = std::collections::HashMap::new();
    let mut mix = vec![0i32; FRAME_SAMPLES];
    let mut scratch = vec![0i16; FRAME_SAMPLES];
    let mut frame = vec![0i16; FRAME_SAMPLES];
    loop {
        let first = match commands.recv_timeout(MIXER_WAKE) {
            Ok(command) => Some(command),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let deaf = deafened.load(Ordering::Relaxed);
        let now = std::time::Instant::now();
        for command in first.into_iter().chain(std::iter::from_fn(|| commands.try_recv().ok())) {
            match command {
                PlayoutCommand::Packet { ssrc, timestamp, payload, gain } => {
                    if deaf {
                        continue;
                    }
                    if !speakers.contains_key(&ssrc) {
                        let Some(speaker) = RemoteSpeaker::new() else { continue };
                        speakers.insert(ssrc, speaker);
                    }
                    if let Some(speaker) = speakers.get_mut(&ssrc) {
                        speaker.jitter.push(timestamp, payload, gain, now);
                    }
                }
                PlayoutCommand::Forget(ssrc) => {
                    speakers.remove(&ssrc);
                }
            }
        }
        if deaf {
            speakers.values_mut().for_each(|speaker| speaker.jitter.reset());
        }

        let target = (PLAYOUT_QUEUE_FRAMES * FRAME_SAMPLES).max(device_request.load(Ordering::Relaxed) + FRAME_SAMPLES).min(RING_CAPACITY);
        while producer.slots() >= FRAME_SAMPLES && RING_CAPACITY - producer.slots() < target {
            mix.fill(0);
            if !deaf {
                for speaker in speakers.values_mut() {
                    speaker.mix_into(&mut mix, &mut scratch, now);
                }
            }
            for (out, &sample) in frame.iter_mut().zip(mix.iter()) {
                *out = sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            }
            if producer.push_entire_slice(&frame).is_err() {
                break;
            }
        }
    }
}

pub struct SpeakerPlayback {
    _stream: cpal::Stream,
    commands: std::sync::mpsc::Sender<PlayoutCommand>,
    pub deafened: Arc<AtomicBool>,
}

impl SpeakerPlayback {
    pub fn start(device_name: Option<&str>, deafened: Arc<AtomicBool>) -> Result<Self, ClientError> {
        let host = audio_host();
        let device = find_device(&host, device_name, false)
            .ok_or_else(|| ClientError::GatewayClosed("voice audio: no output device available".into()))?;

        let config = StreamConfig { channels: CHANNELS, sample_rate: SAMPLE_RATE, buffer_size: low_latency_buffer_size() };
        let (producer, mut consumer) = rtrb::RingBuffer::<i16>::new(RING_CAPACITY);
        let (commands, command_rx) = std::sync::mpsc::channel();
        let device_request = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        {
            let deafened = deafened.clone();
            let device_request = device_request.clone();
            std::thread::Builder::new()
                .name("rustcord-voice-speakers".into())
                .spawn(move || run_mixer(command_rx, producer, deafened, device_request))
                .map_err(|e| ClientError::GatewayClosed(format!("voice audio: failed to start speaker thread: {e}")))?;
        }

        let err_fn = |e: cpal::Error| tracing::warn!("voice audio: output stream error: {e}");
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [i16], _| {
                    device_request.fetch_max(out.len(), Ordering::Relaxed);
                    let (filled, _) = consumer.pop_partial_slice(out);
                    let filled = filled.len();
                    out[filled..].fill(0);
                },
                err_fn,
                None,
            )
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: build_output_stream failed: {e}")))?;

        stream.play().map_err(|e| ClientError::GatewayClosed(format!("voice audio: stream play failed: {e}")))?;

        Ok(Self { _stream: stream, commands, deafened })
    }

    pub fn push_packet(&self, ssrc: u32, timestamp: u32, payload: Vec<u8>, gain: f32) {
        let _ = self.commands.send(PlayoutCommand::Packet { ssrc, timestamp, payload, gain });
    }

    pub fn forget(&self, ssrc: u32) {
        let _ = self.commands.send(PlayoutCommand::Forget(ssrc));
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioProfile {
    Voice,
    StreamSound,
}

const OPUS_BITRATE_BPS: i32 = 64_000;
const MIN_EXPECTED_LOSS_PCT: u8 = 5;
const MAX_EXPECTED_LOSS_PCT: u8 = 30;

pub struct AudioCodec {
    encoder: OpusEncoder,
    expected_loss_pct: u8,
    last_seen: std::collections::HashMap<u32, std::time::Instant>,
}

impl AudioCodec {
    pub fn new(profile: AudioProfile) -> Result<Self, ClientError> {
        let channels = if CHANNELS == 2 { Channels::Stereo } else { Channels::Mono };
        let application = match profile {
            AudioProfile::Voice => Application::Voip,
            AudioProfile::StreamSound => Application::Audio,
        };
        let opus_error = |e: audiopus::Error| ClientError::GatewayClosed(format!("voice audio: opus encoder setup failed: {e}"));
        let mut encoder = OpusEncoder::new(SampleRate::Hz48000, channels, application).map_err(opus_error)?;
        encoder.set_bitrate(audiopus::Bitrate::BitsPerSecond(OPUS_BITRATE_BPS)).map_err(opus_error)?;
        encoder.set_inband_fec(true).map_err(opus_error)?;
        encoder.set_packet_loss_perc(MIN_EXPECTED_LOSS_PCT).map_err(opus_error)?;
        Ok(Self { encoder, expected_loss_pct: MIN_EXPECTED_LOSS_PCT, last_seen: std::collections::HashMap::new() })
    }

    pub fn set_measured_loss(&mut self, loss_pct: Option<f32>) {
        let target = loss_pct
            .filter(|pct| pct.is_finite())
            .map(|pct| pct.ceil().clamp(f32::from(MIN_EXPECTED_LOSS_PCT), f32::from(MAX_EXPECTED_LOSS_PCT)) as u8)
            .unwrap_or(MIN_EXPECTED_LOSS_PCT);
        if target == self.expected_loss_pct {
            return;
        }
        match self.encoder.set_packet_loss_perc(target) {
            Ok(()) => self.expected_loss_pct = target,
            Err(e) => tracing::debug!("voice audio: failed to update opus expected loss: {e}"),
        }
    }

    pub fn expected_loss_pct(&self) -> u8 {
        self.expected_loss_pct
    }

    pub fn encode(&self, pcm: &[i16]) -> Result<Vec<u8>, ClientError> {
        let mut out = [0u8; 4000];
        let len = self
            .encoder
            .encode(pcm, &mut out)
            .map_err(|e| ClientError::GatewayClosed(format!("voice audio: opus encode failed: {e}")))?;
        Ok(out[..len].to_vec())
    }

    pub fn note_packet(&mut self, ssrc: u32) {
        self.last_seen.insert(ssrc, std::time::Instant::now());
    }

    pub fn track_ssrc(&mut self, ssrc: u32) {
        self.last_seen.entry(ssrc).or_insert_with(std::time::Instant::now);
    }

    pub fn drop_ssrc(&mut self, ssrc: u32) {
        self.last_seen.remove(&ssrc);
    }

    pub fn idle_ssrcs(&self, idle_for: std::time::Duration) -> Vec<u32> {
        let now = std::time::Instant::now();
        self.last_seen
            .iter()
            .filter(|(_, seen)| now.duration_since(**seen) > idle_for)
            .map(|(ssrc, _)| *ssrc)
            .collect()
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

pub(crate) fn find_device(host: &cpal::Host, name: Option<&str>, input: bool) -> Option<cpal::Device> {
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

pub fn run_mic_test(input_device: Option<String>, output_device: Option<String>, duration: std::time::Duration) {
    let host = audio_host();
    let Some(in_device) = find_device(&host, input_device.as_deref(), true) else { return };
    let Some(out_device) = find_device(&host, output_device.as_deref(), false) else { return };

    let config = StreamConfig { channels: CHANNELS, sample_rate: SAMPLE_RATE, buffer_size: low_latency_buffer_size() };
    let recorded: Arc<std::sync::Mutex<Vec<i16>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded_cb = recorded.clone();
    let Ok(input_stream) = in_device.build_input_stream(
        config.clone(),
        move |data: &[i16], _| {
            recorded_cb.lock().unwrap().extend_from_slice(data);
        },
        |e: cpal::Error| tracing::warn!("mic test: input stream error: {e}"),
        None,
    ) else {
        return;
    };
    if input_stream.play().is_err() {
        return;
    }
    std::thread::sleep(duration);
    drop(input_stream);

    let samples = recorded.lock().unwrap().clone();
    if samples.is_empty() {
        return;
    }
    let position = Arc::new(AtomicU32::new(0));
    let position_cb = position.clone();
    let samples_cb = samples.clone();
    let Ok(output_stream) = out_device.build_output_stream(
        config,
        move |out: &mut [i16], _| {
            let pos = position_cb.load(Ordering::Relaxed) as usize;
            for (i, sample) in out.iter_mut().enumerate() {
                *sample = samples_cb.get(pos + i).copied().unwrap_or(0);
            }
            position_cb.fetch_add(out.len() as u32, Ordering::Relaxed);
        },
        |e: cpal::Error| tracing::warn!("mic test: output stream error: {e}"),
        None,
    ) else {
        return;
    };
    if output_stream.play().is_ok() {
        let playback_duration = std::time::Duration::from_secs_f64(samples.len() as f64 / SAMPLE_RATE as f64);
        std::thread::sleep(playback_duration);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn volume_sliders_follow_a_perceptual_curve() {
        use super::slider_gain;
        assert_eq!(slider_gain(0.0), 0.0);
        assert_eq!(slider_gain(1.0), 1.0);
        let half_db = 20.0 * slider_gain(0.5).log10();
        assert!((-18.0..-16.0).contains(&half_db), "50% is {half_db} dB");
        assert!((slider_gain(0.4) - 0.077).abs() < 0.002);
        assert!((slider_gain(2.0) - 1.995).abs() < 0.01);
        assert!(slider_gain(0.3) < slider_gain(0.31) && slider_gain(1.5) < slider_gain(1.6));
    }

    use super::*;

    fn loud() -> Option<Vec<i16>> {
        Some(vec![4000; FRAME_SAMPLES])
    }

    fn quiet() -> Option<Vec<i16>> {
        Some(vec![0; FRAME_SAMPLES])
    }

    fn sent(gate: &mut VoiceGate, frame: Option<Vec<i16>>) -> Option<(bool, Option<u32>)> {
        gate.next(frame).map(|out| (out.audio.is_silence(), out.frames_since_last))
    }

    #[test]
    fn the_first_frame_has_no_clock_to_follow() {
        let mut gate = VoiceGate::new();
        for _ in 0..10 {
            assert!(sent(&mut gate, quiet()).is_none());
        }
        assert_eq!(sent(&mut gate, loud()), Some((false, None)));
        assert_eq!(sent(&mut gate, loud()), Some((false, Some(1))));
    }

    #[test]
    fn closing_the_gate_sends_five_silence_frames_then_counts_the_pause() {
        let mut gate = VoiceGate::new();
        sent(&mut gate, loud());
        for _ in 0..VoiceGate::HANG_FRAMES {
            assert_eq!(sent(&mut gate, quiet()), Some((false, Some(1))));
        }
        for _ in 0..TRAILING_SILENCE_FRAMES {
            assert_eq!(sent(&mut gate, quiet()), Some((true, Some(1))));
        }
        for _ in 0..40 {
            assert!(sent(&mut gate, quiet()).is_none());
        }
        assert_eq!(sent(&mut gate, loud()), Some((false, Some(41))));
    }

    #[test]
    fn muting_closes_the_gate_at_once() {
        let mut gate = VoiceGate::new();
        sent(&mut gate, loud());
        assert_eq!(sent(&mut gate, None), Some((true, Some(1))));
        for _ in 1..TRAILING_SILENCE_FRAMES {
            assert_eq!(sent(&mut gate, None), Some((true, Some(1))));
        }
        assert!(sent(&mut gate, None).is_none());
        assert!(sent(&mut gate, None).is_none());
        assert_eq!(sent(&mut gate, loud()), Some((false, Some(3))));
    }

    #[test]
    fn real_opus_plays_through_loss_and_concealment() {
        let codec = AudioCodec::new(AudioProfile::Voice).unwrap();
        let tone: Vec<i16> = (0..FRAME_SAMPLES).map(|i| ((i as f32 * 0.07).sin() * 8000.0) as i16).collect();
        let packets: Vec<Vec<u8>> = (0..6).map(|_| codec.encode(&tone).unwrap()).collect();
        let mut speaker = RemoteSpeaker::new().unwrap();
        let now = std::time::Instant::now();
        for (n, packet) in packets.iter().enumerate() {
            if n != 2 {
                speaker.jitter.push(n as u32 * 960, packet.clone(), 0.5, now);
            }
        }
        let mut scratch = vec![0i16; FRAME_SAMPLES];
        for tick in 0..9 {
            let mut mix = vec![0i32; FRAME_SAMPLES];
            speaker.mix_into(&mut mix, &mut scratch, now);
            let peak = mix.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak <= 6000, "tick {tick}: gain not applied, peak {peak}");
            if (1..6).contains(&tick) {
                assert!(peak > 500, "tick {tick}: expected audible audio, peak {peak}");
            }
        }
    }

    #[test]
    fn the_encoder_follows_measured_uplink_loss_within_bounds() {
        let mut codec = AudioCodec::new(AudioProfile::Voice).unwrap();
        assert_eq!(codec.expected_loss_pct(), MIN_EXPECTED_LOSS_PCT);
        codec.set_measured_loss(Some(12.2));
        assert_eq!(codec.expected_loss_pct(), 13);
        codec.set_measured_loss(Some(80.0));
        assert_eq!(codec.expected_loss_pct(), MAX_EXPECTED_LOSS_PCT);
        codec.set_measured_loss(Some(f32::NAN));
        assert_eq!(codec.expected_loss_pct(), MIN_EXPECTED_LOSS_PCT);
        codec.set_measured_loss(None);
        assert_eq!(codec.expected_loss_pct(), MIN_EXPECTED_LOSS_PCT);
    }

    #[test]
    fn voice_packets_carry_fec_that_recovers_a_lost_frame() {
        let mut codec = AudioCodec::new(AudioProfile::Voice).unwrap();
        codec.set_measured_loss(Some(20.0));
        let tone: Vec<i16> = (0..FRAME_SAMPLES).map(|i| ((i as f32 * 0.05).sin() * 6000.0) as i16).collect();
        let packets: Vec<Vec<u8>> = (0..10).map(|_| codec.encode(&tone).unwrap()).collect();
        let mut speaker = RemoteSpeaker::new().unwrap();
        let mut out = vec![0i16; FRAME_SAMPLES];
        for packet in &packets[..8] {
            assert!(speaker.decode(Some(packet), false, &mut out));
        }
        assert!(speaker.decode(Some(&packets[9]), true, &mut out));
        assert!(rms(&out) > 1000.0, "fec frame should carry the tone, rms {}", rms(&out));
    }
}
