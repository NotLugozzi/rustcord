use ffmpeg_next as ffmpeg;
use std::time::Duration;

const MAX_VIDEO_DIM: u32 = 640;

pub const BATCH_FRAMES: usize = 48;

pub const MIN_BUFFER_SECS: f64 = 1.0;

pub const MAX_BUFFER_SECS: f64 = 2.5;

const BATCH_SECS: f64 = 3.0;

const AUDIO_LEAD_SECS: f64 = 0.25;

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u16 = 2;

pub struct VideoFrame {
    pub pts_secs: f64,
    pub delay: Duration,
    pub image: image::RgbaImage,
}

pub struct AudioChunk {
    pub pts_secs: f64,
    pub samples: Vec<f32>,
}

#[derive(Default)]
pub struct Batch {
    pub frames: Vec<VideoFrame>,
    pub audio: Vec<AudioChunk>,
    pub finished: bool,
    pub awaiting_data: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    Growing,
    Complete,
}

struct AudioTrack {
    decoder: ffmpeg::decoder::Audio,
    resampler: ffmpeg::software::resampling::Context,
    stream_index: usize,
    time_base: f64,
}

pub struct VideoDecoder {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    stream_index: usize,
    time_base: f64,
    last_pts_secs: Option<f64>,
    nominal_delay: Duration,
    audio: Option<AudioTrack>,
    pub duration_secs: f64,
    discard_before_secs: f64,
    video_pts_secs: f64,
    audio_pts_secs: f64,
    pub width: u32,
    pub height: u32,
    finished: bool,
}

impl VideoDecoder {
    pub fn open(path: &std::path::Path) -> Result<Self, ffmpeg::Error> {
        ffmpeg::init()?;
        let input = ffmpeg::format::input(path)?;
        Self::from_input(input)
    }

    fn from_input(input: ffmpeg::format::context::Input) -> Result<Self, ffmpeg::Error> {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        let stream_index = stream.index();
        let tb = stream.time_base();
        let time_base = f64::from(tb.numerator()) / f64::from(tb.denominator());

        let rate = stream.avg_frame_rate();
        let nominal_delay = if rate.numerator() > 0 {
            Duration::from_secs_f64(f64::from(rate.denominator()) / f64::from(rate.numerator()))
        } else {
            Duration::from_millis(33)
        };

        let stream_duration = stream.duration();
        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().video()?;

        let duration_secs = if stream_duration > 0 {
            stream_duration as f64 * time_base
        } else {
            let whole = input.duration();
            if whole > 0 { whole as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE) } else { 0.0 }
        };

        let audio = open_audio_track(&input);

