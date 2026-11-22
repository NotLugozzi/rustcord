use cpal::traits::{DeviceTrait, StreamTrait};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::voice_audio::{audio_host, find_device, slider_gain, SAMPLE_RATE};

const RATE: f32 = SAMPLE_RATE as f32;
const TAU: f32 = std::f32::consts::TAU;
const PEAK: f32 = 0.3;
const IDLE_CLOSE: Duration = Duration::from_secs(4);
const REPEAT_GAP: Duration = Duration::from_millis(90);
const MAX_VOICES: usize = 8;
const RING_CYCLE: f32 = 2.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sound {
    Join,
    Leave,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    Message,
    Mention,
    StreamStart,
    StreamStop,
    ViewerJoin,
    ViewerLeave,
}

impl Sound {
    pub const ALL: [Self; 12] = [
        Self::Join,
        Self::Leave,
        Self::Mute,
        Self::Unmute,
        Self::Deafen,
        Self::Undeafen,
        Self::Message,
        Self::Mention,
        Self::StreamStart,
        Self::StreamStop,
        Self::ViewerJoin,
        Self::ViewerLeave,
    ];
}

const G4: f32 = 67.0;
const B4: f32 = 71.0;
const D5: f32 = 74.0;
const E5: f32 = 76.0;
const G5: f32 = 79.0;
const B5: f32 = 83.0;
const D6: f32 = 86.0;

type Note = (f32, f32, f32);

fn score(sound: Sound) -> (&'static [Note], f32, f32) {
    match sound {
        Sound::Join => (&[(0.0, D5, 0.8), (0.13, G5, 1.0)], 1.0, 0.0),
        Sound::Leave => (&[(0.0, G5, 0.9), (0.15, D5, 0.75)], 1.0, -1.5),
        Sound::Mute => (&[(0.0, B5, 0.8), (0.07, G5, 0.7)], 0.4, -5.0),
        Sound::Unmute => (&[(0.0, G5, 0.7), (0.07, B5, 0.8)], 0.4, -5.0),
        Sound::Deafen => (&[(0.0, D5, 0.8), (0.08, G4, 0.7)], 0.45, -4.0),
        Sound::Undeafen => (&[(0.0, G4, 0.7), (0.08, D5, 0.8)], 0.45, -4.0),
        Sound::Message => (&[(0.0, E5, 0.7), (0.0, B5, 0.5)], 0.7, -3.0),
        Sound::Mention => (&[(0.0, G5, 0.7), (0.08, B5, 0.8), (0.16, D6, 1.0)], 0.8, -0.5),
        Sound::StreamStart => (&[(0.0, G4, 0.7), (0.0, D5, 0.6), (0.12, G5, 0.85)], 0.85, -2.5),
        Sound::StreamStop => (&[(0.0, G5, 0.75), (0.12, G4, 0.65), (0.12, D5, 0.55)], 0.85, -3.5),
        Sound::ViewerJoin => (&[(0.0, B4, 0.6), (0.09, E5, 0.8)], 0.6, -6.0),
        Sound::ViewerLeave => (&[(0.0, E5, 0.7), (0.1, B4, 0.6)], 0.6, -7.0),
    }
}

const RING: &[Note] = &[(0.0, G5, 0.8), (0.14, B5, 0.85), (0.28, D6, 1.0), (0.62, B5, 0.7), (0.76, D6, 0.9)];

