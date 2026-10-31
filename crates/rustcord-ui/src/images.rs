use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const TEXTURE_BUDGET_BYTES: usize = 192 * 1024 * 1024;
const IN_USE_WINDOW: Duration = Duration::from_secs(2);
const MAX_ANIMATED_DIM: u32 = 640;
const MAX_ANIMATED_BYTES: usize = 48 * 1024 * 1024;

pub enum Decoded {
    Still(image::RgbaImage),
    Animated(Vec<(Duration, image::RgbaImage)>),
    Failed,
}

pub fn decode(bytes: &[u8]) -> Decoded {
    if matches!(image::guess_format(bytes), Ok(image::ImageFormat::Gif)) {
        return match decode_gif_frames(bytes) {
            Ok(frames) if frames.len() > 1 => Decoded::Animated(shrink_frames(frames)),
            Ok(mut frames) if !frames.is_empty() => Decoded::Still(frames.remove(0).1),
            _ => Decoded::Failed,
        };
    }
    match image::load_from_memory(bytes) {
        Ok(img) => Decoded::Still(img.to_rgba8()),
        Err(e) => {
            tracing::warn!("failed to decode image: {e}");
            Decoded::Failed
        }
    }
}

fn shrink_frames(mut frames: Vec<(Duration, image::RgbaImage)>) -> Vec<(Duration, image::RgbaImage)> {
    let (w, h) = frames[0].1.dimensions();
    let longest = w.max(h);
    if longest > MAX_ANIMATED_DIM {
        let scale = MAX_ANIMATED_DIM as f32 / longest as f32;
        let (nw, nh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
        for (_, f) in &mut frames {
            *f = image::imageops::resize(f, nw, nh, image::imageops::FilterType::Triangle);
        }
    }
    let (w, h) = frames[0].1.dimensions();
    if frames.len() * w as usize * h as usize * 4 > MAX_ANIMATED_BYTES {
        frames.truncate(1);
    }
    frames
}

struct DecodedFrame {
    texture_id: imgui::TextureId,
    delay: Duration,
    #[cfg(feature = "video")]
    bytes: usize,
    #[cfg(feature = "video")]
    pts_secs: f64,
}

enum Kind {
    Static { texture_id: imgui::TextureId },
    Animated {
        frames: Vec<DecodedFrame>,
        current_frame: usize,
        frame_started_at: Instant,
        visible: Cell<bool>,
    },
    #[cfg(feature = "video")]
    Video(VideoEntry),
}

#[cfg(feature = "video")]
pub enum PosterState {
    Ready { texture_id: imgui::TextureId, size: [f32; 2] },
    Loading,
    Unavailable,
}

#[cfg(feature = "video")]
pub struct VideoStatus {
    pub playing: bool,
    pub ended: bool,
    pub buffering: bool,
    pub has_audio: bool,
    pub muted: bool,
    pub volume: f32,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub fullscreen: bool,
}

#[cfg(feature = "video")]
pub fn poster_key(url: &str) -> String {
    format!("video-poster::{url}")
}

#[cfg(feature = "video")]
pub struct VideoMeta {
    pub duration_secs: f64,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
}

#[cfg(feature = "video")]
pub struct VideoBatch {
    pub url: String,
    pub batch: crate::video::Batch,
    pub meta: Option<VideoMeta>,
    pub seek_generation: u64,
}

#[cfg(feature = "video")]
struct VideoEntry {
    frames: std::collections::VecDeque<DecodedFrame>,
    frame_started_at: Instant,
    visible: Cell<bool>,
    playing: bool,
    requesting: bool,
    source_finished: bool,
    ended: bool,
    audio: Option<crate::video_audio::VideoAudio>,
    has_audio: bool,
    duration_secs: f64,
    position_secs: f64,
    volume: f32,
    muted: bool,
    seek_generation: u64,
    fullscreen: bool,
    pending_clear: bool,
    last_visible: Instant,
    last_tick: Instant,
    awaiting_data: bool,
    retry_at: Option<Instant>,
    undecoded: std::collections::VecDeque<crate::video::VideoFrame>,
}

enum Entry {
    Loaded { kind: Kind, size: [f32; 2], bytes: usize, last_used: Cell<Instant> },
    Pending,
    Failed,
}

const MAX_EMOJI_IN_FLIGHT: usize = 16;

#[cfg(feature = "video")]
const VIDEO_IDLE_GRACE: Duration = Duration::from_millis(500);

#[cfg(feature = "video")]
const MAX_VIDEO_UPLOADS_PER_FRAME: usize = 4;

#[cfg(feature = "video")]
const AUDIO_MIN_BUFFER_SECS: f64 = 0.8;

#[cfg(feature = "video")]
const HARD_BUFFER_CEILING_SECS: f64 = 6.0;

#[cfg(feature = "video")]
const AWAITING_DATA_RETRY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct TextureCache {
    entries: HashMap<String, Entry>,
    wanted: std::cell::RefCell<std::collections::HashSet<String>>,
    total_bytes: usize,
    pub animating: bool,
    #[cfg(feature = "video")]
    stopped_videos: std::cell::RefCell<Vec<String>>,
}

impl TextureCache {
    pub fn request(&mut self, url: &str, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        if self.entries.contains_key(url) {
            return;
        }
        self.entries.insert(url.to_string(), Entry::Pending);
        let _ = cmd_tx.send(crate::backend::UiCommand::DownloadImage { url: url.to_string() });
    }

    pub fn want(&self, url: &str) {
        if !self.entries.contains_key(url) {
            self.wanted.borrow_mut().insert(url.to_string());
        }
    }

    pub fn flush_wanted(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        let wanted = std::mem::take(&mut *self.wanted.borrow_mut());
        if wanted.is_empty() {
            return;
        }
        let in_flight = self.entries.iter().filter(|(url, e)| matches!(e, Entry::Pending) && url.starts_with(crate::emoji::BASE_URL)).count();
        for url in wanted.into_iter().take(MAX_EMOJI_IN_FLIGHT.saturating_sub(in_flight)) {
            self.request(&url, cmd_tx);
        }
    }

    pub fn ingest(&mut self, url: String, decoded: Decoded, device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut imgui_wgpu::Renderer) {
        let now = Instant::now();
        let entry = match decoded {
            Decoded::Failed => Entry::Failed,
            Decoded::Still(rgba) => Entry::Loaded {
                size: [rgba.width() as f32, rgba.height() as f32],
                bytes: rgba.as_raw().len(),
                kind: Kind::Static { texture_id: upload_texture(device, queue, renderer, &rgba) },
                last_used: Cell::new(now),
            },
            Decoded::Animated(frames) => {
                let size = [frames[0].1.width() as f32, frames[0].1.height() as f32];
                let bytes = frames.iter().map(|(_, f)| f.as_raw().len()).sum();
                let frames = frames
                    .into_iter()
                    .map(|(delay, rgba)| DecodedFrame {
                        texture_id: upload_texture(device, queue, renderer, &rgba),
                        delay,
                        #[cfg(feature = "video")]
                        bytes: rgba.as_raw().len(),
                        #[cfg(feature = "video")]
                        pts_secs: 0.0,
                    })
                    .collect();
                Entry::Loaded {
                    size,
                    bytes,
                    kind: Kind::Animated { frames, current_frame: 0, frame_started_at: now, visible: Cell::new(false) },
                    last_used: Cell::new(now),
                }
            }
        };
        if let Entry::Loaded { bytes, .. } = &entry {
            self.total_bytes += bytes;
        }
        self.entries.insert(url, entry);
    }

    pub fn evict(
        &mut self,
        renderer: &mut imgui_wgpu::Renderer,
        #[cfg(feature = "video")] cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>,
    ) {
        if self.total_bytes <= TEXTURE_BUDGET_BYTES {
            return;
        }
        let now = Instant::now();
        let mut candidates: Vec<(Instant, String)> = self
            .entries
            .iter()
            .filter_map(|(url, e)| match e {
                Entry::Loaded { last_used, .. } if now.duration_since(last_used.get()) > IN_USE_WINDOW => {
                    Some((last_used.get(), url.clone()))
                }
                _ => None,
            })
            .collect();
        candidates.sort();
        for (_, url) in candidates {
            if self.total_bytes <= TEXTURE_BUDGET_BYTES {
                break;
            }
            if let Some(Entry::Loaded { kind, bytes, .. }) = self.entries.remove(&url) {
                self.total_bytes -= bytes;
                match kind {
                    Kind::Static { texture_id } => {
                        renderer.textures.remove(texture_id);
                    }
                    Kind::Animated { frames, .. } => {
                        for f in frames {
                            renderer.textures.remove(f.texture_id);
                        }
                    }
                    #[cfg(feature = "video")]
                    Kind::Video(video) => {
                        for f in video.frames {
                            renderer.textures.remove(f.texture_id);
                        }
                        let _ = cmd_tx.send(crate::backend::UiCommand::CloseVideo { url: url.clone() });
                    }
                }
            }
        }
    }

    pub fn tick(
        &mut self,
        #[cfg(feature = "video")] renderer: &mut imgui_wgpu::Renderer,
        #[cfg(feature = "video")] cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>,
    ) {
        let now = Instant::now();
        self.animating = false;
        #[cfg(feature = "video")]
        {
            self.drop_stopped_videos(renderer);
            self.tick_videos(now, renderer, cmd_tx);
        }
        for entry in self.entries.values_mut() {
            if let Entry::Loaded { kind: Kind::Animated { frames, current_frame, frame_started_at, visible }, .. } = entry {
                if !visible.replace(false) {
                    *frame_started_at = now;
                    continue;
                }
                self.animating = true;
                if now.duration_since(*frame_started_at) >= frames[*current_frame].delay {
                    *current_frame = (*current_frame + 1) % frames.len();
                    *frame_started_at = now;
                }
            }
        }
    }

    #[cfg(feature = "video")]
    pub fn start_video(&mut self, url: &str, size: [f32; 2], cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        if self.entries.contains_key(url) {
            return;
        }
        self.entries.insert(
            url.to_string(),
            Entry::Loaded {
                kind: Kind::Video(VideoEntry {
                    frames: std::collections::VecDeque::new(),
                    frame_started_at: Instant::now(),
                    visible: Cell::new(true),
                    playing: true,
                    requesting: true,
                    source_finished: false,
                    ended: false,
                    audio: None,
                    has_audio: false,
                    duration_secs: 0.0,
                    position_secs: 0.0,
                    volume: 1.0,
                    muted: false,
                    seek_generation: 0,
                    fullscreen: false,
                    pending_clear: false,
                    last_visible: Instant::now(),
                    last_tick: Instant::now(),
                    awaiting_data: false,
                    retry_at: None,
                    undecoded: std::collections::VecDeque::new(),
                }),
                size,
                bytes: 0,
                last_used: Cell::new(Instant::now()),
            },
        );
        let _ = cmd_tx.send(crate::backend::UiCommand::OpenVideo { url: url.to_string() });
    }

    #[cfg(feature = "video")]
    pub fn poster(&mut self, url: &str, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) -> PosterState {
        let key = poster_key(url);
        match self.entries.get(&key) {
            Some(Entry::Loaded { kind: Kind::Static { texture_id }, size, last_used, .. }) => {
                last_used.set(Instant::now());
                PosterState::Ready { texture_id: *texture_id, size: *size }
            }
            Some(Entry::Failed) => PosterState::Unavailable,
            Some(_) => PosterState::Loading,
            None => {
                self.entries.insert(key, Entry::Pending);
                let _ = cmd_tx.send(crate::backend::UiCommand::LoadVideoPoster { url: url.to_string() });
                PosterState::Loading
            }
        }
    }

    #[cfg(feature = "video")]
    pub fn is_video(&self, url: &str) -> bool {
        matches!(self.entries.get(url), Some(Entry::Loaded { kind: Kind::Video(_), .. }))
    }

    #[cfg(feature = "video")]
    pub fn toggle_video_playback(&mut self, url: &str) {
        if let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get_mut(url) {
            if video.ended {
                return;
            }
            video.playing = !video.playing;
            video.frame_started_at = Instant::now();
            if let Some(audio) = &video.audio {
                audio.set_paused(!video.playing);
            }
        }
    }

    #[cfg(feature = "video")]
    pub fn video_status(&self, url: &str) -> Option<VideoStatus> {
        let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get(url) else { return None };
        Some(VideoStatus {
            playing: video.playing,
            ended: video.ended,
            buffering: video.frames.is_empty(),
            has_audio: video.has_audio,
            muted: video.muted,
            volume: video.volume,
            position_secs: video.position_secs,
            duration_secs: video.duration_secs,
            fullscreen: video.fullscreen,
        })
    }

    #[cfg(feature = "video")]
    pub fn set_video_volume(&mut self, url: &str, volume: f32) {
        if let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get_mut(url) {
            video.volume = volume.clamp(0.0, 1.0);
            video.muted = video.volume <= f32::EPSILON;
            if let Some(audio) = &video.audio {
                audio.set_volume(video.volume);
            }
        }
    }

    #[cfg(feature = "video")]
    pub fn toggle_video_mute(&mut self, url: &str) {
        if let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get_mut(url) {
            video.muted = !video.muted;
            if let Some(audio) = &video.audio {
                audio.set_volume(if video.muted { 0.0 } else { video.volume });
            }
        }
    }

    #[cfg(feature = "video")]
    pub fn toggle_video_fullscreen(&mut self, url: &str) {
        if let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get_mut(url) {
            video.fullscreen = !video.fullscreen;
        }
    }

    #[cfg(feature = "video")]
    pub fn exit_video_fullscreen(&mut self, url: &str) {
        if let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get_mut(url) {
            video.fullscreen = false;
        }
    }

    #[cfg(feature = "video")]
    pub fn fullscreen_video(&self) -> Option<String> {
        self.entries.iter().find_map(|(url, entry)| match entry {
            Entry::Loaded { kind: Kind::Video(video), .. } if video.fullscreen => Some(url.clone()),
            _ => None,
        })
    }

    #[cfg(feature = "video")]
    pub fn request_video_seek(&mut self, url: &str, target_secs: f64, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        let Some(Entry::Loaded { kind: Kind::Video(video), .. }) = self.entries.get_mut(url) else { return };
        let target = target_secs.clamp(0.0, video.duration_secs.max(0.0));
        video.seek_generation += 1;
        video.requesting = true;
        video.source_finished = false;
        video.ended = false;
        video.playing = true;
        video.position_secs = target;
        video.pending_clear = true;
        if let Some(audio) = &video.audio {
            audio.reset_clock(target);
            audio.set_paused(false);
        }
        let _ = cmd_tx.send(crate::backend::UiCommand::SeekVideo { url: url.to_string(), target_secs: target });
    }

    #[cfg(feature = "video")]
    pub fn mark_video_stopped(&self, url: &str) {
        self.stopped_videos.borrow_mut().push(url.to_string());
    }

    #[cfg(feature = "video")]
    fn drop_stopped_videos(&mut self, renderer: &mut imgui_wgpu::Renderer) {
        let stopped = std::mem::take(&mut *self.stopped_videos.borrow_mut());
        for url in stopped {

            if let Some(Entry::Loaded { kind: Kind::Video(video), bytes, .. }) = self.entries.remove(&url) {
                self.total_bytes = self.total_bytes.saturating_sub(bytes);
                for f in video.frames {
                    renderer.textures.remove(f.texture_id);
                }
            }
        }
    }

    #[cfg(feature = "video")]
    pub fn ingest_video_batch(&mut self, batch: crate::images::VideoBatch, renderer: &mut imgui_wgpu::Renderer) {
        let Some(Entry::Loaded { kind: Kind::Video(video), size, bytes, .. }) = self.entries.get_mut(&batch.url) else {
            return;
        };
        if batch.seek_generation < video.seek_generation {
            return;
        }
        video.requesting = false;
        video.source_finished = batch.batch.finished;
        video.awaiting_data = batch.batch.awaiting_data;
        if video.pending_clear {
            video.pending_clear = false;
            video.undecoded.clear();
            for frame in video.frames.drain(..) {
                renderer.textures.remove(frame.texture_id);
                *bytes = bytes.saturating_sub(frame.bytes);
                self.total_bytes = self.total_bytes.saturating_sub(frame.bytes);
            }
        }

        if let Some(meta) = batch.meta {
            video.duration_secs = meta.duration_secs;
            video.has_audio = meta.has_audio;
            *size = [meta.width as f32, meta.height as f32];
            if meta.has_audio && video.audio.is_none() {
                video.audio = crate::video_audio::VideoAudio::start();
                if let Some(audio) = &video.audio {
                    audio.set_volume(video.volume);
                }
            }
        }

        if let Some(audio) = &video.audio {
            for chunk in &batch.batch.audio {
                audio.push(&chunk.samples);
            }
        }

        video.undecoded.extend(batch.batch.frames);
    }

    #[cfg(feature = "video")]
    pub fn upload_pending_video_frames(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut imgui_wgpu::Renderer,
    ) {
        for entry in self.entries.values_mut() {
            let Entry::Loaded { kind: Kind::Video(video), bytes, .. } = entry else { continue };
            let budget = if video.frames.is_empty() { MAX_VIDEO_UPLOADS_PER_FRAME * 2 } else { MAX_VIDEO_UPLOADS_PER_FRAME };
            for _ in 0..budget {
                let Some(frame) = video.undecoded.pop_front() else { break };
                let frame_bytes = frame.image.as_raw().len();
                video.frames.push_back(DecodedFrame {
                    texture_id: upload_texture(device, queue, renderer, &frame.image),
                    delay: frame.delay,
                    bytes: frame_bytes,
                    pts_secs: frame.pts_secs,
                });
                *bytes += frame_bytes;
                self.total_bytes += frame_bytes;
            }
        }
    }

    #[cfg(feature = "video")]
    fn tick_videos(&mut self, now: Instant, renderer: &mut imgui_wgpu::Renderer, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        for (url, entry) in self.entries.iter_mut() {
            let Entry::Loaded { kind: Kind::Video(video), bytes, .. } = entry else { continue };
            let visible = video.visible.replace(false);
            if visible {
                video.last_visible = now;
            }
            let recently_visible = now.duration_since(video.last_visible) < VIDEO_IDLE_GRACE;
            if (video.playing || !video.undecoded.is_empty()) && recently_visible {
                self.animating = true;
            }
            if !visible || !video.playing {
                video.frame_started_at = now;
                continue;
            }

            let mut drop_front = |video: &mut VideoEntry, bytes: &mut usize, total: &mut usize| {
                let done = video.frames.pop_front().expect("caller checked len");
                renderer.textures.remove(done.texture_id);
                *bytes = bytes.saturating_sub(done.bytes);
                *total = total.saturating_sub(done.bytes);
            };

            match video.audio.as_ref().map(|a| a.clock_secs()) {
                Some(clock) => {
                    if std::env::var("RUSTCORD_VIDEO_TRACE").is_ok() {
                        let tick_ms = now.duration_since(video.last_tick).as_secs_f64() * 1000.0;
                        let front = video.frames.front().map(|f| f.pts_secs).unwrap_or(-1.0);
                        eprintln!(
                            "video tick: +{tick_ms:.1}ms clock {clock:.3} front_pts {front:.3} lead {:.3} ready {} pending {} audio_buf {:.2}s",
                            front - clock,
                            video.frames.len(),
                            video.undecoded.len(),
                            video.audio.as_ref().map(|a| a.buffered_secs()).unwrap_or(0.0),
                        );
                    }
                    video.last_tick = now;
                    video.position_secs = clock;
                    while video.frames.len() > 1 && video.frames[1].pts_secs <= clock {
                        drop_front(video, bytes, &mut self.total_bytes);
                    }
                    if video.frames.len() <= 1 && video.source_finished && clock >= video.duration_secs.max(0.0) {
                        video.ended = true;
                        video.playing = false;
                    }
                    video.frame_started_at = now;
                }
                None => {
                    let elapsed = now.duration_since(video.frame_started_at);
                    match advance_decision(video.frames.front().map(|f| f.delay), video.frames.len(), elapsed, video.source_finished) {
                        Advance::Hold => {}
                        Advance::NextFrame => {
                            drop_front(video, bytes, &mut self.total_bytes);
                            video.frame_started_at = now;
                            video.position_secs = video.frames.front().map(|f| f.pts_secs).unwrap_or(video.position_secs);
                        }
                        Advance::Starved => {
                            video.frame_started_at = now;
                        }
                        Advance::End => {
                            video.frame_started_at = now;
                            video.ended = true;
                            video.playing = false;
                        }
                    }
                }
            }
            let buffered_secs = match (video.frames.front(), video.frames.back()) {
                (Some(front), Some(back)) => back.pts_secs - front.pts_secs + back.delay.as_secs_f64(),
                _ => 0.0,
            };
            let audio_buffered = video.audio.as_ref().map(|a| a.buffered_secs());
            let pending_secs: f64 = video.undecoded.iter().map(|f| f.delay.as_secs_f64()).sum();
            let video_secs = buffered_secs + pending_secs;
            if video.awaiting_data {
                let ready = video.retry_at.is_none_or(|at| now >= at);
                if !video.requesting && ready {
                    video.requesting = true;
                    video.retry_at = Some(now + AWAITING_DATA_RETRY);
                    let _ = cmd_tx.send(crate::backend::UiCommand::DecodeVideoBatch { url: url.clone() });
                }
                self.animating = true;
            } else if !video.requesting && !video.source_finished && should_refill(video_secs, audio_buffered) {
                video.requesting = true;
                let _ = cmd_tx.send(crate::backend::UiCommand::DecodeVideoBatch { url: url.clone() });
            }
        }
    }

    pub fn get(&self, url: &str) -> Option<(imgui::TextureId, [f32; 2])> {
        self.lookup(url, true)
    }

    pub fn get_still(&self, url: &str) -> Option<(imgui::TextureId, [f32; 2])> {
        self.lookup(url, false)
    }

    fn lookup(&self, url: &str, animate: bool) -> Option<(imgui::TextureId, [f32; 2])> {
        let Entry::Loaded { kind, size, last_used, .. } = self.entries.get(url)? else { return None };
        last_used.set(Instant::now());
        match kind {
            Kind::Static { texture_id } => Some((*texture_id, *size)),
            Kind::Animated { frames, current_frame, visible, .. } => {
                if animate {
                    visible.set(true);
                    frames.get(*current_frame)
                } else {
                    frames.first()
                }
                .map(|f| (f.texture_id, *size))
            }
            #[cfg(feature = "video")]
            Kind::Video(video) => {
                if animate {
                    video.visible.set(true);
                }
                video.frames.front().map(|f| (f.texture_id, *size))
            }
        }
    }

    pub fn is_failed(&self, url: &str) -> bool {
        matches!(self.entries.get(url), Some(Entry::Failed))
    }
}

fn decode_gif_frames(bytes: &[u8]) -> image::ImageResult<Vec<(Duration, image::RgbaImage)>> {
    use image::{codecs::gif::GifDecoder, AnimationDecoder};
    let decoder = GifDecoder::new(std::io::Cursor::new(bytes))?;
    let frames = decoder.into_frames().collect_frames()?;
    Ok(frames
        .into_iter()
        .map(|f| {
            let delay = Duration::from(f.delay());
            let delay = if delay.is_zero() { Duration::from_millis(100) } else { delay };
            (delay, f.into_buffer())
        })
        .collect())
}

#[cfg(feature = "video")]
fn should_refill(video_secs: f64, audio_secs: Option<f64>) -> bool {
    let video_low = video_secs < crate::video::MIN_BUFFER_SECS;
    let audio_low = audio_secs.is_some_and(|audio| audio < AUDIO_MIN_BUFFER_SECS);
    if audio_low {
        return video_secs < HARD_BUFFER_CEILING_SECS;
    }
    video_low && video_secs < crate::video::MAX_BUFFER_SECS
}

#[cfg(feature = "video")]
#[derive(Debug, PartialEq, Eq)]
enum Advance {
    Hold,
    NextFrame,
    Starved,
    End,
}

#[cfg(feature = "video")]
fn advance_decision(front_delay: Option<Duration>, queued: usize, elapsed: Duration, source_finished: bool) -> Advance {
    let Some(delay) = front_delay else { return Advance::Hold };
    if elapsed < delay {
        return Advance::Hold;
    }
    if queued > 1 {
        Advance::NextFrame
    } else if source_finished {
        Advance::End
    } else {
        Advance::Starved
    }
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut imgui_wgpu::Renderer,
    rgba: &image::RgbaImage,
) -> imgui::TextureId {
    let (width, height) = rgba.dimensions();
    let texture_config = imgui_wgpu::TextureConfig {
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        label: Some("chat-image"),
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..Default::default()
    };
    let texture = imgui_wgpu::Texture::new(device, renderer, texture_config);
    texture.write(queue, rgba.as_raw(), width, height);
    renderer.textures.insert(texture)
}

#[cfg(all(test, feature = "video"))]
mod video_tests {
    use super::*;

    #[test]
    fn a_poster_key_cannot_collide_with_a_real_url() {
        let url = "https://cdn.discordapp.com/attachments/1/2/clip.mp4";
        let key = poster_key(url);
        assert_ne!(key, url);
        assert!(!key.starts_with("http"));
    }

    #[test]
    fn poster_keys_are_distinct_per_video() {
        assert_ne!(poster_key("https://a/1.mp4"), poster_key("https://a/2.mp4"));
    }


    const FRAME: Duration = Duration::from_millis(40);

    #[test]
    fn holds_until_the_frame_delay_elapses() {
        assert_eq!(advance_decision(Some(FRAME), 5, Duration::from_millis(10), false), Advance::Hold);
    }

    #[test]
    fn advances_once_the_delay_elapses() {
        assert_eq!(advance_decision(Some(FRAME), 5, FRAME, false), Advance::NextFrame);
    }

    #[test]
    fn an_empty_queue_holds_rather_than_ending() {
        assert_eq!(advance_decision(None, 0, Duration::from_secs(10), false), Advance::Hold);
    }

    #[test]
    fn the_last_frame_starves_while_more_is_coming() {
        assert_eq!(advance_decision(Some(FRAME), 1, FRAME, false), Advance::Starved);
    }

    #[test]
    fn the_last_frame_ends_once_the_source_is_exhausted() {
        assert_eq!(advance_decision(Some(FRAME), 1, FRAME, true), Advance::End);
    }

    #[test]
    fn low_audio_refills_even_when_video_is_healthy() {
        assert!(should_refill(2.5, Some(0.3)));
    }

    #[test]
    fn low_video_refills_even_when_audio_is_healthy() {
        assert!(should_refill(0.5, Some(2.0)));
    }

    #[test]
    fn a_healthy_queue_on_both_sides_does_not_refill() {
        assert!(!should_refill(2.5, Some(1.5)));
    }

    #[test]
    fn the_video_buffer_is_capped_when_audio_is_healthy() {
        assert!(!should_refill(crate::video::MAX_BUFFER_SECS + 1.0, Some(2.0)));
        assert!(!should_refill(crate::video::MAX_BUFFER_SECS + 1.0, None));
    }

    #[test]
    fn starving_audio_is_served_past_the_video_ceiling_but_not_forever() {
        assert!(should_refill(crate::video::MAX_BUFFER_SECS + 1.0, Some(0.1)));
        assert!(!should_refill(HARD_BUFFER_CEILING_SECS + 1.0, Some(0.1)));
    }

    #[test]
    fn a_silent_video_refills_on_video_alone() {
        assert!(should_refill(0.5, None));
        assert!(!should_refill(2.5, None));
    }

    #[test]
    fn a_starved_player_does_not_burn_frames_catching_up() {
        let backlog = Duration::from_secs(5);
        assert_eq!(advance_decision(Some(FRAME), 1, backlog, false), Advance::Starved);
        assert_eq!(advance_decision(Some(FRAME), 2, backlog, false), Advance::NextFrame);
    }
}
