use crate::error::ClientError;
use crate::screen_capture::{CapturedFrame, PixelLayout};
use ffmpeg_next as ffmpeg;
use ffmpeg::codec::profile::{H264 as H264Profile, Profile};
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg::util::frame::video::Video as VideoFrame;
pub use crate::screen_decode::VideoCodec;

pub const TIMEBASE_HZ: i64 = 90_000;
const DEFAULT_MAX_HEIGHT: u32 = 1080;
const DEFAULT_BITRATE_BPS: usize = 2_500_000;
const KEYFRAME_INTERVAL_FRAMES: u32 = 120;

#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    pub max_height: u32,
    pub bitrate_bps: usize,
    pub keyframe_interval: u32,
    pub frame_rate: u32,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            max_height: DEFAULT_MAX_HEIGHT,
            bitrate_bps: DEFAULT_BITRATE_BPS,
            keyframe_interval: KEYFRAME_INTERVAL_FRAMES,
            frame_rate: 30,
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
    pub upload: std::time::Duration,
    pub encode: std::time::Duration,
}

#[derive(Debug, Clone)]
pub struct EncoderChoice {
    pub codec: VideoCodec,
    backend: EncoderBackend,
}

impl EncoderChoice {
    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            EncoderBackend::SoftwareVp8 => "libvpx",
            EncoderBackend::Vaapi { .. } => "VAAPI",
            EncoderBackend::Qsv { .. } => "QSV",
            EncoderBackend::Cuda { .. } => "NVENC",
        }
    }
}

#[derive(Debug, Clone)]
enum EncoderBackend {
    SoftwareVp8,
    Vaapi { device: String },
    Qsv { device: String },
    Cuda { device: String },
}

pub enum ScreenEncoder {
    SoftwareVp8(Vp8Encoder),
    Hardware(HardwareEncoder),
}

impl ScreenEncoder {
    pub fn new(
        choice: &EncoderChoice,
        width: u32,
        height: u32,
        layout: PixelLayout,
        settings: EncoderSettings,
    ) -> Result<Self, ClientError> {
        match &choice.backend {
            EncoderBackend::SoftwareVp8 => Ok(Self::SoftwareVp8(Vp8Encoder::new(width, height, layout, settings)?)),
            backend @ (EncoderBackend::Vaapi { .. } | EncoderBackend::Qsv { .. } | EncoderBackend::Cuda { .. }) => Ok(Self::Hardware(HardwareEncoder::new(
                backend, choice.codec, width, height, layout, settings,
            )?)),
        }
    }

    pub fn target_size(&self) -> (u32, u32) {
        match self {
            Self::SoftwareVp8(encoder) => encoder.target_size(),
            Self::Hardware(encoder) => encoder.target_size(),
        }
    }

    pub fn conversion_name(&self) -> &'static str {
        match self {
            Self::SoftwareVp8(_) => "CPU",
            Self::Hardware(encoder) => encoder.conversion_name(),
        }
    }

    pub fn matches_source(&self, width: u32, height: u32, layout: PixelLayout) -> bool {
        match self {
            Self::SoftwareVp8(encoder) => encoder.matches_source(width, height, layout),
            Self::Hardware(encoder) => encoder.source == (width, height, layout),
        }
    }

    pub fn encode(&mut self, frame: &CapturedFrame) -> Result<Vec<EncodedFrame>, ClientError> {
        match self {
            Self::SoftwareVp8(encoder) => encoder.encode(frame),
            Self::Hardware(encoder) => encoder.encode(frame),
        }
    }

    pub fn take_timing_sample(&mut self) -> EncodeTimingSample {
        match self {
            Self::SoftwareVp8(encoder) => encoder.take_timing_sample(),
            Self::Hardware(encoder) => encoder.take_timing_sample(),
        }
    }
}

pub fn software_vp8() -> EncoderChoice {
    EncoderChoice { codec: VideoCodec::Vp8, backend: EncoderBackend::SoftwareVp8 }
}