        let (width, height) = scaled_dimensions(decoder.width(), decoder.height());
        let scaler = ffmpeg::software::scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            ffmpeg::format::Pixel::RGBA,
            width,
            height,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )?;

        Ok(Self {
            input,
            decoder,
            scaler,
            stream_index,
            time_base,
            last_pts_secs: None,
            nominal_delay,
            audio,
            duration_secs,
            discard_before_secs: 0.0,
            video_pts_secs: 0.0,
            audio_pts_secs: 0.0,
            width,
            height,
            finished: false,
        })
    }

    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    pub fn seek(&mut self, target_secs: f64) -> Result<(), ffmpeg::Error> {
        let clamped = target_secs.clamp(0.0, self.duration_secs.max(0.0));
        let ts = (clamped * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        self.input.seek(ts, ..)?;
        self.decoder.flush();
        if let Some(audio) = &mut self.audio {
            audio.decoder.flush();
        }
        self.last_pts_secs = None;
        self.finished = false;
        self.discard_before_secs = clamped;
        self.video_pts_secs = clamped;
        self.audio_pts_secs = clamped;
        Ok(())
    }

    pub fn next_batch(&mut self, max_frames: usize, source: SourceState) -> Batch {
        let mut batch = Batch::default();
        if self.finished {
            batch.finished = true;
            return batch;
        }

        let mut decoded = ffmpeg::frame::Video::empty();
        let mut rgba = ffmpeg::frame::Video::empty();

        let mut span_target = None;
        loop {
            let span = match (batch.frames.first(), batch.frames.last()) {
                (Some(first), Some(last)) => last.pts_secs - first.pts_secs + last.delay.as_secs_f64(),
                _ => 0.0,
            };
            let span_done = span_target.is_some_and(|target: f64| span >= target);
            let video_done = batch.frames.len() >= max_frames || span_done;
            let audio_done = match &self.audio {
                Some(_) => self.audio_pts_secs >= self.video_pts_secs + AUDIO_LEAD_SECS,
                None => true,
            };
            if video_done && audio_done {
                break;
            }

            let Some((stream, packet)) = self.input.packets().next() else {
                if source == SourceState::Growing {
                    batch.awaiting_data = true;
                    break;
                }
                let _ = self.decoder.send_eof();
                while self.decoder.receive_frame(&mut decoded).is_ok() {
                    self.push_frame(&decoded, &mut rgba, &mut batch.frames);
                }
                if let Some(audio) = &mut self.audio {
                    let _ = audio.decoder.send_eof();
                    drain_audio(audio, &mut batch.audio, &mut self.audio_pts_secs);
                }
                self.finished = true;
                break;
            };
            let index = stream.index();
            if index == self.stream_index {
                if video_done {
                    continue;
                }
                if self.decoder.send_packet(&packet).is_err() {
                    continue;
                }
                while self.decoder.receive_frame(&mut decoded).is_ok() {
                    self.push_frame(&decoded, &mut rgba, &mut batch.frames);
                }
                span_target.get_or_insert(BATCH_SECS);
            } else if let Some(audio) = &mut self.audio {
                if index == audio.stream_index && audio.decoder.send_packet(&packet).is_ok() {
                    drain_audio(audio, &mut batch.audio, &mut self.audio_pts_secs);
                }
            }
        }
        batch.finished = self.finished;
        batch
    }

    fn push_frame(&mut self, decoded: &ffmpeg::frame::Video, rgba: &mut ffmpeg::frame::Video, out: &mut Vec<VideoFrame>) {
        if self.scaler.run(decoded, rgba).is_err() {
            return;
        }
        let Some(image) = copy_to_image(rgba, self.width, self.height) else { return };

        let pts_secs = decoded.pts().map(|pts| pts as f64 * self.time_base);
        let delay = match pts_secs {
            Some(secs) => match self.last_pts_secs {
                Some(prev) if secs > prev => Duration::from_secs_f64(secs - prev),
                _ => self.nominal_delay,
            },
            None => self.nominal_delay,
        };
        let pts_secs = pts_secs.unwrap_or_else(|| self.last_pts_secs.map(|p| p + delay.as_secs_f64()).unwrap_or(0.0));
        self.last_pts_secs = Some(pts_secs);
        if pts_secs + delay.as_secs_f64() < self.discard_before_secs {
            return;
        }
        self.discard_before_secs = 0.0;
        self.video_pts_secs = pts_secs + delay.as_secs_f64();
        out.push(VideoFrame { pts_secs, delay, image });
    }
}

fn open_audio_track(input: &ffmpeg::format::context::Input) -> Option<AudioTrack> {
    let stream = input.streams().best(ffmpeg::media::Type::Audio)?;
    let stream_index = stream.index();
    let tb = stream.time_base();
    let time_base = f64::from(tb.numerator()) / f64::from(tb.denominator());

    let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters()).ok()?;
    let decoder = ctx.decoder().audio().ok()?;

    let in_layout = match decoder.channel_layout().channels() {
        0 => ffmpeg::util::channel_layout::ChannelLayout::default(decoder.channels().max(1) as i32),
        _ => decoder.channel_layout(),
    };
    let resampler = ffmpeg::software::resampling::Context::get(
        decoder.format(),
        in_layout,
        decoder.rate(),
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::util::channel_layout::ChannelLayout::STEREO,
        AUDIO_SAMPLE_RATE,
    )
    .ok()?;

    Some(AudioTrack { decoder, resampler, stream_index, time_base })
}

fn drain_audio(audio: &mut AudioTrack, out: &mut Vec<AudioChunk>, cursor_secs: &mut f64) {
    let mut decoded = ffmpeg::frame::Audio::empty();
    while audio.decoder.receive_frame(&mut decoded).is_ok() {
        let pts_secs = decoded.pts().map(|pts| pts as f64 * audio.time_base).unwrap_or(*cursor_secs);
        let capacity = resampled_capacity(decoded.samples(), decoded.rate(), audio.resampler.delay());
        let mut resampled = ffmpeg::frame::Audio::new(
            ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
            capacity,
            ffmpeg::util::channel_layout::ChannelLayout::STEREO,
        );
        if audio.resampler.run(&decoded, &mut resampled).is_err() {
            continue;
        }
        let samples = take_samples(&resampled);
        if samples.is_empty() {
            continue;
        }
        *cursor_secs = pts_secs + samples.len() as f64 / (f64::from(AUDIO_SAMPLE_RATE) * f64::from(AUDIO_CHANNELS));
        out.push(AudioChunk { pts_secs, samples });
    }
}

