use crate::error::ClientError;
use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg::util::frame::video::Video;

pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Vp8,
    H264,
}

impl VideoCodec {
    pub fn from_gateway(value: &str) -> Self {
        if value.eq_ignore_ascii_case("H264") { Self::H264 } else { Self::Vp8 }
    }

    fn ffmpeg_id(self) -> ffmpeg::codec::Id {
        match self {
            Self::Vp8 => ffmpeg::codec::Id::VP8,
            Self::H264 => ffmpeg::codec::Id::H264,
        }
    }

    fn name(self) -> &'static str {
        match self { Self::Vp8 => "VP8", Self::H264 => "H264" }
    }
}

pub struct VideoDecoder {
    decoder: ffmpeg::decoder::Video,
    codec_kind: VideoCodec,
    scaler: Option<scaling::Context>,
    dimensions: (u32, u32),
}

impl VideoDecoder {
    pub fn new(codec_kind: VideoCodec) -> Result<Self, ClientError> {
        ffmpeg::init().map_err(|e| ClientError::ScreenCapture(format!("ffmpeg init failed: {e}")))?;
        let codec = ffmpeg::codec::decoder::find(codec_kind.ffmpeg_id())
            .ok_or_else(|| ClientError::ScreenCapture(format!("this ffmpeg build has no {} decoder", codec_kind.name())))?;
        let decoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .decoder()
            .video()
            .map_err(|e| ClientError::ScreenCapture(format!("could not open the {} decoder: {e}", codec_kind.name())))?;
        Ok(Self { decoder, codec_kind, scaler: None, dimensions: (0, 0) })
    }

    pub fn decode(&mut self, payload: &[u8]) -> Result<Option<DecodedFrame>, ClientError> {
        let packet_data;
        let payload = if self.codec_kind == VideoCodec::H264
            && !payload.starts_with(&[0, 0, 0, 1]) && !payload.starts_with(&[0, 0, 1])
        {
            packet_data = [vec![0, 0, 0, 1], payload.to_vec()].concat();
            packet_data.as_slice()
        } else {
            payload
        };
        let packet = ffmpeg::Packet::copy(payload);
        self.decoder
            .send_packet(&packet)
            .map_err(|e| ClientError::ScreenCapture(format!("{} packet decode failed: {e}", self.codec_kind.name())))?;
        let mut decoded = Video::empty();
        if self.decoder.receive_frame(&mut decoded).is_err() {
            return Ok(None);
        }
        let dimensions = (decoded.width(), decoded.height());
        if dimensions != self.dimensions {
            self.scaler = Some(
                scaling::Context::get(
                    decoded.format(), dimensions.0, dimensions.1,
                    Pixel::RGBA, dimensions.0, dimensions.1,
                    scaling::Flags::BILINEAR,
                )
                .map_err(|e| ClientError::ScreenCapture(format!("could not build the {} scaler: {e}", self.codec_kind.name())))?,
            );
            self.dimensions = dimensions;
        }
        let mut rgba = Video::empty();
        self.scaler
            .as_mut()
            .expect("scaler initialized")
            .run(&decoded, &mut rgba)
            .map_err(|e| ClientError::ScreenCapture(format!("could not convert decoded {} frame: {e}", self.codec_kind.name())))?;
        let width = dimensions.0 as usize;
        let height = dimensions.1 as usize;
        let stride = rgba.stride(0);
        let plane = rgba.data(0);
        let mut pixels = vec![0; width * height * 4];
        for y in 0..height {
            let src = y * stride;
            let dst = y * width * 4;
            pixels[dst..dst + width * 4].copy_from_slice(&plane[src..src + width * 4]);
        }
        Ok(Some(DecodedFrame { width: dimensions.0, height: dimensions.1, rgba: pixels }))
    }
}
