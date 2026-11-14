#[cfg(not(target_os = "linux"))]
use crate::error::ClientError;

static TARGET_APP_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_target_app(pid: Option<u32>) {
    TARGET_APP_PID.store(pid.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
}

#[cfg(target_os = "macos")]
fn target_app() -> u32 {
    TARGET_APP_PID.load(std::sync::atomic::Ordering::Relaxed)
}

pub const MAX_CAPTURE_WIDTH: u32 = 8192;
pub const MAX_CAPTURE_HEIGHT: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelLayout {
    Bgrx,
    Rgbx,
}

impl PixelLayout {
    pub fn red_offset(self) -> usize {
        match self {
            Self::Bgrx => 2,
            Self::Rgbx => 0,
        }
    }

    pub fn blue_offset(self) -> usize {
        match self {
            Self::Bgrx => 0,
            Self::Rgbx => 2,
        }
    }
}

pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
    pub pixels: Vec<u8>,
    pub captured_at: std::time::Instant,
}

impl CapturedFrame {
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::ScreenCapture;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::ScreenCapture;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::ScreenCapture;
#[cfg(target_os = "macos")]
pub(crate) use macos::{capture_audio, list_apps};

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub struct ScreenCapture;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
impl ScreenCapture {
    pub async fn start(_framerate: u32) -> Result<Self, ClientError> {
        Err(ClientError::ScreenCapture("screen capture is not supported on this platform".into()))
    }

    pub async fn next_frame(&mut self) -> Option<CapturedFrame> {
        None
    }

    pub(crate) fn blocking_next_frame(&mut self) -> Option<CapturedFrame> {
        None
    }
}