fn resampled_capacity(input_samples: usize, input_rate: u32, delay: Option<ffmpeg::software::resampling::Delay>) -> usize {
    let rate = if input_rate == 0 { AUDIO_SAMPLE_RATE } else { input_rate };
    let scaled = (input_samples as u64 * u64::from(AUDIO_SAMPLE_RATE)).div_ceil(u64::from(rate)) as usize;
    let buffered = delay.map(|d| d.output.max(0) as usize).unwrap_or(0);
    scaled + buffered + 1
}

fn take_samples(frame: &ffmpeg::frame::Audio) -> Vec<f32> {
    let count = frame.samples() * AUDIO_CHANNELS as usize;
    if count == 0 {
        return Vec::new();
    }
    let plane: &[f32] = frame.plane(0);
    plane[..count.min(plane.len())].to_vec()
}

pub fn first_frame(path: &std::path::Path) -> Option<image::RgbaImage> {
    let mut decoder = VideoDecoder::open(path).ok()?;
    for _ in 0..8 {
        let batch = decoder.next_batch(1, SourceState::Complete);
        if let Some(frame) = batch.frames.into_iter().next() {
            return Some(frame.image);
        }
        if batch.finished {
            break;
        }
    }
    None
}

fn scaled_dimensions(w: u32, h: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= MAX_VIDEO_DIM || longest == 0 {
        return (w.max(1), h.max(1));
    }
    let scale = MAX_VIDEO_DIM as f32 / longest as f32;
    (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1))
}

