use crate::voice_audio::{audio_host, find_device, FRAME_SAMPLES, SAMPLE_RATE};
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::{Application, Bitrate, Channels, SampleRate};
use base64::Engine;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::StreamConfig;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const FLAG_IS_VOICE_MESSAGE: u64 = 1 << 13;
pub const FILENAME: &str = "voice-message.ogg";
pub const CONTENT_TYPE: &str = "audio/ogg";
pub const MAX_RECORDING_SECS: f64 = 20.0 * 60.0;
const WAVEFORM_MAX_POINTS: usize = 256;
const WAVEFORM_POINTS_PER_SEC: f64 = 10.0;
const PRE_SKIP: u16 = 312;
const PACKETS_PER_PAGE: usize = 50;
const MAX_DECODED_FRAME: usize = 5760;
const BITRATE: i32 = 64_000;

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in data {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04c1_1db7 } else { crc << 1 };
        }
    }
    crc
}

fn write_page(out: &mut Vec<u8>, header_type: u8, granule: i64, serial: u32, sequence: u32, packets: &[&[u8]]) {
    let mut lacing = Vec::new();
    for packet in packets {
        lacing.extend(std::iter::repeat_n(255u8, packet.len() / 255));
        lacing.push((packet.len() % 255) as u8);
    }
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.push(0);
    out.push(header_type);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(lacing.len() as u8);
    out.extend_from_slice(&lacing);
    for packet in packets {
        out.extend_from_slice(packet);
    }
    let crc = crc32(&out[start..]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

fn opus_head(channels: u8) -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

fn opus_tags() -> Vec<u8> {
    let vendor = b"rustcord";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

pub fn encode_ogg(pcm: &[i16]) -> Result<Vec<u8>, String> {
    let mut encoder = OpusEncoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip).map_err(|e| format!("opus encoder: {e}"))?;
    encoder.set_bitrate(Bitrate::BitsPerSecond(BITRATE)).map_err(|e| format!("opus bitrate: {e}"))?;
    let padded: Vec<i16> = pcm.iter().copied().chain(std::iter::repeat_n(0, PRE_SKIP as usize)).collect();
    let mut packets = Vec::with_capacity(padded.len() / FRAME_SAMPLES + 1);
    let mut buffer = [0u8; 4000];
    for chunk in padded.chunks(FRAME_SAMPLES) {
        let mut frame = [0i16; FRAME_SAMPLES];
        frame[..chunk.len()].copy_from_slice(chunk);
        let len = encoder.encode(&frame, &mut buffer).map_err(|e| format!("opus encode: {e}"))?;
        packets.push(buffer[..len].to_vec());
    }
    let serial = 0x7275_7374;
    let mut out = Vec::new();
    write_page(&mut out, 0x02, 0, serial, 0, &[&opus_head(1)]);
    write_page(&mut out, 0x00, 0, serial, 1, &[&opus_tags()]);
    let pages: Vec<&[Vec<u8>]> = packets.chunks(PACKETS_PER_PAGE).collect();
    let mut encoded = 0usize;
    for (index, page) in pages.iter().enumerate() {
        encoded += page.len() * FRAME_SAMPLES;
        let last = index + 1 == pages.len();
        let granule = if last { PRE_SKIP as usize + pcm.len() } else { encoded } as i64;
        let refs: Vec<&[u8]> = page.iter().map(Vec::as_slice).collect();
        write_page(&mut out, if last { 0x04 } else { 0x00 }, granule, serial, index as u32 + 2, &refs);
    }
    Ok(out)
}

struct Demuxed {
    packets: Vec<Vec<u8>>,
    final_granule: Option<i64>,
}

fn demux(bytes: &[u8]) -> Result<Demuxed, String> {
    let mut packets = Vec::new();
    let mut partial = Vec::new();
    let mut final_granule = None;
    let mut offset = 0;
    while offset + 27 <= bytes.len() {
        if &bytes[offset..offset + 4] != b"OggS" {
            return Err("not an Ogg file".into());
        }
        let granule = i64::from_le_bytes(bytes[offset + 6..offset + 14].try_into().unwrap());
        let segments = bytes[offset + 26] as usize;
        let lacing_end = offset + 27 + segments;
        let lacing = bytes.get(offset + 27..lacing_end).ok_or("truncated Ogg page")?;
        let mut data = lacing_end;
        for &len in lacing {
            let end = data + len as usize;
            partial.extend_from_slice(bytes.get(data..end).ok_or("truncated Ogg page")?);
            data = end;
            if len < 255 {
                packets.push(std::mem::take(&mut partial));
            }
        }
        if granule >= 0 {
            final_granule = Some(granule);
        }
        offset = data;
    }
    Ok(Demuxed { packets, final_granule })
}

pub fn decode_ogg(bytes: &[u8]) -> Result<Vec<i16>, String> {
    let demuxed = demux(bytes)?;
    let mut packets = demuxed.packets.into_iter();
    let head = packets.next().ok_or("empty Ogg file")?;
    if !head.starts_with(b"OpusHead") || head.len() < 19 {
        return Err("not an Ogg Opus file".into());
    }
    let channels = match head[9] {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        n => return Err(format!("{n}-channel Opus is not supported")),
    };
    let channel_count = head[9] as usize;
    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
    let mut decoder = OpusDecoder::new(SampleRate::Hz48000, channels).map_err(|e| format!("opus decoder: {e}"))?;
    let mut pcm = Vec::new();
    let mut frame = vec![0i16; MAX_DECODED_FRAME * channel_count];
    for packet in packets.filter(|p| !p.starts_with(b"OpusTags") && !p.is_empty()) {
        let input: audiopus::packet::Packet = packet.as_slice().try_into().map_err(|e| format!("bad opus packet: {e}"))?;
        let output: audiopus::MutSignals<i16> = frame.as_mut_slice().try_into().map_err(|e| format!("opus buffer: {e}"))?;
        let samples = match decoder.decode(Some(input), output, false) {
            Ok(samples) => samples,
            Err(e) => {
                tracing::debug!("voice message: skipping undecodable packet: {e}");
                continue;
            }
        };
        if channel_count == 1 {
            pcm.extend_from_slice(&frame[..samples]);
        } else {
            pcm.extend(frame[..samples * 2].chunks_exact(2).map(|pair| ((pair[0] as i32 + pair[1] as i32) / 2) as i16));
        }
    }
    let mut pcm = pcm.split_off(pre_skip.min(pcm.len()));
    if let Some(granule) = demuxed.final_granule {
        let length = (granule as usize).saturating_sub(pre_skip);
        if length > 0 && length < pcm.len() {
            pcm.truncate(length);
        }
    }
    Ok(pcm)
}

pub fn duration_secs(pcm: &[i16]) -> f64 {
    pcm.len() as f64 / SAMPLE_RATE as f64
}

pub fn waveform(pcm: &[i16]) -> Vec<u8> {
    if pcm.is_empty() {
        return Vec::new();
    }
    let points = ((duration_secs(pcm) * WAVEFORM_POINTS_PER_SEC).ceil() as usize).clamp(1, WAVEFORM_MAX_POINTS);
    let bucket = pcm.len().div_ceil(points);
    let levels: Vec<f64> = pcm
        .chunks(bucket)
        .map(|chunk| (chunk.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt())
        .collect();
    let loudest = levels.iter().copied().fold(1.0, f64::max);
    levels.iter().map(|level| (level / loudest * 255.0).round() as u8).collect()
}

pub fn waveform_base64(pcm: &[i16]) -> String {
    base64::engine::general_purpose::STANDARD.encode(waveform(pcm))
}

pub fn decode_waveform(encoded: &str) -> Vec<f32> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map(|bytes| bytes.into_iter().map(|b| b as f32 / 255.0).collect())
        .unwrap_or_default()
}

pub fn resample_waveform(levels: &[f32], bars: usize) -> Vec<f32> {
    if levels.is_empty() || bars == 0 {
        return vec![0.0; bars];
    }
    (0..bars)
        .map(|bar| {
            let start = bar * levels.len() / bars;
            let end = ((bar + 1) * levels.len() / bars).max(start + 1).min(levels.len());
            levels[start..end].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

pub struct ClipPlayer {
    _stream: cpal::Stream,
    position: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    length: usize,
}

impl ClipPlayer {
    pub fn start(samples: Arc<Vec<i16>>, start_at: usize, device_name: Option<&str>) -> Result<Self, String> {
        let host = audio_host();
        let device = find_device(&host, device_name, false).ok_or("no output device available")?;
        let config = StreamConfig { channels: 1, sample_rate: SAMPLE_RATE, buffer_size: cpal::BufferSize::Default };
        let length = samples.len();
        let position = Arc::new(AtomicUsize::new(start_at.min(length)));
        let paused = Arc::new(AtomicBool::new(false));
        let (position_cb, paused_cb) = (position.clone(), paused.clone());
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [i16], _| {
                    if paused_cb.load(Ordering::Relaxed) {
                        out.fill(0);
                        return;
                    }
                    let start = position_cb.load(Ordering::Relaxed).min(samples.len());
                    let end = (start + out.len()).min(samples.len());
                    out[..end - start].copy_from_slice(&samples[start..end]);
                    out[end - start..].fill(0);
                    position_cb.store(end, Ordering::Relaxed);
                },
                |e: cpal::Error| tracing::warn!("voice message playback error: {e}"),
                None,
            )
            .map_err(|e| format!("could not open the output device: {e}"))?;
        stream.play().map_err(|e| format!("could not start playback: {e}"))?;
        Ok(Self { _stream: stream, position, paused, length })
    }

    pub fn position(&self) -> usize {
        self.position.load(Ordering::Relaxed)
    }

    pub fn progress(&self) -> f32 {
        if self.length == 0 {
            return 1.0;
        }
        self.position() as f32 / self.length as f32
    }

    pub fn finished(&self) -> bool {
        self.position() >= self.length
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn seek(&self, fraction: f32) {
        let target = (fraction.clamp(0.0, 1.0) as f64 * self.length as f64) as usize;
        self.position.store(target.min(self.length), Ordering::Relaxed);
    }
}

pub struct ClipRecorder {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<i16>>>,
    level_bits: Arc<AtomicU32>,
}

impl ClipRecorder {
    pub fn start(device_name: Option<&str>) -> Result<Self, String> {
        let host = audio_host();
        let device = find_device(&host, device_name, true).ok_or("no microphone available")?;
        let config = StreamConfig { channels: 1, sample_rate: SAMPLE_RATE, buffer_size: cpal::BufferSize::Default };
        let samples = Arc::new(Mutex::new(Vec::with_capacity(SAMPLE_RATE as usize * 30)));
        let level_bits = Arc::new(AtomicU32::new(0));
        let (samples_cb, level_cb) = (samples.clone(), level_bits.clone());
        let limit = (MAX_RECORDING_SECS * SAMPLE_RATE as f64) as usize;
        let stream = device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    if data.is_empty() {
                        return;
                    }
                    let peak = data.iter().map(|&s| (s as i32).unsigned_abs()).max().unwrap_or(0) as f32 / i16::MAX as f32;
                    level_cb.store(peak.min(1.0).to_bits(), Ordering::Relaxed);
                    let mut samples = samples_cb.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let room = limit.saturating_sub(samples.len());
                    samples.extend_from_slice(&data[..data.len().min(room)]);
                },
                |e: cpal::Error| tracing::warn!("voice message recording error: {e}"),
                None,
            )
            .map_err(|e| format!("could not open the microphone: {e}"))?;
        stream.play().map_err(|e| format!("could not start recording: {e}"))?;
        Ok(Self { _stream: stream, samples, level_bits })
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Relaxed))
    }

    pub fn duration_secs(&self) -> f64 {
        self.samples.lock().map(|s| duration_secs(&s)).unwrap_or(0.0)
    }

    pub fn finish(self) -> Vec<i16> {
        drop(self._stream);
        std::mem::take(&mut *self.samples.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f64) -> Vec<i16> {
        let len = (secs * SAMPLE_RATE as f64) as usize;
        (0..len).map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 8000.0) as i16).collect()
    }

    #[test]
    fn the_crc_matches_the_ogg_reference_value() {
        assert_eq!(crc32(b"123456789"), 0x89a1_897f);
    }

    #[test]
    fn a_recording_survives_an_ogg_round_trip_with_its_exact_length() {
        for secs in [2.0, 2.37] {
            let pcm = tone(secs);
            let decoded = decode_ogg(&encode_ogg(&pcm).unwrap()).unwrap();
            assert_eq!(decoded.len(), pcm.len(), "{secs}s");
        }
        let pcm = tone(2.0);
        let decoded = decode_ogg(&encode_ogg(&pcm).unwrap()).unwrap();
        let energy = decoded.iter().map(|&s| (s as f64).abs()).sum::<f64>() / decoded.len() as f64;
        assert!(energy > 1000.0, "decoded audio is near silent: {energy}");
    }

    #[test]
    fn pages_carry_valid_checksums_and_end_with_eos() {
        let ogg = encode_ogg(&tone(3.0)).unwrap();
        let mut offset = 0;
        let mut last_type = 0;
        while offset < ogg.len() {
            let segments = ogg[offset + 26] as usize;
            let body: usize = ogg[offset + 27..offset + 27 + segments].iter().map(|&l| l as usize).sum();
            let end = offset + 27 + segments + body;
            let mut page = ogg[offset..end].to_vec();
            let stored = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].fill(0);
            assert_eq!(crc32(&page), stored);
            last_type = ogg[offset + 5];
            offset = end;
        }
        assert_eq!(last_type, 0x04);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(decode_ogg(b"RIFF....WAVE").is_err());
        assert!(decode_ogg(&[]).is_err());
    }

    #[test]
    fn the_waveform_has_ten_points_a_second_up_to_256_and_peaks_at_255() {
        assert_eq!(waveform(&tone(1.5)).len(), 15);
        assert_eq!(waveform(&tone(60.0)).len(), 256);
        let mut pcm = vec![0i16; SAMPLE_RATE as usize];
        pcm.extend(tone(1.0));
        let wave = waveform(&pcm);
        assert_eq!(wave[0], 0);
        assert_eq!(*wave.iter().max().unwrap(), 255);
        assert!(waveform(&[]).is_empty());
    }

    #[test]
    fn waveforms_round_trip_through_base64_and_resample_by_peak() {
        let levels = decode_waveform(&waveform_base64(&tone(1.0)));
        assert_eq!(levels.len(), 10);
        assert!(levels.iter().all(|&l| (0.0..=1.0).contains(&l)));
        assert_eq!(resample_waveform(&[0.1, 0.9, 0.2, 0.4], 2), [0.9, 0.4]);
        assert_eq!(resample_waveform(&[0.5], 3), [0.5, 0.5, 0.5]);
        assert_eq!(resample_waveform(&[], 2), [0.0, 0.0]);
        assert!(decode_waveform("not base64!").is_empty());
    }
}