pub fn encoder_for(codec: VideoCodec, probed: &EncoderChoice) -> Option<EncoderChoice> {
    if probed.codec == codec {
        return Some(probed.clone());
    }
    (codec == VideoCodec::Vp8).then(software_vp8)
}

pub fn probe_encoder() -> EncoderChoice {
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        let mut devices = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("renderD")))
            .collect::<Vec<_>>();
        devices.sort();
        for device in &devices {
            let device = device.to_string_lossy().into_owned();
            if probe_hardware_encoder(&EncoderBackend::Vaapi { device: device.clone() }, VideoCodec::Vp8) {
                tracing::info!(codec = ?VideoCodec::Vp8, %device, "selected VAAPI screen-share encoder");
                return EncoderChoice { codec: VideoCodec::Vp8, backend: EncoderBackend::Vaapi { device } };
            }
        }
        for device in &devices {
            let device = device.to_string_lossy().into_owned();
            if probe_hardware_encoder(&EncoderBackend::Qsv { device: device.clone() }, VideoCodec::H264) {
                tracing::info!(%device, "selected Intel QSV H.264 screen-share encoder");
                return EncoderChoice { codec: VideoCodec::H264, backend: EncoderBackend::Qsv { device } };
            }
        }
        for device in &devices {
            let device = device.to_string_lossy().into_owned();
            if probe_hardware_encoder(&EncoderBackend::Vaapi { device: device.clone() }, VideoCodec::H264) {
                tracing::info!(codec = ?VideoCodec::H264, %device, "selected VAAPI screen-share encoder");
                return EncoderChoice { codec: VideoCodec::H264, backend: EncoderBackend::Vaapi { device } };
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        // A null QSV device asks FFmpeg to select the platform's default Intel
        // media device; Windows has no /dev/dri render-node enumeration.
        let device = String::new();
        if probe_hardware_encoder(&EncoderBackend::Qsv { device: device.clone() }, VideoCodec::H264) {
            tracing::info!("selected Intel QSV H.264 screen-share encoder");
            return EncoderChoice { codec: VideoCodec::H264, backend: EncoderBackend::Qsv { device } };
        }
    }
    for index in 0..4 {
        let device = index.to_string();
        if probe_hardware_encoder(&EncoderBackend::Cuda { device: device.clone() }, VideoCodec::H264) {
            tracing::info!(%device, "selected NVENC H.264 screen-share encoder");
            return EncoderChoice { codec: VideoCodec::H264, backend: EncoderBackend::Cuda { device } };
        }
    }
    tracing::info!("no usable hardware video encoder found; using software VP8");
    EncoderChoice { codec: VideoCodec::Vp8, backend: EncoderBackend::SoftwareVp8 }
}

fn probe_hardware_encoder(backend: &EncoderBackend, codec: VideoCodec) -> bool {
    let width = 1920;
    let height = 1080;
    let Ok(mut encoder) = HardwareEncoder::new(
        backend,
        codec,
        width,
        height,
        PixelLayout::Bgrx,
        EncoderSettings::default(),
    ) else {
        return false;
    };
    let frame = CapturedFrame {
        width,
        height,
        layout: PixelLayout::Bgrx,
        pixels: vec![0; (width * height * 4) as usize],
        captured_at: std::time::Instant::now(),
    };
    encoder.encode(&frame).is_ok()
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
        drain_packets(&mut self.encoder)
    }

    pub fn settings(&self) -> EncoderSettings {
        self.settings
    }

    pub fn take_timing_sample(&mut self) -> EncodeTimingSample {
        std::mem::take(&mut self.timing)
    }
}

fn drain_packets(encoder: &mut ffmpeg::encoder::Video) -> Vec<EncodedFrame> {
    let mut out = Vec::new();
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        let Some(data) = packet.data() else { continue };
        out.push(EncodedFrame {
            data: data.to_vec(),
            keyframe: packet.is_key(),
            timestamp: packet.pts().unwrap_or(0) as u32,
        });
    }
    out
}

