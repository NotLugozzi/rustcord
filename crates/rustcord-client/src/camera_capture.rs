use crate::error::ClientError;
use crate::screen_capture::{CapturedFrame, PixelLayout};
use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg::util::frame::video::Video;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CameraSettings {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self { width: 1280, height: 720, frame_rate: 30 }
    }
}

#[cfg(target_os = "linux")]
const DEVICE_FORMAT: &str = "video4linux2";
#[cfg(target_os = "windows")]
const DEVICE_FORMAT: &str = "dshow";
#[cfg(target_os = "macos")]
const DEVICE_FORMAT: &str = "avfoundation";
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const DEVICE_FORMAT: &str = "";

#[cfg(target_os = "linux")]
pub fn list_cameras() -> Vec<CameraDevice> {
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else { return Vec::new() };
    let mut cameras: Vec<CameraDevice> = entries
        .flatten()
        .filter_map(|entry| {
            let node = entry.file_name().to_string_lossy().into_owned();
            let read = |file: &str| std::fs::read_to_string(entry.path().join(file)).ok().map(|s| s.trim().to_string());
            if read("index").is_some_and(|index| index != "0") {
                return None;
            }
            let name = read("name").filter(|n| !n.is_empty()).unwrap_or_else(|| node.clone());
            Some(CameraDevice { id: format!("/dev/{node}"), name })
        })
        .collect();
    cameras.sort_by(|a, b| natural_device_order(&a.id, &b.id));
    cameras
}

#[cfg(target_os = "linux")]
fn natural_device_order(a: &str, b: &str) -> std::cmp::Ordering {
    let number = |s: &str| s.trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<u32>().unwrap_or(u32::MAX);
    number(a).cmp(&number(b)).then_with(|| a.cmp(b))
}

#[cfg(not(target_os = "linux"))]
pub fn list_cameras() -> Vec<CameraDevice> {
    if DEVICE_FORMAT.is_empty() || ffmpeg::init().is_err() {
        return Vec::new();
    }
    ffmpeg::device::register_all();
    let Ok(format_name) = std::ffi::CString::new(DEVICE_FORMAT) else { return Vec::new() };
    let mut cameras = Vec::new();
    unsafe {
        let format = ffmpeg::ffi::av_find_input_format(format_name.as_ptr());
        if format.is_null() {
            return cameras;
        }
        let mut list = std::ptr::null_mut();
        if ffmpeg::ffi::avdevice_list_input_sources(format as *mut _, std::ptr::null(), std::ptr::null_mut(), &mut list) < 0 || list.is_null() {
            return cameras;
        }
        for index in 0..(*list).nb_devices.max(0) as usize {
            let info = *(*list).devices.add(index);
            if info.is_null() {
                continue;
            }
            let text = |ptr: *mut std::os::raw::c_char| {
                (!ptr.is_null()).then(|| std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned())
            };
            let device_name = text((*info).device_name).unwrap_or_default();
            let description = text((*info).device_description).unwrap_or_else(|| device_name.clone());
            let id = if cfg!(target_os = "windows") { format!("video={description}") } else { device_name };
            cameras.push(CameraDevice { id, name: description });
        }
        ffmpeg::ffi::avdevice_free_list_devices(&mut list);
    }
    cameras
}

fn default_device() -> Option<String> {
    if cfg!(target_os = "macos") {
        return Some("0:none".into());
    }
    list_cameras().into_iter().next().map(|camera| camera.id)
}

fn open_attempts(settings: CameraSettings) -> Vec<Vec<(&'static str, String)>> {
    let size = ("video_size", format!("{}x{}", settings.width, settings.height));
    let rate = ("framerate", settings.frame_rate.to_string());
    let mut attempts = Vec::new();
    if cfg!(target_os = "linux") {
        attempts.push(vec![("input_format", "mjpeg".to_string()), size.clone(), rate.clone()]);
    }
    attempts.push(vec![size, rate.clone()]);
    attempts.push(vec![rate]);
    attempts.push(Vec::new());
    attempts
}

pub struct CameraCapture {
    input: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::decoder::Video,
    scaler: Option<(scaling::Context, Pixel, u32, u32)>,
    pending: VecDeque<CapturedFrame>,
}

