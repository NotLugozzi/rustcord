use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Theme {
    pub name: String,
    pub background: [f32; 4],
    pub panel: [f32; 4],
    pub text: [f32; 4],
    pub text_muted: [f32; 4],
    pub accent: [f32; 4],
    pub accent_hover: [f32; 4],
    pub border: [f32; 4],
    #[serde(default)]
    pub gradient: Option<[[f32; 4]; 2]>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "discord-dark".into(),
            background: [0.13, 0.14, 0.16, 1.0],
            panel: [0.17, 0.18, 0.20, 1.0],
            text: [0.90, 0.90, 0.92, 1.0],
            text_muted: [0.60, 0.62, 0.66, 1.0],
            accent: [0.35, 0.40, 0.95, 1.0],
            accent_hover: [0.45, 0.50, 1.0, 1.0],
            border: [0.08, 0.09, 0.10, 1.0],
            gradient: None,
        }
    }
}

impl Theme {
    const BUNDLED: [(&'static str, &'static str); 6] = [
        ("discord-dark.toml", include_str!("../../../themes/discord-dark.toml")),
        ("nord.toml", include_str!("../../../themes/nord.toml")),
        ("solarized-light.toml", include_str!("../../../themes/solarized-light.toml")),
        ("aurora-gradient.toml", include_str!("../../../themes/aurora-gradient.toml")),
        ("sunset-gradient.toml", include_str!("../../../themes/sunset-gradient.toml")),
        ("cosmic-gradient.toml", include_str!("../../../themes/cosmic-gradient.toml")),
    ];

    pub fn load_from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn apply(&self, style: &mut imgui::Style) {
        use imgui::StyleColor::*;
        style[WindowBg] = if self.gradient.is_some() { [self.background[0], self.background[1], self.background[2], 0.84] } else { self.background };
        style[ChildBg] = if self.gradient.is_some() { [self.panel[0], self.panel[1], self.panel[2], 0.86] } else { self.panel };
        style[PopupBg] = self.panel;
        style[Text] = self.text;
        style[TextDisabled] = self.text_muted;
        style[Button] = self.accent;
        style[ButtonHovered] = self.accent_hover;
        style[ButtonActive] = self.accent_hover;
        style[Header] = self.accent;
        style[HeaderHovered] = self.accent_hover;
        style[HeaderActive] = self.accent_hover;
        style[Border] = self.border;
        style[FrameBg] = self.panel;
        style[FrameBgHovered] = self.accent;
        style[FrameBgActive] = self.accent_hover;
    }

    pub fn themes_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "rustcord", "rustcord")
            .map(|dirs| dirs.config_dir().join("themes"))
    }

    pub fn install_bundled() {
        let Some(dir) = Self::themes_dir() else { return };
        if let Err(error) = std::fs::create_dir_all(&dir) {
            tracing::warn!("could not create themes directory {}: {error}", dir.display());
            return;
        }
        for (name, contents) in Self::BUNDLED {
            let path = dir.join(name);
            if !path.exists() {
                if let Err(error) = std::fs::write(&path, contents) {
                    tracing::warn!("could not install bundled theme {}: {error}", path.display());
                }
            }
        }
    }

    pub fn list_available() -> Vec<PathBuf> {
        Self::install_bundled();
        let Some(dir) = Self::themes_dir() else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut paths: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        paths.sort();
        paths
    }
}