pub struct HardwareEncoder {
    encoder: ffmpeg::encoder::Video,
    scaler: Option<scaling::Context>,
    source_frame: VideoFrame,
    converted_frame: Option<VideoFrame>,
    hardware_frame: VideoFrame,
    gpu_filter: Option<HardwareGpuFilter>,
    hardware: HardwareFrames,
    source: (u32, u32, PixelLayout),
    target: (u32, u32),
    frames_sent: i64,
    started: Option<std::time::Instant>,
    timing: EncodeTimingSample,
}

struct HardwareGpuFilter {
    _graph: ffmpeg::filter::Graph,
    source: ffmpeg::filter::Context,
    sink: ffmpeg::filter::Context,
}

impl HardwareGpuFilter {
    fn new(
        backend: &EncoderBackend,
        device: *mut ffmpeg::ffi::AVBufferRef,
        width: u32,
        height: u32,
        target_w: u32,
        target_h: u32,
        layout: PixelLayout,
    ) -> Result<(Self, *mut ffmpeg::ffi::AVBufferRef), ClientError> {
        let (device_filter, processing_filter, processing_args) = match backend {
            EncoderBackend::Vaapi { .. } => (
                "hwupload",
                "scale_vaapi",
                format!("w={target_w}:h={target_h}:format=nv12"),
            ),
            EncoderBackend::Qsv { .. } => (
                "hwupload",
                "vpp_qsv",
                format!("w={target_w}:h={target_h}:format=nv12"),
            ),
            _ => return Err(ClientError::ScreenCapture("this backend has no GPU conversion filter".into())),
        };
        let pixel_format = match layout {
            PixelLayout::Bgrx => ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_BGRA,
            PixelLayout::Rgbx => ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_RGBA,
        };
        let mut graph = ffmpeg::filter::Graph::new();
        let source_args = format!(
            "video_size={width}x{height}:pix_fmt={}:time_base=1/{TIMEBASE_HZ}:pixel_aspect=1/1",
            pixel_format as i32,
        );
        let mut source = graph
            .add(&ffmpeg::filter::find("buffer").ok_or_else(|| ClientError::ScreenCapture("FFmpeg buffer filter is unavailable".into()))?, "capture", &source_args)
            .map_err(|e| ClientError::ScreenCapture(format!("could not create the GPU conversion input: {e}")))?;
        let mut upload = graph
            .add(&ffmpeg::filter::find(device_filter).ok_or_else(|| ClientError::ScreenCapture(format!("FFmpeg {device_filter} filter is unavailable")))?, "upload", "")
            .map_err(|e| ClientError::ScreenCapture(format!("could not create the GPU upload filter: {e}")))?;
        unsafe {
            (*upload.as_mut_ptr()).hw_device_ctx = ffmpeg::ffi::av_buffer_ref(device);
            if (*upload.as_mut_ptr()).hw_device_ctx.is_null() {
                return Err(ClientError::ScreenCapture("could not attach the VAAPI device to the upload filter".into()));
            }
            (*upload.as_mut_ptr()).extra_hw_frames = 16;
        }
        let mut scale = graph
            .add(
                &ffmpeg::filter::find(processing_filter).ok_or_else(|| ClientError::ScreenCapture(format!("FFmpeg {processing_filter} filter is unavailable")))?,
                "scale",
                &processing_args,
            )
            .map_err(|e| ClientError::ScreenCapture(format!("could not create the VAAPI scale filter: {e}")))?;
        let mut sink = graph
            .add(&ffmpeg::filter::find("buffersink").ok_or_else(|| ClientError::ScreenCapture("FFmpeg buffersink filter is unavailable".into()))?, "encoded_input", "")
            .map_err(|e| ClientError::ScreenCapture(format!("could not create the GPU conversion output: {e}")))?;
        source.link(0, &mut upload, 0);
        upload.link(0, &mut scale, 0);
        scale.link(0, &mut sink, 0);
        graph
            .validate()
            .map_err(|e| ClientError::ScreenCapture(format!("VAAPI GPU conversion is unavailable for this capture format: {e}")))?;

        let frames = unsafe {
            let sink_ptr = sink.as_mut_ptr();
            if (*sink_ptr).nb_inputs == 0 || (*sink_ptr).inputs.is_null() {
                std::ptr::null_mut()
            } else {
                ffmpeg::ffi::avfilter_link_get_hw_frames_ctx(*(*sink_ptr).inputs)
            }
        };
        if frames.is_null() {
            return Err(ClientError::ScreenCapture("GPU conversion did not provide hardware frames to the encoder".into()));
        }
        Ok((Self { _graph: graph, source, sink }, frames))
    }