impl CameraCapture {
    pub fn open(device: Option<&str>, settings: CameraSettings) -> Result<Self, ClientError> {
        let device = device
            .map(str::to_string)
            .or_else(default_device)
            .ok_or_else(|| ClientError::ScreenCapture("no camera was found".into()))?;
        let mut last_error = None;
        for options in open_attempts(settings) {
            match Self::open_input(DEVICE_FORMAT, &device, &options) {
                Ok(capture) => {
                    tracing::info!(%device, ?options, "camera opened");
                    return Ok(capture);
                }
                Err(error) => {
                    tracing::debug!(%device, ?options, "camera open attempt failed: {error}");
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| ClientError::ScreenCapture(format!("could not open camera {device}"))))
    }

    pub(crate) fn open_input(format_name: &str, url: &str, options: &[(&str, String)]) -> Result<Self, ClientError> {
        let fail = |what: &str, error: ffmpeg::Error| ClientError::ScreenCapture(format!("camera: {what}: {error}"));
        ffmpeg::init().map_err(|e| fail("ffmpeg init failed", e))?;
        ffmpeg::device::register_all();
        let name = std::ffi::CString::new(format_name)
            .map_err(|_| ClientError::ScreenCapture(format!("camera: bad device format {format_name}")))?;
        let format = unsafe { ffmpeg::ffi::av_find_input_format(name.as_ptr()) };
        if format.is_null() {
            return Err(ClientError::ScreenCapture(format!("this ffmpeg build has no {format_name} input device")));
        }
        let mut dictionary = ffmpeg::Dictionary::new();
        for (key, value) in options {
            dictionary.set(key, value);
        }
        let format = ffmpeg::Format::Input(unsafe { ffmpeg::format::Input::wrap(format as *mut _) });
        let input = match ffmpeg::format::open_with(url, &format, dictionary).map_err(|e| fail(&format!("could not open {url}"), e))? {
            ffmpeg::format::context::Context::Input(input) => input,
            ffmpeg::format::context::Context::Output(_) => return Err(ClientError::ScreenCapture("camera: opened as an output".into())),
        };
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| ClientError::ScreenCapture(format!("camera: {url} has no video stream")))?;
        let stream_index = stream.index();
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| fail("bad stream parameters", e))?
            .decoder()
            .video()
            .map_err(|e| fail("could not open the camera decoder", e))?;
        Ok(Self { input, stream_index, decoder, scaler: None, pending: VecDeque::new() })
    }

    pub fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ClientError> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(Some(frame));
            }
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other { errno }) if errno == libc_eagain() => continue,
                Err(error) => return Err(ClientError::ScreenCapture(format!("camera read failed: {error}"))),
            }
            if packet.stream() != self.stream_index {
                continue;
            }
            if let Err(error) = self.decoder.send_packet(&packet) {
                tracing::debug!("camera: dropping undecodable packet: {error}");
                continue;
            }
            let mut decoded = Video::empty();
            while self.decoder.receive_frame(&mut decoded).is_ok() {
                let frame = self.convert(&decoded)?;
                self.pending.push_back(frame);
            }
        }
    }

    fn convert(&mut self, decoded: &Video) -> Result<CapturedFrame, ClientError> {
        let (format, width, height) = (decoded.format(), decoded.width(), decoded.height());
        if self.scaler.as_ref().is_none_or(|(_, f, w, h)| (*f, *w, *h) != (format, width, height)) {
            let context = scaling::Context::get(format, width, height, Pixel::BGRA, width, height, scaling::Flags::BILINEAR)
                .map_err(|e| ClientError::ScreenCapture(format!("camera: could not build the scaler: {e}")))?;
            self.scaler = Some((context, format, width, height));
        }
        let mut bgra = Video::empty();
        let (scaler, ..) = self.scaler.as_mut().expect("scaler initialized");
        scaler
            .run(decoded, &mut bgra)
            .map_err(|e| ClientError::ScreenCapture(format!("camera: could not convert a frame: {e}")))?;
        let row = width as usize * 4;
        let stride = bgra.stride(0);
        let plane = bgra.data(0);
        let mut pixels = vec![0; row * height as usize];
        for (y, dst) in pixels.chunks_exact_mut(row).enumerate() {
            dst.copy_from_slice(&plane[y * stride..y * stride + row]);
        }
        Ok(CapturedFrame { width, height, layout: PixelLayout::Bgrx, pixels, captured_at: std::time::Instant::now() })
    }
}

fn libc_eagain() -> i32 {
    if cfg!(target_os = "macos") { 35 } else { 11 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_source_device_yields_packed_bgrx_frames() {
        let options = [("framerate", "30".to_string())];
        let mut capture = CameraCapture::open_input("lavfi", "testsrc=size=320x240:rate=30", &options).expect("lavfi test source opens");
        let frame = capture.next_frame().expect("frame reads").expect("a frame arrives");
        assert_eq!((frame.width, frame.height, frame.layout), (320, 240, PixelLayout::Bgrx));
        assert_eq!(frame.pixels.len(), 320 * 240 * 4);
        assert!(frame.pixels.chunks_exact(4).any(|px| px[..3] != [0, 0, 0]), "the test pattern is not blank");
    }

    #[test]
    fn camera_frames_feed_the_screen_encoder_and_decode_back() {
        let options = [("framerate", "30".to_string())];
        let mut capture = CameraCapture::open_input("lavfi", "testsrc=size=640x360:rate=30", &options).expect("lavfi test source opens");
        let choice = crate::screen_encode::software_vp8();
        let settings = crate::screen_encode::EncoderSettings { max_height: 360, bitrate_bps: 800_000, keyframe_interval: 30, frame_rate: 30 };
        let mut encoder = None;
        let mut decoder = crate::screen_decode::VideoDecoder::new(crate::screen_decode::VideoCodec::Vp8).expect("decoder");
        let mut decoded = 0;
        for _ in 0..10 {
            let frame = capture.next_frame().expect("frame reads").expect("a frame arrives");
            let encoder = encoder.get_or_insert_with(|| {
                crate::screen_encode::ScreenEncoder::new(&choice, frame.width, frame.height, frame.layout, settings).expect("encoder")
            });
            for packet in encoder.encode(&frame).expect("encodes") {
                if let Some(out) = decoder.decode(&packet.data).expect("decodes") {
                    assert_eq!((out.width, out.height), (640, 360));
                    decoded += 1;
                }
            }
        }
        assert!(decoded > 0, "no frame survived the round trip");
    }

    #[test]
    fn opening_a_missing_device_reports_an_error_instead_of_panicking() {
        assert!(CameraCapture::open(Some("/dev/rustcord-no-such-camera"), CameraSettings::default()).is_err());
    }

    #[test]
    fn mjpeg_is_tried_first_on_linux_and_a_bare_open_is_the_last_resort() {
        let attempts = open_attempts(CameraSettings::default());
        assert!(attempts.last().is_some_and(Vec::is_empty));
        if cfg!(target_os = "linux") {
            assert_eq!(attempts[0][0], ("input_format", "mjpeg".to_string()));
        }
        assert!(attempts.iter().any(|a| a.contains(&("video_size", "1280x720".to_string()))));
    }
}
