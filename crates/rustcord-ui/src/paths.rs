use std::path::PathBuf;
use std::sync::OnceLock;

static SANDBOX: OnceLock<PathBuf> = OnceLock::new();

pub fn sandbox(root: PathBuf) {
    let _ = SANDBOX.set(root);
}

pub fn is_sandboxed() -> bool {
    SANDBOX.get().is_some()
}

pub fn config_dir() -> Option<PathBuf> {
    if let Some(root) = SANDBOX.get() {
        return Some(root.join("config"));
    }
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|dirs| dirs.config_dir().to_path_buf())
}

pub fn cache_dir() -> Option<PathBuf> {
    if let Some(root) = SANDBOX.get() {
        return Some(root.join("cache"));
    }
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|dirs| dirs.cache_dir().to_path_buf())
}
