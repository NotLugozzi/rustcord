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
        }
    }
}

impl Theme {
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
        style[WindowBg] = self.background;
        style[ChildBg] = self.panel;
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

    pub fn list_available() -> Vec<PathBuf> {
        let Some(dir) = Self::themes_dir() else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
            .collect()
    }
}
