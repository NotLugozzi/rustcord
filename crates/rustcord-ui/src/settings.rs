use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub theme_file: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub camera_device: Option<String>,
    pub mic_gain: Option<f32>,
    pub denoise: bool,
    pub user_volumes: std::collections::HashMap<String, f32>,
    pub keybinds: std::collections::HashMap<String, String>,
    pub clock_24h: Option<bool>,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub close_to_tray: Option<bool>,
    pub share_game: Option<bool>,
    pub sound_volume: Option<f32>,
    pub call_sounds: Option<bool>,
    pub stream_sounds: Option<bool>,
    pub message_sounds: Option<bool>,
    pub luna_enabled: Option<bool>,
    pub luna_plugins: Vec<String>,
    pub update_channel: Option<String>,
}

fn settings_path() -> Option<std::path::PathBuf> {
    crate::paths::config_dir().map(|dir| dir.join("settings.toml"))
}

pub fn load() -> Settings {
    settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) {
    let Some(path) = settings_path() else { return };
    let Some(parent) = path.parent() else { return };
    if let Err(error) = std::fs::create_dir_all(parent) {
        tracing::warn!("could not create settings directory {}: {error}", parent.display());
        return;
    }
    match toml::to_string_pretty(settings) {
        Ok(contents) => {
            if let Err(error) = std::fs::write(&path, contents) {
                tracing::warn!("could not save settings {}: {error}", path.display());
            }
        }
        Err(error) => tracing::warn!("could not serialize settings: {error}"),
    }
}
