use super::CapturedFrame;
use crate::error::ClientError;

/// Windows capture backend seam. The intended implementation uses Windows
/// Graphics Capture and publishes packed BGRA frames through this API.
pub struct ScreenCapture;

impl ScreenCapture {
    pub async fn start(_framerate: u32) -> Result<Self, ClientError> {
        Err(ClientError::ScreenCapture(
            "Windows Graphics Capture backend is not implemented yet".into(),
        ))
    }

    pub async fn next_frame(&mut self) -> Option<CapturedFrame> {
        None
    }

    pub(crate) fn blocking_next_frame(&mut self) -> Option<CapturedFrame> {
        None
    }
}
