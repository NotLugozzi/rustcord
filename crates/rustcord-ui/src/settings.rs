use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub theme_file: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub mic_gain: Option<f32>,
    pub denoise: bool,
    pub user_volumes: std::collections::HashMap<String, f32>,
}

fn settings_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|dirs| dirs.config_dir().join("settings.toml"))
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
