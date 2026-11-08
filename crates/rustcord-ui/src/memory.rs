#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HeapStats {
    pub in_use: usize,
    pub free_held: usize,
    pub mmapped: usize,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use super::HeapStats;

    const MMAP_THRESHOLD: i32 = 1 << 20;
    const ARENA_MAX: i32 = 4;
    const TRIM_ABOVE: usize = 32 << 20;
    const TRIM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

    pub fn configure() {
        unsafe {
            libc::mallopt(libc::M_MMAP_THRESHOLD, MMAP_THRESHOLD);
            libc::mallopt(libc::M_ARENA_MAX, ARENA_MAX);
        }
        let spawned = std::thread::Builder::new().name("heap-trim".into()).spawn(|| loop {
            std::thread::sleep(TRIM_INTERVAL);
            if stats().is_some_and(|s| s.free_held > TRIM_ABOVE) {
                trim();
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("could not start the heap trim thread: {e}");
        }
    }

    pub fn stats() -> Option<HeapStats> {
        let info = unsafe { libc::mallinfo2() };
        Some(HeapStats { in_use: info.uordblks, free_held: info.fordblks, mmapped: info.hblkhd })
    }

    pub fn trim() {
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub use glibc::{configure, stats, trim};

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn configure() {}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn stats() -> Option<HeapStats> {
    None
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim() {}
