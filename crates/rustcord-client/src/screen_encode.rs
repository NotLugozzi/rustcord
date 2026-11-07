use crate::error::ClientError;
use crate::screen_capture::{CapturedFrame, PixelLayout};
use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg::util::frame::video::Video as VideoFrame;

pub const TIMEBASE_HZ: i64 = 90_000;
const DEFAULT_MAX_HEIGHT: u32 = 1080;
const DEFAULT_BITRATE_BPS: usize = 2_500_000;
const KEYFRAME_INTERVAL_FRAMES: u32 = 120;

#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    pub max_height: u32,
    pub bitrate_bps: usize,
    pub keyframe_interval: u32,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            max_height: DEFAULT_MAX_HEIGHT,
            bitrate_bps: DEFAULT_BITRATE_BPS,
            keyframe_interval: KEYFRAME_INTERVAL_FRAMES,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub timestamp: u32,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EncodeTimingSample {
    pub frames: u64,
    pub copy: std::time::Duration,
    pub convert: std::time::Duration,
    pub encode: std::time::Duration,
}

pub fn scaled_size(width: u32, height: u32, max_height: u32) -> (u32, u32) {
    let even = |v: u32| v.max(2) & !1;
    if height <= max_height {
        return (even(width), even(height));
    }
    let scale = max_height as f64 / height as f64;
    (even((width as f64 * scale).round() as u32), even(max_height))
}

fn source_pixel(layout: PixelLayout) -> Pixel {
    match layout {
        PixelLayout::Bgrx => Pixel::BGRA,
        PixelLayout::Rgbx => Pixel::RGBA,
    }
}

pub struct Vp8Encoder {
    encoder: ffmpeg::encoder::Video,
    scaler: scaling::Context,
    source_frame: VideoFrame,
    converted_frame: VideoFrame,
    settings: EncoderSettings,
    source: (u32, u32, PixelLayout),
    target: (u32, u32),
    frames_sent: i64,
    started: Option<std::time::Instant>,
    timing: EncodeTimingSample,
}

impl Vp8Encoder {
    pub fn new(
        width: u32,
        height: u32,
        layout: PixelLayout,
        settings: EncoderSettings,
    ) -> Result<Self, ClientError> {
        ffmpeg::init().map_err(|e| ClientError::ScreenCapture(format!("ffmpeg init failed: {e}")))?;
        let (target_w, target_h) = scaled_size(width, height, settings.max_height);

        let codec = ffmpeg::encoder::find_by_name("libvpx")
            .ok_or_else(|| ClientError::ScreenCapture("this ffmpeg build has no libvpx VP8 encoder".into()))?;

        let mut context = ffmpeg::codec::context::Context::new_with_codec(codec).encoder().video()
            .map_err(|e| ClientError::ScreenCapture(format!("could not open a VP8 encoder: {e}")))?;
        context.set_width(target_w);
        context.set_height(target_h);
        context.set_format(Pixel::YUV420P);
        context.set_time_base(ffmpeg::Rational::new(1, TIMEBASE_HZ as i32));
        context.set_bit_rate(settings.bitrate_bps);
        context.set_gop(settings.keyframe_interval);
        context.set_threading(ffmpeg::threading::Config {
            kind: ffmpeg::threading::Type::Frame,
            count: 0,
        });

        let mut options = ffmpeg::Dictionary::new();
        options.set("deadline", "realtime");
        options.set("cpu-used", "8");
        options.set("error-resilient", "1");
        options.set("lag-in-frames", "0");

        let encoder = context
            .open_with(options)
            .map_err(|e| ClientError::ScreenCapture(format!("could not start the VP8 encoder: {e}")))?;

        let scaler = scaling::Context::get(
            source_pixel(layout),
            width,
            height,
            Pixel::YUV420P,
            target_w,
            target_h,
            scaling::Flags::FAST_BILINEAR,
        )
        .map_err(|e| ClientError::ScreenCapture(format!("could not build the scaler: {e}")))?;
        let source_frame = VideoFrame::new(source_pixel(layout), width, height);
        let converted_frame = VideoFrame::new(Pixel::YUV420P, target_w, target_h);

        Ok(Self {
            encoder,
            scaler,
            source_frame,
            converted_frame,
            settings,
            source: (width, height, layout),
            target: (target_w, target_h),
            frames_sent: 0,
            started: None,
            timing: EncodeTimingSample::default(),
        })
    }

    pub fn target_size(&self) -> (u32, u32) {
        self.target
    }

    pub fn matches_source(&self, width: u32, height: u32, layout: PixelLayout) -> bool {
        self.source == (width, height, layout)
    }

