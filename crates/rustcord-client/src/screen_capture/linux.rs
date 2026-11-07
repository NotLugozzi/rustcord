use crate::error::ClientError;
use super::{CapturedFrame, PixelLayout};
use libspa_sys as spa_sys;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::param::video::VideoInfoRaw;
use spa::pod::{ChoiceValue, Pod, Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Rectangle};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub const MAX_CAPTURE_WIDTH: u32 = 8192;
pub const MAX_CAPTURE_HEIGHT: u32 = 8192;

impl PixelLayout {
    fn from_spa(format: spa::param::video::VideoFormat) -> Option<Self> {
        use spa::param::video::VideoFormat;
        match format {
            VideoFormat::BGRx | VideoFormat::BGRA => Some(Self::Bgrx),
            VideoFormat::RGBx | VideoFormat::RGBA => Some(Self::Rgbx),
            _ => None,
        }
    }

}

pub struct ScreenCapture {
    frames: mpsc::Receiver<CapturedFrame>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScreenCapture {
    pub async fn start(framerate: u32) -> Result<Self, ClientError> {
        let (fd, node_id) = negotiate_portal().await?;
        let (frame_tx, frames) = mpsc::channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("screen-capture".into())
            .spawn(move || {
                if let Err(e) = run_pipewire(fd, node_id, frame_tx, thread_stop, framerate.clamp(15, 60)) {
                    tracing::error!("screen capture loop failed: {e}");
                }
            })
            .map_err(|e| ClientError::ScreenCapture(format!("could not spawn capture thread: {e}")))?;

        Ok(Self { frames, stop, thread: Some(thread) })
    }

    pub async fn next_frame(&mut self) -> Option<CapturedFrame> {
        self.frames.recv().await
    }

    pub(crate) fn blocking_next_frame(&mut self) -> Option<CapturedFrame> {
        self.frames.blocking_recv()
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn negotiate_portal() -> Result<(std::os::fd::OwnedFd, u32), ClientError> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;

    let portal = |e: ashpd::Error| ClientError::ScreenCapture(e.to_string());

    let proxy = Screencast::new().await.map_err(portal)?;
    let session = proxy.create_session(Default::default()).await.map_err(portal)?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .map_err(portal)?;

    let response = proxy.start(&session, None, Default::default()).await.map_err(portal)?;
    let streams = response.response().map_err(|e| ClientError::ScreenCapture(e.to_string()))?;
    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| ClientError::ScreenCapture("you did not pick anything to share".into()))?;
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await.map_err(portal)?;
    std::mem::forget(session);
    Ok((fd, node_id))
}

struct StreamState {
    info: VideoInfoRaw,
    layout: Option<PixelLayout>,
    sender: mpsc::Sender<CapturedFrame>,
    dropped: u64,
}

fn run_pipewire(
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    sender: mpsc::Sender<CapturedFrame>,
    stop: Arc<AtomicBool>,
    framerate: u32,
) -> Result<(), ClientError> {
    let pipewire = |e: pw::Error| ClientError::ScreenCapture(e.to_string());

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(pipewire)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(pipewire)?;
    let core = context.connect_fd_rc(fd, None).map_err(pipewire)?;

    let state = Rc::new(RefCell::new(StreamState {
        info: Default::default(),
        layout: None,
        sender,
        dropped: 0,
    }));

    let stream = pw::stream::StreamRc::new(
        core.clone(),
        "rustcord-screenshare",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(pipewire)?;

    let format_state = state.clone();
    let frame_state = state.clone();
    let mut capture_stats_started = std::time::Instant::now();
    let mut capture_stats_frames = 0u64;
    let mut capture_stats_drops = 0u64;
    let mut capture_stats_total = std::time::Duration::ZERO;
    let mut capture_stats_max = std::time::Duration::ZERO;
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else { return };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }
            let mut state = format_state.borrow_mut();
            if state.info.parse(param).is_err() {
                return;
            }
            state.layout = PixelLayout::from_spa(state.info.format());
            let size = state.info.size();
            if state.layout.is_none() {
                tracing::warn!("screen capture negotiated unsupported format {:?}", state.info.format());
            }
            tracing::info!("screen capture negotiated {}x{}", size.width, size.height);
        })
        .process(move |stream, _| {
            let callback_started = std::time::Instant::now();
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let chunk = datas[0].chunk();
            let (size, stride) = (chunk.size() as usize, chunk.stride() as usize);
            if size == 0 || stride == 0 {
                return;
            }
            let Some(pixels) = datas[0].data() else { return };

            let mut state = frame_state.borrow_mut();
            let Some(layout) = state.layout else { return };
            let info_size = state.info.size();
            let (width, height) = (info_size.width, info_size.height);
            if width == 0 || height == 0 {
                return;
            }
            let Some(packed) = pack_rows(pixels, width, height, stride) else {
                tracing::warn!("screen capture buffer too small for {width}x{height} at stride {stride}");
                return;
            };

            let frame = CapturedFrame {
                width,
                height,
                layout,
                pixels: packed,
                captured_at: std::time::Instant::now(),
            };
            if state.sender.try_send(frame).is_err() {
                state.dropped += 1;
                capture_stats_drops += 1;
                if state.dropped % 60 == 1 {
                    tracing::debug!("screen capture dropped {} frames, consumer is behind", state.dropped);
                }
            }
            let elapsed = callback_started.elapsed();
            capture_stats_frames += 1;
            capture_stats_total += elapsed;
            capture_stats_max = capture_stats_max.max(elapsed);
            let window = capture_stats_started.elapsed();
            if window >= std::time::Duration::from_secs(2) {
                tracing::info!(
                    capture_fps = capture_stats_frames as f32 / window.as_secs_f32(),
                    capture_avg_ms = capture_stats_total.as_secs_f64() * 1000.0 / capture_stats_frames as f64,
                    capture_max_ms = capture_stats_max.as_secs_f64() * 1000.0,
                    capture_queue_drops = capture_stats_drops,
                    width,
                    height,
                    "screen capture callback timing"
                );
                capture_stats_started = std::time::Instant::now();
                capture_stats_frames = 0;
                capture_stats_drops = 0;
                capture_stats_total = std::time::Duration::ZERO;
                capture_stats_max = std::time::Duration::ZERO;
            }
        })
        .register()
        .map_err(pipewire)?;

    let values = format_pod(framerate)?;
    let mut params = [Pod::from_bytes(&values)
        .ok_or_else(|| ClientError::ScreenCapture("could not build the format descriptor".into()))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(pipewire)?;

    let loop_ref = mainloop.clone();
    let _timer = mainloop
        .loop_()
        .add_timer(move |_| {
            if stop.load(Ordering::Relaxed) {
                loop_ref.quit();
            }
        });
    _timer
        .update_timer(
            Some(std::time::Duration::from_millis(100)),
            Some(std::time::Duration::from_millis(100)),
        )
        .into_result()
        .map_err(|e| ClientError::ScreenCapture(format!("could not arm the stop timer: {e}")))?;

    mainloop.run();
    Ok(())
}