fn frequency(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

fn tine(hz: f32, time: f32, velocity: f32, fade: f32) -> f32 {
    let longer = (440.0 / hz).powf(0.3) * fade;
    let partial = |ratio: f32, amplitude: f32, seconds: f32| amplitude * (TAU * hz * ratio * time).sin() * (-time / (seconds * longer)).exp();
    let tone = partial(1.0, 1.0, 0.32) + partial(2.0, 0.22 * velocity, 0.12) + partial(3.02, 0.08 * velocity, 0.05) + partial(5.43, 0.03 * velocity, 0.02);
    let attack = if time < 0.004 { 0.5 - 0.5 * (std::f32::consts::PI * time / 0.004).cos() } else { 1.0 };
    tone * attack
}

fn strike(notes: &[Note], fade: f32, length: usize) -> Vec<f32> {
    let mut dry = vec![0.0f32; length];
    for &(start, midi, velocity) in notes {
        let hz = frequency(midi);
        let first = ((start * RATE) as usize).min(length);
        for (index, sample) in dry[first..].iter_mut().enumerate() {
            *sample += velocity * tine(hz, index as f32 / RATE, velocity, fade);
        }
    }
    soften(&mut dry);
    dry
}

fn soften(samples: &mut [f32]) {
    let smoothing = (-TAU * 3500.0 / RATE).exp();
    let mut previous = 0.0f32;
    for sample in samples.iter_mut() {
        previous = previous * smoothing + *sample * (1.0 - smoothing);
        *sample = previous;
    }
    let taps = [((0.031 * RATE) as usize, 0.14f32), ((0.057 * RATE) as usize, 0.09)];
    for index in (0..samples.len()).rev() {
        let echo: f32 = taps.iter().filter(|(delay, _)| index >= *delay).map(|(delay, gain)| samples[index - delay] * gain).sum();
        samples[index] += echo;
    }
}

fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

pub fn render(sound: Sound) -> Vec<f32> {
    let (notes, fade, loudness) = score(sound);
    let last = notes.iter().fold(0.0f32, |last, note| last.max(note.0));
    let mut samples = strike(notes, fade, ((last + 1.5) * RATE) as usize);
    let peak = peak_of(&samples).max(1e-6);
    let end = samples.iter().rposition(|sample| sample.abs() > peak * 0.01).map_or(0, |index| index + 1);
    samples.truncate(end);
    let gain = PEAK * 10f32.powf(loudness / 20.0) / peak;
    let tail = (end / 4).min((0.06 * RATE) as usize).max(1);
    for (index, sample) in samples.iter_mut().enumerate() {
        let left = end - index;
        let fade = if left <= tail { (left as f32 / tail as f32).powi(2) } else { 1.0 };
        *sample *= gain * fade;
    }
    samples
}

pub fn render_ring() -> Vec<f32> {
    let mut samples = strike(RING, 1.0, (RING_CYCLE * RATE) as usize);
    let peak = peak_of(&samples).max(1e-6);
    let length = samples.len();
    let tail = (0.3 * RATE) as usize;
    for (index, sample) in samples.iter_mut().enumerate() {
        let left = length - index;
        let fade = if left <= tail { left as f32 / tail as f32 } else { 1.0 };
        *sample *= PEAK * 0.9 / peak * fade;
    }
    samples
}

enum Command {
    Play(Sound),
    Ring(bool),
    Device(Option<String>),
    Volume(f32),
}

#[derive(Default)]
struct Mixer {
    voices: Vec<(Arc<[f32]>, usize)>,
    ring: Option<(Arc<[f32]>, usize)>,
    gain: f32,
}

impl Mixer {
    fn idle(&self) -> bool {
        self.voices.is_empty() && self.ring.is_none()
    }

    fn fill(&mut self, out: &mut [i16]) {
        for sample in out.iter_mut() {
            let mut sum = 0.0f32;
            for (samples, position) in &mut self.voices {
                if let Some(value) = samples.get(*position) {
                    sum += value;
                }
                *position += 1;
            }
            if let Some((samples, position)) = &mut self.ring {
                sum += samples[*position % samples.len()];
                *position += 1;
            }
            *sample = ((sum * self.gain).clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        }
        self.voices.retain(|(samples, position)| *position < samples.len());
    }
}

struct Output {
    _stream: cpal::Stream,
    broken: Arc<AtomicBool>,
}

fn open(device_name: Option<&str>, mixer: &Arc<Mutex<Mixer>>) -> Option<Output> {
    let host = audio_host();
    let device = find_device(&host, device_name, false)?;
    let config = cpal::StreamConfig { channels: 1, sample_rate: SAMPLE_RATE, buffer_size: cpal::BufferSize::Default };
    let broken = Arc::new(AtomicBool::new(false));
    let callback_mixer = mixer.clone();
    let error_flag = broken.clone();
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [i16], _| match callback_mixer.lock() {
                Ok(mut mixer) => mixer.fill(out),
                Err(_) => out.fill(0),
            },
            move |error: cpal::Error| {
                tracing::warn!("sound output error: {error}");
                error_flag.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|error| tracing::warn!("could not open the speakers for sounds: {error}"))
        .ok()?;
    stream.play().map_err(|error| tracing::warn!("could not start sounds: {error}")).ok()?;
    Some(Output { _stream: stream, broken })
}

fn run(rx: mpsc::Receiver<Command>) {
    let mixer = Arc::new(Mutex::new(Mixer { gain: 1.0, ..Default::default() }));
    let mut rendered: HashMap<Sound, Arc<[f32]>> = HashMap::new();
    let mut last_started: HashMap<Sound, Instant> = HashMap::new();
    let mut ring: Option<Arc<[f32]>> = None;
    let mut device: Option<String> = None;
    let mut output: Option<Output> = None;
    let mut idle_since: Option<Instant> = None;
    loop {
        let command = if output.is_some() {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        };
        match command {
            Some(Command::Play(sound)) => {
                let now = Instant::now();
                if last_started.get(&sound).is_some_and(|at| now.duration_since(*at) < REPEAT_GAP) {
                    continue;
                }
                last_started.insert(sound, now);
                let samples = rendered.entry(sound).or_insert_with(|| render(sound).into()).clone();
                if let Ok(mut mixer) = mixer.lock() {
                    if mixer.voices.len() >= MAX_VOICES {
                        mixer.voices.remove(0);
                    }
                    mixer.voices.push((samples, 0));
                }
            }
            Some(Command::Ring(on)) => {
                let samples = on.then(|| ring.get_or_insert_with(|| render_ring().into()).clone());
                if let Ok(mut mixer) = mixer.lock() {
                    match samples {
                        Some(samples) if mixer.ring.is_none() => mixer.ring = Some((samples, 0)),
                        Some(_) => {}
                        None => mixer.ring = None,
                    }
                }
            }
            Some(Command::Device(name)) => {
                if name != device {
                    device = name;
                    output = None;
                }
            }
            Some(Command::Volume(gain)) => {
                if let Ok(mut mixer) = mixer.lock() {
                    mixer.gain = gain;
                }
            }
            None => {}
        }
        if output.as_ref().is_some_and(|output| output.broken.load(Ordering::Relaxed)) {
            output = None;
        }
        let idle = mixer.lock().map_or(true, |mixer| mixer.idle());
        if idle {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= IDLE_CLOSE {
                output = None;
            }
        } else {
            idle_since = None;
            if output.is_none() {
                output = open(device.as_deref(), &mixer);
                if output.is_none() {
                    if let Ok(mut mixer) = mixer.lock() {
                        mixer.voices.clear();
                    }
                }
            }
        }
    }
}

fn send(command: Command) {
    static PLAYER: OnceLock<Mutex<mpsc::Sender<Command>>> = OnceLock::new();
    let player = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        if let Err(error) = std::thread::Builder::new().name("rustcord-sounds".into()).spawn(move || run(rx)) {
            tracing::warn!("could not start the sound player: {error}");
        }
        Mutex::new(tx)
    });
    if let Ok(tx) = player.lock() {
        let _ = tx.send(command);
    }
}