    fn convert(&mut self, frame: &VideoFrame, output: &mut VideoFrame) -> Result<(), ClientError> {
        let input = frame.clone();
        self.source
            .source()
            .add(&input)
            .map_err(|e| ClientError::ScreenCapture(format!("could not submit a frame to VAAPI conversion: {e}")))?;
        self.sink
            .sink()
            .frame(output)
            .map_err(|e| ClientError::ScreenCapture(format!("could not receive a VAAPI converted frame: {e}")))
    }
}

struct HardwareFrames {
    device: *mut ffmpeg::ffi::AVBufferRef,
    frames: *mut ffmpeg::ffi::AVBufferRef,
}

fn allocate_hardware_frames(
    device: *mut ffmpeg::ffi::AVBufferRef,
    hw_pixel: Pixel,
    width: u32,
    height: u32,
) -> Result<*mut ffmpeg::ffi::AVBufferRef, ClientError> {
    let mut frames_ref = unsafe { ffmpeg::ffi::av_hwframe_ctx_alloc(device) };
    if frames_ref.is_null() {
        return Err(ClientError::ScreenCapture("could not allocate a hardware frames context".into()));
    }
    let frames = unsafe { &mut *(*frames_ref).data.cast::<ffmpeg::ffi::AVHWFramesContext>() };
    frames.format = match hw_pixel {
        Pixel::VAAPI => ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_VAAPI,
        Pixel::QSV => ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_QSV,
        Pixel::CUDA => ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_CUDA,
        _ => unreachable!(),
    };
    frames.sw_format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NV12;
    frames.width = width as i32;
    frames.height = height as i32;
    frames.initial_pool_size = 16;
    let result = unsafe { ffmpeg::ffi::av_hwframe_ctx_init(frames_ref) };
    if result < 0 {
        unsafe { ffmpeg::ffi::av_buffer_unref(&mut frames_ref) };
        return Err(ClientError::ScreenCapture(format!("could not initialize hardware frame pool: {}", ffmpeg::Error::from(result))));
    }
    Ok(frames_ref)
}

impl Drop for HardwareFrames {
    fn drop(&mut self) {
        unsafe {
            ffmpeg::ffi::av_buffer_unref(&mut self.frames);
            ffmpeg::ffi::av_buffer_unref(&mut self.device);
        }
    }
}