    pub fn encode(&mut self, frame: &CapturedFrame) -> Result<Vec<EncodedFrame>, ClientError> {
        if !self.matches_source(frame.width, frame.height, frame.layout) {
            return Err(ClientError::ScreenCapture(
                "the capture format changed; rebuild the encoder".into(),
            ));
        }

        let started = *self.started.get_or_insert(frame.captured_at);
        let elapsed = frame.captured_at.saturating_duration_since(started);
        let pts = (elapsed.as_secs_f64() * TIMEBASE_HZ as f64).round() as i64;
        let pts = pts.max(self.frames_sent);

        let copy_started = std::time::Instant::now();
        make_writable(&mut self.source_frame)?;
        copy_into_plane(&mut self.source_frame, &frame.pixels, frame.stride());
        make_writable(&mut self.converted_frame)?;
        self.timing.copy += copy_started.elapsed();
        let convert_started = std::time::Instant::now();
        self.scaler
            .run(&self.source_frame, &mut self.converted_frame)
            .map_err(|e| ClientError::ScreenCapture(format!("scaling failed: {e}")))?;
        self.timing.convert += convert_started.elapsed();
        self.converted_frame.set_pts(Some(pts));

        let encode_started = std::time::Instant::now();
        self.encoder
            .send_frame(&self.converted_frame)
            .map_err(|e| ClientError::ScreenCapture(format!("encoder rejected a frame: {e}")))?;
        self.frames_sent = pts + 1;
        let output = self.drain();
        self.timing.encode += encode_started.elapsed();
        self.timing.frames += 1;
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Vec<EncodedFrame>, ClientError> {
        self.encoder
            .send_eof()
            .map_err(|e| ClientError::ScreenCapture(format!("could not flush the encoder: {e}")))?;
        Ok(self.drain())
    }

    fn drain(&mut self) -> Vec<EncodedFrame> {
        let mut out = Vec::new();
        let mut packet = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            let Some(data) = packet.data() else { continue };
            out.push(EncodedFrame {
                data: data.to_vec(),
                keyframe: packet.is_key(),
                timestamp: packet.pts().unwrap_or(0) as u32,
            });
        }
        out
    }

    pub fn settings(&self) -> EncoderSettings {
        self.settings
    }

    pub fn take_timing_sample(&mut self) -> EncodeTimingSample {
        std::mem::take(&mut self.timing)
    }
}

fn make_writable(frame: &mut VideoFrame) -> Result<(), ClientError> {
    let result = unsafe { ffmpeg::ffi::av_frame_make_writable(frame.as_mut_ptr()) };
    if result < 0 {
        return Err(ClientError::ScreenCapture(format!("could not reuse a video frame buffer: {}", ffmpeg::Error::from(result))));
    }
    Ok(())
}