pub fn play(sound: Sound) {
    send(Command::Play(sound));
}

pub fn set_ringing(on: bool) {
    send(Command::Ring(on));
}

pub fn set_output_device(name: Option<String>) {
    send(Command::Device(name));
}

pub fn set_volume(slider: f32) {
    send(Command::Volume(slider_gain(slider)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_and_start_and_end_quietly() {
        for sound in Sound::ALL {
            let samples = render(sound);
            let seconds = samples.len() as f32 / RATE;
            assert!((0.15..1.5).contains(&seconds), "{sound:?} lasts {seconds} s");
            let (_, _, loudness) = score(sound);
            let expected = PEAK * 10f32.powf(loudness / 20.0);
            assert!((peak_of(&samples) - expected).abs() < 1e-3, "{sound:?} peaks at {}", peak_of(&samples));
            assert!(samples[0].abs() < 1e-3, "{sound:?} starts with a click");
            assert!(samples[samples.len() - 1].abs() < 1e-4, "{sound:?} ends abruptly");
        }
    }

    #[test]
    fn the_ring_loops_without_a_click() {
        let ring = render_ring();
        assert_eq!(ring.len(), (RING_CYCLE * RATE) as usize);
        assert!(ring[0].abs() < 1e-3 && ring[ring.len() - 1].abs() < 1e-3);
        assert!(peak_of(&ring) <= PEAK + 1e-3);
    }

    #[test]
    fn the_mixer_drops_finished_sounds_and_keeps_ringing() {
        let mut mixer = Mixer { gain: 1.0, ..Default::default() };
        mixer.voices.push((vec![0.5f32; 4].into(), 0));
        mixer.ring = Some((vec![0.25f32; 3].into(), 0));
        let mut out = [0i16; 6];
        mixer.fill(&mut out);
        assert!(mixer.voices.is_empty());
        assert!(!mixer.idle());
        assert_eq!(out[0], (0.75 * f32::from(i16::MAX)) as i16);
        assert_eq!(out[5], (0.25 * f32::from(i16::MAX)) as i16);
        mixer.ring = None;
        assert!(mixer.idle());
    }
}