fn copy_to_image(frame: &ffmpeg::frame::Video, width: u32, height: u32) -> Option<image::RgbaImage> {
    let stride = frame.stride(0);
    let data = frame.data(0);
    let row_bytes = width as usize * 4;
    if stride < row_bytes {
        return None;
    }
    let mut packed = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        let end = start + row_bytes;
        if end > data.len() {
            return None;
        }
        packed.extend_from_slice(&data[start..end]);
    }
    image::RgbaImage::from_raw(width, height, packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_preserves_small_videos() {
        assert_eq!(scaled_dimensions(320, 240), (320, 240));
    }

    #[test]
    fn scaling_bounds_the_longest_edge() {
        assert_eq!(scaled_dimensions(1920, 1080), (640, 360));
        let (w, h) = scaled_dimensions(1080, 1920);
        assert_eq!(h, 640);
        assert_eq!(w, 360);
    }

    #[test]
    fn scaling_never_yields_a_zero_dimension() {
        let (w, h) = scaled_dimensions(4000, 1);
        assert!(w >= 1 && h >= 1);
    }

    fn fixture(name: &str, size: &str) -> Option<std::path::PathBuf> {
        let path = std::env::temp_dir().join(format!("rustcord-video-test-{name}.mp4"));
        if !path.exists() {
            let status = std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", &format!("testsrc=size={size}:rate=10:duration=2"), "-c:v", "libx264", "-pix_fmt", "yuv420p"])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .ok()?;
            if !status.success() {
                return None;
            }
        }
        Some(path)
    }

    #[test]
    fn decodes_every_frame_with_increasing_delays() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        assert_eq!((decoder.width, decoder.height), (320, 240));

        let mut frames = Vec::new();
        loop {
            let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
            let done = batch.finished;
            frames.extend(batch.frames);
            if done {
                break;
            }
        }
        assert_eq!(frames.len(), 20);
        assert!(frames.iter().skip(1).all(|f| f.delay > Duration::ZERO));
        for frame in &frames {
            assert_eq!(frame.image.dimensions(), (320, 240));
        }
    }

    #[test]
    fn presentation_timestamps_increase_monotonically() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
        assert!(batch.frames.len() > 1);
        for pair in batch.frames.windows(2) {
            assert!(pair[1].pts_secs > pair[0].pts_secs, "pts must advance");
        }
    }

    #[test]
    fn duration_is_reported() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let decoder = VideoDecoder::open(&path).expect("open");
        assert!((decoder.duration_secs - 2.0).abs() < 0.3, "got {}", decoder.duration_secs);
    }

    #[test]
    fn unaligned_width_is_unpadded_not_skewed() {
        let Some(path) = fixture("unaligned", "330x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        let batch = decoder.next_batch(4, SourceState::Complete);
        assert!(!batch.frames.is_empty());
        for frame in &batch.frames {
            assert_eq!(frame.image.dimensions(), (330, 240));
            assert_eq!(frame.image.as_raw().len(), 330 * 240 * 4);
        }
    }

    #[test]
    fn batching_resumes_rather_than_restarting() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        let first = decoder.next_batch(5, SourceState::Complete);
        let second = decoder.next_batch(5, SourceState::Complete);
        assert_eq!(first.frames.len(), 5);
        assert_eq!(second.frames.len(), 5);
        assert!(second.frames[0].pts_secs > first.frames[0].pts_secs);
    }

    #[test]
    fn seeking_moves_the_presentation_clock() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        decoder.seek(1.0).expect("seek");
        let batch = decoder.next_batch(4, SourceState::Complete);
        assert!(!batch.frames.is_empty());
        assert!(batch.frames[0].pts_secs > 0.5, "got {}", batch.frames[0].pts_secs);
    }

    #[test]
    fn seeking_forward_then_back_lands_in_both_places() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        decoder.seek(1.5).expect("seek forward");
        let late = decoder.next_batch(2, SourceState::Complete);
        assert!(late.frames[0].pts_secs > 1.0, "got {}", late.frames[0].pts_secs);
        decoder.seek(0.3).expect("seek back");
        let early = decoder.next_batch(2, SourceState::Complete);
        assert!(early.frames[0].pts_secs < 1.0, "got {}", early.frames[0].pts_secs);
    }

    #[test]
    fn seeking_back_to_zero_restarts_the_stream() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
        decoder.seek(0.0).expect("seek");
        let batch = decoder.next_batch(4, SourceState::Complete);
        assert!(!batch.frames.is_empty());
        assert!(batch.frames[0].pts_secs < 0.5, "got {}", batch.frames[0].pts_secs);
    }

    fn faststart_fixture() -> Option<std::path::PathBuf> {
        let path = std::env::temp_dir().join("rustcord-video-test-faststart.mp4");
        if !path.exists() {
            let ok = std::process::Command::new("ffmpeg")
                .args([
                    "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=6",
                    "-f", "lavfi", "-i", "sine=frequency=440:duration=6",
                    "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac",
                    "-movflags", "+faststart",
                ])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return None;
            }
        }
        Some(path)
    }

    #[test]
    fn a_poster_frame_is_decoded_from_the_start_of_the_file() {
        let Some(path) = faststart_fixture() else { return };
        let poster = first_frame(&path).expect("poster frame");
        assert_eq!(poster.dimensions(), (320, 240));
    }

    #[test]
    fn a_poster_frame_is_not_blank() {
        let Some(path) = faststart_fixture() else { return };
        let poster = first_frame(&path).expect("poster frame");
        let distinct: std::collections::HashSet<[u8; 4]> =
            poster.pixels().map(|p| [p.0[0], p.0[1], p.0[2], p.0[3]]).collect();
        assert!(distinct.len() > 1, "poster frame is a single flat colour");
    }

    #[test]
    fn a_growing_file_resumes_after_more_bytes_arrive() {
        let Some(src) = faststart_fixture() else { return };
        let full = std::fs::read(&src).expect("read fixture");
        let partial_path = std::env::temp_dir().join("rustcord-video-test-growing.mp4");
        let half = full.len() / 2;
        std::fs::write(&partial_path, &full[..half]).expect("write partial");

        let mut decoder = VideoDecoder::open(&partial_path).expect("open partial");
        let mut frames = 0;
        loop {
            let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Growing);
            frames += batch.frames.len();
            if batch.awaiting_data || batch.finished {
                assert!(batch.awaiting_data, "a truncated file should await data, not finish");
                break;
            }
        }
        assert!(frames > 0, "expected frames from the first half");

        std::fs::write(&partial_path, &full).expect("write full");
        let mut more = 0;
        loop {
            let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
            more += batch.frames.len();
            if batch.finished {
                break;
            }
        }
        assert!(more > 0, "decoder did not resume after the file grew; got {more} further frames");
    }

    #[test]
    fn a_growing_file_never_reports_finished_prematurely() {
        let Some(src) = faststart_fixture() else { return };
        let full = std::fs::read(&src).expect("read fixture");
        let path = std::env::temp_dir().join("rustcord-video-test-incremental.mp4");

        let head = full.len() / 4;
        std::fs::write(&path, &full[..head]).expect("write head");
        let mut decoder = VideoDecoder::open(&path).expect("open head");

        let mut total = 0;
        let mut saw_awaiting = false;
        for step in 1..=4 {
            loop {
                let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Growing);
                total += batch.frames.len();
                assert!(!batch.finished, "a growing source must never report finished");
                if batch.awaiting_data {
                    saw_awaiting = true;
                    break;
                }
            }
            let upto = (full.len() * step / 4).min(full.len());
            std::fs::write(&path, &full[..upto]).expect("grow file");
        }
        assert!(saw_awaiting, "expected to await data at least once");

        loop {
            let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
            total += batch.frames.len();
            if batch.finished {
                break;
            }
        }
        assert!(total > 0, "decoded no frames across the growing file");
    }

    #[test]
    fn a_non_faststart_partial_file_cannot_be_opened() {
        let Some(src) = fixture("aligned", "320x240") else { return };
        let full = std::fs::read(&src).expect("read fixture");
        let path = std::env::temp_dir().join("rustcord-video-test-nonfaststart-partial.mp4");
        std::fs::write(&path, &full[..full.len() / 2]).expect("write partial");
        assert!(
            VideoDecoder::open(&path).is_err(),
            "moov lives at the end of a non-faststart file, so a partial one has no index; \
             the backend must fall back to waiting for the full download"
        );
    }

    #[test]
    fn a_complete_source_still_reports_finished() {
        let Some(src) = faststart_fixture() else { return };
        let mut decoder = VideoDecoder::open(&src).expect("open");
        let mut finished = false;
        for _ in 0..200 {
            let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
            assert!(!batch.awaiting_data, "a complete source must not await data");
            if batch.finished {
                finished = true;
                break;
            }
        }
        assert!(finished, "a complete source should finish");
    }

    #[test]
    fn a_silent_file_reports_no_audio_track() {
        let Some(path) = fixture("aligned", "320x240") else { return };
        let decoder = VideoDecoder::open(&path).expect("open");
        assert!(!decoder.has_audio());
    }

    fn long_fixture() -> Option<std::path::PathBuf> {
        let path = std::env::temp_dir().join("rustcord-video-test-long.mp4");
        if !path.exists() {
            let ok = std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=20", "-f", "lavfi", "-i", "sine=frequency=440:duration=20", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return None;
            }
        }
        Some(path)
    }

    #[test]
    fn each_batch_carries_audio_covering_its_video_span() {
        let Some(path) = long_fixture() else { return };
        let mut decoder = VideoDecoder::open(&path).expect("open");
        let mut total_audio_samples = 0usize;
        let mut video_end = 0.0f64;
        for n in 0..8 {
            let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
            if batch.frames.is_empty() {
                break;
            }
            let last = batch.frames.last().expect("non-empty");
            video_end = last.pts_secs + last.delay.as_secs_f64();
            total_audio_samples += batch.audio.iter().map(|c| c.samples.len()).sum::<usize>();
            let audio_end = total_audio_samples as f64 / (f64::from(AUDIO_SAMPLE_RATE) * f64::from(AUDIO_CHANNELS));
            println!("batch {n}: cumulative video {video_end:.3}s, audio {audio_end:.3}s, deficit {:.3}s", video_end - audio_end);
        }
        let audio_end = total_audio_samples as f64 / (f64::from(AUDIO_SAMPLE_RATE) * f64::from(AUDIO_CHANNELS));
        assert!(
            audio_end >= video_end - 0.1,
            "audio fell {:.3}s behind video; the clock driving playback would drift",
            video_end - audio_end
        );
    }

    #[test]
    fn resampling_capacity_accounts_for_the_rate_ratio() {
        let capacity = resampled_capacity(1024, 44100, None);
        assert!(capacity >= 1115, "got {capacity}");
    }

    #[test]
    fn resampling_capacity_survives_a_zero_rate() {
        assert!(resampled_capacity(1024, 0, None) >= 1024);
    }

    #[test]
    fn an_audio_track_decodes_to_interleaved_stereo() {
        let path = std::env::temp_dir().join("rustcord-video-test-audio.mp4");
        if !path.exists() {
            let ok = std::process::Command::new("ffmpeg")
                .args(["-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=2", "-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return;
            }
        }
        let mut decoder = VideoDecoder::open(&path).expect("open");
        assert!(decoder.has_audio());
        let batch = decoder.next_batch(BATCH_FRAMES, SourceState::Complete);
        assert!(!batch.audio.is_empty(), "expected decoded audio chunks");
        assert!(batch.audio.iter().all(|c| c.samples.len() % AUDIO_CHANNELS as usize == 0));
    }
}
