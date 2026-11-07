use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Available app audio output sources as `(pid, display name, currently playing)`.
#[cfg(target_os = "linux")]
pub fn list_sources() -> Result<Vec<(u32, String, bool)>, String> {
    flexaudio_os_linux::list_processes()
        .map(|sources| sources.into_iter().map(|s| (s.pid, s.name, s.is_output_active.unwrap_or(false))).collect())
        .map_err(|error| error.to_string())
}

/// Process-specific capture currently requires PipeWire's per-process stream support.
#[cfg(not(target_os = "linux"))]
pub fn list_sources() -> Result<Vec<(u32, String, bool)>, String> {
    Err("per-app audio capture is currently available on Linux with PipeWire".into())
}

/// Captures 48 kHz mono PCM until screen sharing stops. PID zero captures system
/// output excluding RustCord; a nonzero PID captures only that application's output.
pub fn capture_system_audio(sender: mpsc::Sender<Vec<i16>>, sharing: Arc<AtomicBool>, source_pid: u32) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use flexaudio_core::{raw_ring, CaptureBackend, RawSink};
        use flexaudio_core::types::ProcessMode;
        use flexaudio_os_linux::PwProcessBackend;

        let (producer, mut consumer) = raw_ring(48_000 * 2 * 2);
        let mut backend = if source_pid == 0 {
            PwProcessBackend::new(std::process::id(), ProcessMode::Exclude)
        } else {
            PwProcessBackend::new(source_pid, ProcessMode::Include)
        };
        let sink = RawSink::new(producer, 48_000, 2);
        backend.start(sink).map_err(|error| error.to_string())?;
        tracing::info!(source_pid, "screen audio capture started");

        let mut scratch = vec![0.0f32; 4096];
        let mut pending = VecDeque::with_capacity(4096);
        while sharing.load(Ordering::Relaxed) {
            let read = consumer.pop_slice(&mut scratch);
            if read == 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            }
            pending.extend(scratch[..read].iter().copied());
            while pending.len() >= 1920 {
                let mut mono = Vec::with_capacity(960);
                for _ in 0..960 {
                    let left = pending.pop_front().unwrap_or_default();
                    let right = pending.pop_front().unwrap_or_default();
                    mono.push((((left + right) * 0.5 * i16::MAX as f32)
                        .round()
                        .clamp(i16::MIN as f32, i16::MAX as f32)) as i16);
                }
                let _ = sender.try_send(mono);
            }
        }
        backend.stop();
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (sender, sharing, source_pid);
        Err("per-app audio capture is currently available on Linux with PipeWire".into())
    }
}