impl HardwareEncoder {
    fn conversion_name(&self) -> &'static str {
        if self.gpu_filter.is_some() { "GPU VPP" } else { "CPU" }
    }

    fn new(
        backend: &EncoderBackend,
        codec_kind: VideoCodec,
        width: u32,
        height: u32,
        layout: PixelLayout,
        settings: EncoderSettings,
    ) -> Result<Self, ClientError> {
        ffmpeg::init().map_err(|e| ClientError::ScreenCapture(format!("ffmpeg init failed: {e}")))?;
        let (target_w, target_h) = scaled_size(width, height, settings.max_height);
        let (device_type, hw_pixel, codec_name, device_name) = match backend {
            EncoderBackend::Vaapi { device } => (
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                Pixel::VAAPI,
                match codec_kind { VideoCodec::Vp8 => "vp8_vaapi", VideoCodec::H264 => "h264_vaapi" },
                device.as_str(),
            ),
            EncoderBackend::Qsv { device } => (
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_QSV,
                Pixel::QSV,
                "h264_qsv",
                device.as_str(),
            ),
            EncoderBackend::Cuda { device } => (
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA,
                Pixel::CUDA,
                "h264_nvenc",
                device.as_str(),
            ),
            EncoderBackend::SoftwareVp8 => unreachable!("software encoder does not create hardware frames"),
        };
        let device = std::ffi::CString::new(device_name)
            .map_err(|e| ClientError::ScreenCapture(format!("invalid hardware device name: {e}")))?;
        let device_ptr = if device.as_bytes().is_empty() { std::ptr::null() } else { device.as_ptr() };
        let mut device_ref = std::ptr::null_mut();
        let result = unsafe {
            ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut device_ref,
                device_type,
                device_ptr,
                std::ptr::null_mut(),
                0,
            )
        };
        if result < 0 {
            return Err(ClientError::ScreenCapture(format!("could not open VAAPI device: {}", ffmpeg::Error::from(result))));
        }
        let (frames_ref, gpu_filter) = if matches!(backend, EncoderBackend::Vaapi { .. } | EncoderBackend::Qsv { .. }) {
            match HardwareGpuFilter::new(backend, device_ref, width, height, target_w, target_h, layout) {
                Ok((filter, frames)) => {
                    tracing::info!(backend = ?backend, "GPU upload, scaling, and color conversion enabled");
                    (frames, Some(filter))
                }
                Err(error) => {
                    tracing::info!(%error, backend = ?backend, "GPU conversion unavailable; using CPU conversion before hardware encoding");
                    let frames = match allocate_hardware_frames(device_ref, hw_pixel, target_w, target_h) {
                        Ok(frames) => frames,
                        Err(error) => {
                            unsafe { ffmpeg::ffi::av_buffer_unref(&mut device_ref) };
                            return Err(error);
                        }
                    };
                    (frames, None)
                }
            }
        } else {
            let frames = match allocate_hardware_frames(device_ref, hw_pixel, target_w, target_h) {
                Ok(frames) => frames,
                Err(error) => {
                    unsafe { ffmpeg::ffi::av_buffer_unref(&mut device_ref) };
                    return Err(error);
                }
            };
            (frames, None)
        };
        let hardware = HardwareFrames { device: device_ref, frames: frames_ref };

        let codec = ffmpeg::encoder::find_by_name(codec_name)
            .ok_or_else(|| ClientError::ScreenCapture(format!("this ffmpeg build has no {codec_name} encoder")))?;
        let mut context = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(|e| ClientError::ScreenCapture(format!("could not create {codec_name} context: {e}")))?;
        context.set_width(target_w);
        context.set_height(target_h);
        context.set_format(hw_pixel);
        context.set_time_base(ffmpeg::Rational::new(1, TIMEBASE_HZ as i32));
        context.set_frame_rate(Some(ffmpeg::Rational::new(settings.frame_rate as i32, 1)));
        context.set_bit_rate(settings.bitrate_bps);
        context.set_gop(settings.keyframe_interval);
        context.set_max_b_frames(0);
        unsafe {
            let context_ptr = context.as_mut_ptr();
            (*context_ptr).hw_frames_ctx = ffmpeg::ffi::av_buffer_ref(hardware.frames);
            if (*context_ptr).hw_frames_ctx.is_null() {
                return Err(ClientError::ScreenCapture("could not attach VAAPI frames to encoder".into()));
            }
            if codec_kind == VideoCodec::H264 {
                (*context_ptr).profile = Profile::H264(H264Profile::ConstrainedBaseline).into();
            }
        }
        let mut options = ffmpeg::Dictionary::new();
        if codec_kind == VideoCodec::H264 {
            options.set("level", "51");
        }
        let encoder = context
            .open_with(options)
            .map_err(|e| ClientError::ScreenCapture(format!("could not open {codec_name}: {e}")))?;

        let scaler = if gpu_filter.is_none() {
            Some(scaling::Context::get(
                source_pixel(layout),
                width,
                height,
                Pixel::NV12,
                target_w,
                target_h,
                scaling::Flags::FAST_BILINEAR,
            )
            .map_err(|e| ClientError::ScreenCapture(format!("could not build the hardware encoder input scaler: {e}")))?)
        } else {
            None
        };
        let source_frame = VideoFrame::new(source_pixel(layout), width, height);
        let converted_frame = gpu_filter.is_none().then(|| VideoFrame::new(Pixel::NV12, target_w, target_h));

        Ok(Self {
            encoder,
            scaler,
            source_frame,
            converted_frame,
            hardware_frame: VideoFrame::empty(),
            gpu_filter,
            hardware,
            source: (width, height, layout),
            target: (target_w, target_h),
            frames_sent: 0,
            started: None,
            timing: EncodeTimingSample::default(),
        })
    }

    fn target_size(&self) -> (u32, u32) {
        self.target
    }

    fn encode(&mut self, frame: &CapturedFrame) -> Result<Vec<EncodedFrame>, ClientError> {
        if self.source != (frame.width, frame.height, frame.layout) {
            return Err(ClientError::ScreenCapture("the capture format changed; rebuild the VAAPI encoder".into()));
        }
        let started = *self.started.get_or_insert(frame.captured_at);
        let elapsed = frame.captured_at.saturating_duration_since(started);
        let pts = ((elapsed.as_secs_f64() * TIMEBASE_HZ as f64).round() as i64).max(self.frames_sent);

        let stage = std::time::Instant::now();
        make_writable(&mut self.source_frame)?;
        copy_into_plane(&mut self.source_frame, &frame.pixels, frame.stride());
        self.source_frame.set_pts(Some(pts));
        self.timing.copy += stage.elapsed();

        let stage = std::time::Instant::now();
        if let Some(filter) = self.gpu_filter.as_mut() {
            filter.convert(&self.source_frame, &mut self.hardware_frame)?;
        } else {
            let scaler = self.scaler.as_mut().expect("CPU scaler exists when GPU conversion is disabled");
            let converted_frame = self.converted_frame.as_mut().expect("CPU conversion buffer exists");
            make_writable(converted_frame)?;
            scaler
                .run(&self.source_frame, converted_frame)
                .map_err(|e| ClientError::ScreenCapture(format!("hardware encoder input conversion failed: {e}")))?;
        }
        self.timing.convert += stage.elapsed();

        if self.gpu_filter.is_none() {
            let stage = std::time::Instant::now();
            unsafe {
                ffmpeg::ffi::av_frame_unref(self.hardware_frame.as_mut_ptr());
                let result = ffmpeg::ffi::av_hwframe_get_buffer(self.hardware.frames, self.hardware_frame.as_mut_ptr(), 0);
                if result < 0 {
                    return Err(ClientError::ScreenCapture(format!("could not allocate a hardware surface: {}", ffmpeg::Error::from(result))));
                }
                let result = ffmpeg::ffi::av_hwframe_transfer_data(
                    self.hardware_frame.as_mut_ptr(),
                    self.converted_frame.as_ref().expect("CPU conversion buffer exists").as_ptr(),
                    0,
                );
                if result < 0 {
                    return Err(ClientError::ScreenCapture(format!("could not upload a frame to the hardware encoder: {}", ffmpeg::Error::from(result))));
                }
            }
            self.timing.upload += stage.elapsed();
        }
        self.hardware_frame.set_pts(Some(pts));

        let stage = std::time::Instant::now();
        self.encoder
            .send_frame(&self.hardware_frame)
            .map_err(|e| ClientError::ScreenCapture(format!("VAAPI encoder rejected a frame: {e}")))?;
        self.frames_sent = pts + 1;
        let output = drain_packets(&mut self.encoder);
        self.timing.encode += stage.elapsed();
        self.timing.frames += 1;
        Ok(output)
    }

    fn take_timing_sample(&mut self) -> EncodeTimingSample {
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