fn copy_into_plane(frame: &mut VideoFrame, pixels: &[u8], src_stride: usize) {
    let height = frame.plane_height(0) as usize;
    let dst_stride = frame.stride(0);
    let row_bytes = src_stride.min(dst_stride);
    let dst = frame.data_mut(0);
    for y in 0..height {
        let (src_start, dst_start) = (y * src_stride, y * dst_stride);
        if src_start + row_bytes > pixels.len() || dst_start + row_bytes > dst.len() {
            break;
        }
        dst[dst_start..dst_start + row_bytes].copy_from_slice(&pixels[src_start..src_start + row_bytes]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_frame(width: u32, height: u32, tick: u8) -> CapturedFrame {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            let x = (i as u32 % width) as u8;
            px[0] = x.wrapping_add(tick);
            px[1] = tick.wrapping_mul(3);
            px[2] = x.wrapping_mul(2);
            px[3] = 255;
        }
        CapturedFrame {
            width,
            height,
            layout: PixelLayout::Bgrx,
            pixels,
            captured_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn a_frame_under_the_cap_keeps_its_size() {
        assert_eq!(scaled_size(1280, 720, 1080), (1280, 720));
    }

    #[test]
    fn an_oversized_frame_scales_by_height_and_keeps_its_aspect() {
        let (w, h) = scaled_size(3440, 1440, 1080);
        assert_eq!(h, 1080);
        let source_aspect = 3440.0 / 1440.0;
        let scaled_aspect = w as f64 / h as f64;
        assert!((source_aspect - scaled_aspect).abs() < 0.01, "{w}x{h}");
    }

    #[test]
    fn scaled_dimensions_are_always_even() {
        for (w, h) in [(3441, 1441), (1919, 1079), (1001, 999)] {
            let (sw, sh) = scaled_size(w, h, 720);
            assert_eq!(sw % 2, 0, "{sw} from {w}");
            assert_eq!(sh % 2, 0, "{sh} from {h}");
        }
    }

    #[test]
    fn a_tiny_frame_never_scales_below_the_minimum() {
        let (w, h) = scaled_size(1, 1, 1080);
        assert!(w >= 2 && h >= 2, "{w}x{h}");
    }

    #[test]
    fn the_encoder_produces_a_keyframe_first() {
        let settings = EncoderSettings { max_height: 240, ..Default::default() };
        let mut encoder = Vp8Encoder::new(320, 240, PixelLayout::Bgrx, settings).expect("encoder");
        let packets = encoder.encode(&synthetic_frame(320, 240, 0)).expect("encode");
        let packets = if packets.is_empty() { encoder.finish().expect("flush") } else { packets };
        assert!(!packets.is_empty(), "the encoder produced nothing at all");
        assert!(packets[0].keyframe, "the first packet must be a keyframe");
        assert!(!packets[0].data.is_empty());
    }

    #[test]
    fn encoded_output_is_far_smaller_than_the_raw_frame() {
        let settings = EncoderSettings { max_height: 240, ..Default::default() };
        let mut encoder = Vp8Encoder::new(320, 240, PixelLayout::Bgrx, settings).expect("encoder");
        let mut total = 0usize;
        for tick in 0..10 {
            total += encoder
                .encode(&synthetic_frame(320, 240, tick))
                .expect("encode")
                .iter()
                .map(|p| p.data.len())
                .sum::<usize>();
        }
        total += encoder.finish().expect("flush").iter().map(|p| p.data.len()).sum::<usize>();
        let raw = 320 * 240 * 4 * 10;
        assert!(total > 0, "no output at all");
        assert!(total < raw / 4, "{total} bytes is not meaningfully smaller than {raw}");
    }

    #[test]
    fn a_source_format_change_is_refused_rather_than_silently_wrong() {
        let mut encoder =
            Vp8Encoder::new(320, 240, PixelLayout::Bgrx, EncoderSettings { max_height: 240, ..Default::default() })
                .expect("encoder");
        let wrong = synthetic_frame(640, 480, 0);
        assert!(encoder.encode(&wrong).is_err());
    }

    #[test]
    fn a_layout_change_is_also_refused() {
        let mut encoder =
            Vp8Encoder::new(320, 240, PixelLayout::Bgrx, EncoderSettings { max_height: 240, ..Default::default() })
                .expect("encoder");
        let mut frame = synthetic_frame(320, 240, 0);
        frame.layout = PixelLayout::Rgbx;
        assert!(encoder.encode(&frame).is_err());
    }

    #[test]
    fn a_wide_source_is_downscaled_before_encoding() {
        let encoder =
            Vp8Encoder::new(3440, 1440, PixelLayout::Bgrx, EncoderSettings { max_height: 720, ..Default::default() })
                .expect("encoder");
        let (w, h) = encoder.target_size();
        assert_eq!(h, 720);
        assert!(w < 3440, "{w}");
    }

    #[test]
    fn the_bitstream_actually_decodes_back_to_video() {
        let settings = EncoderSettings { max_height: 240, ..Default::default() };
        let mut encoder = Vp8Encoder::new(320, 240, PixelLayout::Bgrx, settings).expect("encoder");
        let mut packets = Vec::new();
        for tick in 0..12 {
            packets.extend(encoder.encode(&synthetic_frame(320, 240, tick)).expect("encode"));
        }
        packets.extend(encoder.finish().expect("flush"));
        assert!(!packets.is_empty());

        ffmpeg::init().expect("ffmpeg");
        let codec = ffmpeg::decoder::find(ffmpeg::codec::Id::VP8).expect("a VP8 decoder");
        let mut decoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .decoder()
            .video()
            .expect("open decoder");

        let mut decoded = 0;
        for encoded in &packets {
            let packet = ffmpeg::Packet::copy(&encoded.data);
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            let mut frame = VideoFrame::empty();
            while decoder.receive_frame(&mut frame).is_ok() {
                assert_eq!(frame.width(), 320, "decoded width is wrong");
                assert_eq!(frame.height(), 240, "decoded height is wrong");
                decoded += 1;
            }
        }
        assert!(decoded > 0, "encoded {} packets but none decoded", packets.len());
    }

    #[test]
    fn timestamps_advance_across_frames() {
        let settings = EncoderSettings { max_height: 240, ..Default::default() };
        let mut encoder = Vp8Encoder::new(320, 240, PixelLayout::Bgrx, settings).expect("encoder");
        let mut stamps = Vec::new();
        for tick in 0..6 {
            let mut frame = synthetic_frame(320, 240, tick);
            frame.captured_at = std::time::Instant::now() + std::time::Duration::from_millis(33 * tick as u64);
            for packet in encoder.encode(&frame).expect("encode") {
                stamps.push(packet.timestamp);
            }
        }
        for packet in encoder.finish().expect("flush") {
            stamps.push(packet.timestamp);
        }
        assert!(stamps.len() >= 2, "need several packets to compare, got {}", stamps.len());
        assert!(stamps.windows(2).all(|w| w[1] >= w[0]), "timestamps went backwards: {stamps:?}");
        assert!(stamps.last() > stamps.first(), "timestamps never advanced: {stamps:?}");
    }
}
