use super::{CapturedFrame, PixelLayout, MAX_CAPTURE_HEIGHT, MAX_CAPTURE_WIDTH};
use crate::error::ClientError;
use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt, CMTime};
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

fn filter_for(pid: u32) -> Result<(SCContentFilter, SCDisplay), ClientError> {
    let err = |e: &dyn std::fmt::Display| ClientError::ScreenCapture(format!("ScreenCaptureKit: {e}"));
    let content = SCShareableContent::get().map_err(|e| err(&e))?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or_else(|| ClientError::ScreenCapture("no display to capture".into()))?;
    let builder = SCContentFilter::create().with_display(&display);
    let filter = if pid == 0 {
        builder.with_excluding_windows(&[]).build()
    } else {
        let app = content
            .applications()
            .into_iter()
            .find(|app| app.process_id() as u32 == pid)
            .ok_or_else(|| ClientError::ScreenCapture("the selected app is no longer running".into()))?;
        builder.with_including_applications(&[&app], &[]).build()
    }
    .map_err(|e| err(&e))?;
    Ok((filter, display))
}

pub(crate) fn list_apps() -> Result<Vec<(u32, String, bool)>, String> {
    let own = std::process::id();
    let mut apps: Vec<_> = SCShareableContent::get()
        .map_err(|e| e.to_string())?
        .applications()
        .into_iter()
        .map(|app| (app.process_id() as u32, app.application_name()))
        .filter(|(pid, name)| *pid != own && !name.trim().is_empty())
        .map(|(pid, name)| (pid, name, false))
        .collect();
    apps.sort_by_key(|(_, name, _)| name.to_lowercase());
    apps.dedup_by_key(|(pid, _, _)| *pid);
    Ok(apps)
}

pub(crate) fn capture_audio(sender: mpsc::Sender<Vec<i16>>, sharing: Arc<AtomicBool>, pid: u32) -> Result<(), String> {
    let (filter, _) = filter_for(pid).map_err(|e| e.to_string())?;
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_minimum_frame_interval(&CMTime::new(1, 1))
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2)
        .with_excludes_current_process_audio(true);
    let mut stream = SCStream::new(&filter, &config).map_err(|e| e.to_string())?;
    let pending = Mutex::new(Vec::<f32>::with_capacity(4096));
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                if !matches!(kind, SCStreamOutputType::Audio) {
                    return;
                }
                let Ok(list) = sample.audio_buffer_list() else { return };
                let planes: Vec<Vec<f32>> = list
                    .iter()
                    .map(|buffer| (buffer.number_channels.max(1) as usize, buffer.data()))
                    .map(|(channels, bytes)| {
                        let floats: Vec<f32> = bytes
                            .chunks_exact(4)
                            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                        floats.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
                    })
                    .collect();
                let Some(frames) = planes.iter().map(Vec::len).min() else { return };
                let mut pending = pending.lock().unwrap();
                pending.extend((0..frames).map(|i| planes.iter().map(|p| p[i]).sum::<f32>() / planes.len() as f32));
                while pending.len() >= 960 {
                    let mono = pending
                        .drain(..960)
                        .map(|v| (v * i16::MAX as f32).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16)
                        .collect();
                    let _ = sender.try_send(mono);
                }
            },
            SCStreamOutputType::Audio,
        )
        .map_err(|e| e.to_string())?;
    stream.start_capture().map_err(|e| e.to_string())?;
    while sharing.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = stream.stop_capture();
    Ok(())
}

pub struct ScreenCapture {
    frames: mpsc::Receiver<CapturedFrame>,
    stream: SCStream,
}

impl ScreenCapture {
    pub async fn start(framerate: u32) -> Result<Self, ClientError> {
        let err = |e: &dyn std::fmt::Display| ClientError::ScreenCapture(format!("ScreenCaptureKit: {e}"));
        let (filter, display) = filter_for(super::target_app())?;

        let config = SCStreamConfiguration::new()
            .with_width(display.width().min(MAX_CAPTURE_WIDTH))
            .with_height(display.height().min(MAX_CAPTURE_HEIGHT))
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(true)
            .with_queue_depth(3)
            .with_minimum_frame_interval(&CMTime::new(1, framerate.clamp(15, 60) as i32));

        let (tx, frames) = mpsc::channel(2);
        let mut stream = SCStream::new(&filter, &config).map_err(|e| err(&e))?;
        stream
            .add_output_handler(
                move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                    if matches!(kind, SCStreamOutputType::Screen) {
                        if let Some(frame) = to_frame(&sample) {
                            let _ = tx.try_send(frame);
                        }
                    }
                },
                SCStreamOutputType::Screen,
            )
            .map_err(|e| err(&e))?;
        stream.start_capture().map_err(|e| err(&e))?;
        Ok(Self { frames, stream })
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
        let _ = self.stream.stop_capture();
    }
}
fn to_frame(sample: &CMSampleBuffer) -> Option<CapturedFrame> {
    let buffer = sample.pixel_buffer()?;
    let guard = buffer.lock(CVPixelBufferLockFlags::READ_ONLY).ok()?;
    let (width, height, stride) = (guard.width(), guard.height(), guard.bytes_per_row());
    let src = unsafe { guard.as_slice() }?;
    let row = width * 4;
    if width == 0 || height == 0 || stride < row || src.len() < stride * (height - 1) + row {
        return None;
    }
    let mut pixels = Vec::with_capacity(row * height);
    for y in 0..height {
        pixels.extend_from_slice(&src[y * stride..y * stride + row]);
    }
    Some(CapturedFrame {
        width: width as u32,
        height: height as u32,
        layout: PixelLayout::Bgrx,
        pixels,
        captured_at: std::time::Instant::now(),
    })
}
