use super::CapturedFrame;
use crate::error::ClientError;

/// macOS capture backend seam. The intended implementation uses
/// ScreenCaptureKit and normalizes CVPixelBuffer output to packed BGRA frames.
pub struct ScreenCapture;

impl ScreenCapture {
    pub async fn start(_framerate: u32) -> Result<Self, ClientError> {
        Err(ClientError::ScreenCapture(
            "ScreenCaptureKit backend is not implemented yet".into(),
        ))
    }

    pub async fn next_frame(&mut self) -> Option<CapturedFrame> {
        None
    }

    pub(crate) fn blocking_next_frame(&mut self) -> Option<CapturedFrame> {
        None
    }
}