pub fn pack_rows(pixels: &[u8], width: u32, height: u32, stride: usize) -> Option<Vec<u8>> {
    let row_bytes = width as usize * 4;
    let height = height as usize;
    if stride < row_bytes || pixels.len() < stride * (height - 1) + row_bytes {
        return None;
    }
    if stride == row_bytes {
        return Some(pixels[..row_bytes * height].to_vec());
    }
    let mut packed = Vec::with_capacity(row_bytes * height);
    for y in 0..height {
        packed.extend_from_slice(&pixels[y * stride..y * stride + row_bytes]);
    }
    Some(packed)
}

fn format_pod(framerate: u32) -> Result<Vec<u8>, ClientError> {
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: vec![
            Property {
                key: spa_sys::SPA_FORMAT_mediaType,
                flags: PropertyFlags::empty(),
                value: Value::Id(spa::utils::Id(spa_sys::SPA_MEDIA_TYPE_video)),
            },
            Property {
                key: spa_sys::SPA_FORMAT_mediaSubtype,
                flags: PropertyFlags::empty(),
                value: Value::Id(spa::utils::Id(spa_sys::SPA_MEDIA_SUBTYPE_raw)),
            },
            Property {
                key: spa_sys::SPA_FORMAT_VIDEO_format,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: spa::utils::Id(spa_sys::SPA_VIDEO_FORMAT_BGRx),
                        alternatives: vec![
                            spa::utils::Id(spa_sys::SPA_VIDEO_FORMAT_BGRx),
                            spa::utils::Id(spa_sys::SPA_VIDEO_FORMAT_RGBx),
                            spa::utils::Id(spa_sys::SPA_VIDEO_FORMAT_BGRA),
                            spa::utils::Id(spa_sys::SPA_VIDEO_FORMAT_RGBA),
                        ],
                    },
                ))),
            },
            Property {
                key: spa_sys::SPA_FORMAT_VIDEO_size,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Rectangle { width: 1920, height: 1080 },
                        min: Rectangle { width: 1, height: 1 },
                        max: Rectangle { width: MAX_CAPTURE_WIDTH, height: MAX_CAPTURE_HEIGHT },
                    },
                ))),
            },
            Property {
                key: spa_sys::SPA_FORMAT_VIDEO_framerate,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Fraction(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Fraction { num: framerate, denom: 1 },
                        min: Fraction { num: 0, denom: 1 },
                        max: Fraction { num: framerate, denom: 1 },
                    },
                ))),
            },
        ],
    };
    let bytes = spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
        .map_err(|e| ClientError::ScreenCapture(format!("could not serialize the format descriptor: {e}")))?
        .0
        .into_inner();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpadded_buffer_is_copied_verbatim() {
        let pixels: Vec<u8> = (0..32).collect();
        let packed = pack_rows(&pixels, 2, 4, 8).expect("fits");
        assert_eq!(packed, pixels);
    }

    #[test]
    fn padding_is_stripped_row_by_row() {
        let mut pixels = Vec::new();
        for row in 0..3u8 {
            pixels.extend_from_slice(&[row; 8]);
            pixels.extend_from_slice(&[0xFF; 4]);
        }
        let packed = pack_rows(&pixels, 2, 3, 12).expect("fits");
        assert_eq!(packed.len(), 24);
        assert!(!packed.contains(&0xFF), "padding leaked into the packed frame: {packed:?}");
        assert_eq!(&packed[0..8], &[0; 8]);
        assert_eq!(&packed[8..16], &[1; 8]);
    }

    #[test]
    fn a_buffer_shorter_than_the_frame_is_rejected() {
        let pixels = vec![0u8; 20];
        assert!(pack_rows(&pixels, 2, 4, 8).is_none());
    }

    #[test]
    fn a_stride_narrower_than_a_row_is_rejected() {
        let pixels = vec![0u8; 64];
        assert!(pack_rows(&pixels, 4, 2, 8).is_none());
    }

    #[test]
    fn the_last_row_need_not_carry_its_padding() {
        let pixels = vec![7u8; 12 * 2 + 8];
        let packed = pack_rows(&pixels, 2, 3, 12).expect("trailing padding is optional");
        assert_eq!(packed.len(), 24);
    }

    #[test]
    fn channel_offsets_match_the_negotiated_layout() {
        assert_eq!(PixelLayout::Bgrx.red_offset(), 2);
        assert_eq!(PixelLayout::Bgrx.blue_offset(), 0);
        assert_eq!(PixelLayout::Rgbx.red_offset(), 0);
        assert_eq!(PixelLayout::Rgbx.blue_offset(), 2);
    }

    #[test]
    fn the_alpha_variants_share_their_colour_order() {
        use spa::param::video::VideoFormat;
        assert_eq!(PixelLayout::from_spa(VideoFormat::BGRA), Some(PixelLayout::Bgrx));
        assert_eq!(PixelLayout::from_spa(VideoFormat::RGBA), Some(PixelLayout::Rgbx));
    }

    #[test]
    fn a_planar_format_is_not_mistaken_for_packed_rgb() {
        use spa::param::video::VideoFormat;
        assert_eq!(PixelLayout::from_spa(VideoFormat::I420), None);
    }
}
