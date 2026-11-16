use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Theme {
    pub name: String,
    pub background: [f32; 4],
    pub panel: [f32; 4],
    pub text: [f32; 4],
    #[serde(default)]
    pub servers_bg: Option<[f32; 4]>,
    #[serde(default)]
    pub user_bar_bg: Option<[f32; 4]>,
    pub text_muted: [f32; 4],
    pub accent: [f32; 4],
    pub accent_hover: [f32; 4],
    pub border: [f32; 4],
    #[serde(default)]
    pub mention_badge: Option<[f32; 4]>,
    #[serde(default, skip_serializing)]
    pub gradient: Option<[[f32; 4]; 2]>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_global_gradient: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_gradient_start: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_gradient_end: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_gradient_angle: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_gradient_ratio: Option<GradientRatio>,
    #[serde(default)]
    pub call_active: Option<[f32; 4]>,
    #[serde(default)]
    pub call_missed: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fonts: Option<ThemeFonts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub danger: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_faint: Option<[f32; 4]>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ThemeFonts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regular: Option<FontSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<FontSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<FontSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold_italic: Option<FontSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<FontSpec>,
}

impl ThemeFonts {
    pub fn specs(&self) -> impl Iterator<Item = &FontSpec> {
        [&self.regular, &self.bold, &self.italic, &self.bold_italic, &self.mono].into_iter().flatten()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct FontSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub const DEFAULT_GRADIENT_ANGLE: f32 = 90.0;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum GradientRatio {
    Midpoint(f32),
    Stops([f32; 2]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    pub start: [f32; 4],
    pub end: [f32; 4],
    pub angle: f32,
    pub ratio: Option<GradientRatio>,
}

impl Gradient {
    pub fn is_linear(&self) -> bool {
        match self.ratio {
            None => true,
            Some(GradientRatio::Midpoint(m)) => (m - 0.5).abs() < 1e-4,
            Some(GradientRatio::Stops([a, b])) => a <= 0.0 && b >= 1.0,
        }
    }

    pub fn blend(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self.ratio {
            None => t,
            Some(GradientRatio::Midpoint(m)) => {
                let m = m.clamp(0.01, 0.99);
                if t < m { 0.5 * t / m } else { 0.5 + 0.5 * (t - m) / (1.0 - m) }
            }
            Some(GradientRatio::Stops([a, b])) => {
                let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
                if b <= a { if t < a { 0.0 } else { 1.0 } } else { ((t - a) / (b - a)).clamp(0.0, 1.0) }
            }
        }
    }

    pub fn axis_position(&self, point: [f32; 2], size: [f32; 2]) -> f32 {
        let radians = self.angle.to_radians();
        let direction = [radians.sin(), -radians.cos()];
        let length = (size[0] * direction[0]).abs() + (size[1] * direction[1]).abs();
        if length <= f32::EPSILON {
            return 0.0;
        }
        let offset = [point[0] - size[0] / 2.0, point[1] - size[1] / 2.0];
        0.5 + (offset[0] * direction[0] + offset[1] * direction[1]) / length
    }

    pub fn color_at(&self, point: [f32; 2], size: [f32; 2]) -> [f32; 4] {
        let k = self.blend(self.axis_position(point, size));
        std::array::from_fn(|i| self.start[i] + (self.end[i] - self.start[i]) * k)
    }

    pub fn draw(&self, draw_list: &imgui::DrawListMut<'_>, min: [f32; 2], max: [f32; 2]) {
        let size = [max[0] - min[0], max[1] - min[1]];
        let steps = if self.is_linear() { 1 } else { 24 };
        let at = |i: usize, j: usize| [size[0] * i as f32 / steps as f32, size[1] * j as f32 / steps as f32];
        for j in 0..steps {
            for i in 0..steps {
                let (a, b) = (at(i, j), at(i + 1, j + 1));
                draw_list.add_rect_filled_multicolor(
                    [min[0] + a[0], min[1] + a[1]],
                    [min[0] + b[0], min[1] + b[1]],
                    self.color_at(a, size),
                    self.color_at([b[0], a[1]], size),
                    self.color_at(b, size),
                    self.color_at([a[0], b[1]], size),
                );
            }
        }
    }
}

fn darken(color: [f32; 4], factor: f32) -> [f32; 4] {
    [color[0] * factor, color[1] * factor, color[2] * factor, color[3]]
}

fn blend(base: [f32; 4], over: [f32; 4], amount: f32) -> [f32; 4] {
    std::array::from_fn(|i| if i == 3 { base[3] } else { base[i] + (over[i] - base[i]) * amount })
}

fn with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], alpha]
}

fn luma(color: [f32; 4]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
const BUNDLED_MANIFEST: &str = ".bundled";

fn content_hash(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

fn parse_manifest(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines().filter_map(|line| line.split_once(' ')).map(|(name, hash)| (name.to_string(), hash.trim().to_string())).collect()
}

fn bundled_copy_is_pristine(name: &str, current: Option<&str>, recorded_hash: Option<&str>) -> bool {
    let Some(current) = current else { return true };
    let untouched = recorded_hash.is_some_and(|hash| hash == content_hash(current));
    let retired_default = name == "discord-dark.toml" && current.trim() == RETIRED_DISCORD_DARK;
    untouched || retired_default
}
const RETIRED_DISCORD_DARK: &str = "name = \"discord-dark\"\nbackground = [0.13, 0.14, 0.16, 1.0]\npanel = [0.17, 0.18, 0.20, 1.0]\ntext = [0.90, 0.90, 0.92, 1.0]\ntext_muted = [0.60, 0.62, 0.66, 1.0]\naccent = [0.35, 0.40, 0.95, 1.0]\naccent_hover = [0.45, 0.50, 1.0, 1.0]\nborder = [0.08, 0.09, 0.10, 1.0]";

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "discord-dark".into(),
            background: [0.192, 0.200, 0.220, 1.0],
            panel: [0.169, 0.176, 0.192, 1.0],
            servers_bg: Some([0.118, 0.122, 0.133, 1.0]),
            user_bar_bg: Some([0.137, 0.141, 0.157, 1.0]),
            text: [0.859, 0.871, 0.882, 1.0],
            text_muted: [0.580, 0.608, 0.643, 1.0],
            accent: [0.345, 0.396, 0.949, 1.0],
            accent_hover: [0.278, 0.322, 0.816, 1.0],
            border: [0.118, 0.122, 0.133, 1.0],
            mention_badge: None,
            gradient: None,
            use_global_gradient: false,
            global_gradient_start: None,
            global_gradient_end: None,
            global_gradient_angle: None,
            global_gradient_ratio: None,
            call_active: None,
            call_missed: None,
            fonts: None,
            field: Some([0.220, 0.227, 0.251, 1.0]),
            floating: Some([0.067, 0.071, 0.078, 1.0]),
            line: Some([0.247, 0.255, 0.278, 1.0]),
            link: None,
            danger: None,
            text_faint: None,
        }
    }
}

impl Theme {
    pub fn global_gradient(&self) -> Option<Gradient> {
        self.use_global_gradient.then(|| Gradient {
            start: self.global_gradient_start.unwrap_or(self.background),
            end: self.global_gradient_end.unwrap_or(self.panel),
            angle: self.global_gradient_angle.unwrap_or(DEFAULT_GRADIENT_ANGLE),
            ratio: self.global_gradient_ratio,
        })
    }

    pub fn migrate_legacy_gradient(&mut self) {
        if let Some([start, end]) = self.gradient.take() {
            if self.global_gradient_start.is_none() && self.global_gradient_end.is_none() {
                self.use_global_gradient = true;
                self.global_gradient_start = Some(start);
                self.global_gradient_end = Some(end);
            }
        }
    }

    pub fn mention_badge(&self) -> [f32; 4] {
        self.mention_badge.unwrap_or([0.925, 0.259, 0.278, 1.0])
    }

    pub fn call_active(&self) -> [f32; 4] {
        self.call_active.unwrap_or([0.14, 0.65, 0.37, 1.0])
    }

    pub fn call_missed(&self) -> [f32; 4] {
        self.call_missed.unwrap_or(self.text_muted)
    }

    pub fn servers_bg(&self) -> [f32; 4] {
        self.servers_bg.unwrap_or_else(|| darken(self.background, 0.8))
    }

    pub fn user_bar_bg(&self) -> [f32; 4] {
        self.user_bar_bg.unwrap_or_else(|| darken(self.panel, 0.92))
    }

    pub fn is_light(&self) -> bool {
        luma(self.background) > 0.5
    }

    pub fn field(&self) -> [f32; 4] {
        self.field.unwrap_or_else(|| blend(self.background, self.text, 0.05))
    }

    pub fn floating(&self) -> [f32; 4] {
        self.floating.unwrap_or_else(|| if self.is_light() { blend(self.panel, [1.0; 4], 0.6) } else { darken(self.panel, 0.45) })
    }

    pub fn line(&self) -> [f32; 4] {
        self.line.unwrap_or_else(|| blend(self.panel, self.text, 0.10))
    }

    pub fn link(&self) -> [f32; 4] {
        self.link.unwrap_or([0.0, 0.659, 0.988, 1.0])
    }

    pub fn danger(&self) -> [f32; 4] {
        self.danger.unwrap_or_else(|| self.mention_badge())
    }

    pub fn text_faint(&self) -> [f32; 4] {
        self.text_faint.unwrap_or_else(|| blend(self.text_muted, self.background, 0.35))
    }

    pub fn overlay(&self, alpha: f32) -> [f32; 4] {
        with_alpha(self.text, alpha)
    }

    pub fn hover(&self) -> [f32; 4] {
        self.overlay(0.06)
    }

    pub fn selected(&self) -> [f32; 4] {
        self.overlay(0.12)
    }

    pub fn style_color(&self, slot: imgui::StyleColor) -> [f32; 4] {
        use imgui::StyleColor::*;
        let solid_field = |amount: f32| blend(self.field(), self.text, amount);
        match slot {
            Text => self.text,
            TextDisabled => self.text_muted,
            WindowBg => if self.use_global_gradient { with_alpha(self.background, 0.84) } else { self.background },
            ChildBg => if self.use_global_gradient { with_alpha(self.panel, 0.86) } else { self.panel },
            PopupBg => self.floating(),
            Border => self.border,
            BorderShadow => CLEAR,
            FrameBg => self.field(),
            FrameBgHovered => solid_field(0.04),
            FrameBgActive => solid_field(0.07),
            TitleBg | TitleBgActive | TitleBgCollapsed | MenuBarBg => self.panel,
            ScrollbarBg => CLEAR,
            ScrollbarGrab => self.overlay(0.12),
            ScrollbarGrabHovered => self.overlay(0.20),
            ScrollbarGrabActive => self.overlay(0.28),
            CheckMark | SliderGrab | PlotLines | PlotHistogram | DragDropTarget | NavHighlight => self.accent,
            SliderGrabActive | PlotLinesHovered | PlotHistogramHovered => self.accent_hover,
            Button => self.accent,
            ButtonHovered => self.accent_hover,
            ButtonActive => darken(self.accent_hover, 0.9),
            Header | TabActive => self.selected(),
            HeaderHovered | TabHovered => self.hover(),
            HeaderActive => self.overlay(0.16),
            Separator | TableBorderStrong | TableBorderLight => self.line(),
            SeparatorHovered | SeparatorActive => self.accent,
            ResizeGrip | ResizeGripHovered | ResizeGripActive | Tab | TabUnfocused | TableRowBg => CLEAR,
            TabUnfocusedActive => self.overlay(0.08),
            TableHeaderBg => blend(self.panel, self.text, 0.04),
            TableRowBgAlt => self.overlay(0.02),
            TextSelectedBg => with_alpha(self.accent, 0.35),
            NavWindowingHighlight => self.overlay(0.7),
            NavWindowingDimBg | ModalWindowDimBg => [0.0, 0.0, 0.0, 0.6],
            _ => self.panel,
        }
    }

    const BUNDLED: [(&'static str, &'static str); 9] = [
        ("discord-dark.toml", include_str!("../../../themes/discord-dark.toml")),
        ("nord.toml", include_str!("../../../themes/nord.toml")),
        ("solarized-light.toml", include_str!("../../../themes/solarized-light.toml")),
        ("aurora-gradient.toml", include_str!("../../../themes/aurora-gradient.toml")),
        ("sunset-gradient.toml", include_str!("../../../themes/sunset-gradient.toml")),
        ("cosmic-gradient.toml", include_str!("../../../themes/cosmic-gradient.toml")),
        ("cyberpunk.toml", include_str!("../../../themes/cyberpunk.toml")),
        ("gruvbox-dark.toml", include_str!("../../../themes/gruvbox-dark.toml")),
        ("gruvbox-light.toml", include_str!("../../../themes/gruvbox-light.toml")),
    ];

    pub fn load_from_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        let mut theme: Self = toml::from_str(&text)?;
        theme.migrate_legacy_gradient();
        Ok(theme)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn apply(&self, style: &mut imgui::Style) {
        for slot in imgui::StyleColor::VARIANTS {
            style[slot] = self.style_color(slot);
        }

        style.window_border_size = 0.0;
        style.child_border_size = 0.0;
        style.popup_border_size = 1.0;
        style.frame_border_size = 0.0;

        style.frame_rounding = 4.0;
        style.popup_rounding = 8.0;
        style.child_rounding = 4.0;
        style.scrollbar_rounding = 8.0;
        style.grab_rounding = 4.0;
        style.tab_rounding = 4.0;

        style.window_padding = [12.0, 12.0];
        style.frame_padding = [9.0, 6.0];
        style.item_spacing = [8.0, 6.0];
        style.scrollbar_size = 8.0;
        style.selectable_text_align = [0.0, 0.5];
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
        let manifest_path = dir.join(BUNDLED_MANIFEST);
        let mut manifest = parse_manifest(&std::fs::read_to_string(&manifest_path).unwrap_or_default());
        for (name, contents) in Self::BUNDLED {
            let path = dir.join(name);
            let current = std::fs::read_to_string(&path).ok();
            if current.as_deref() == Some(contents) {
                manifest.insert(name.to_string(), content_hash(contents));
                continue;
            }
            if !bundled_copy_is_pristine(name, current.as_deref(), manifest.get(name).map(String::as_str)) {
                continue;
            }
            match std::fs::write(&path, contents) {
                Ok(()) => {
                    manifest.insert(name.to_string(), content_hash(contents));
                }
                Err(error) => tracing::warn!("could not install bundled theme {}: {error}", path.display()),
            }
        }
        let text: String = manifest.iter().map(|(name, hash)| format!("{name} {hash}\n")).collect();
        if let Err(error) = std::fs::write(&manifest_path, text) {
            tracing::warn!("could not record bundled themes: {error}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_palette_layers_rail_sidebar_chat_from_dark_to_light() {
        let (_, text) = Theme::BUNDLED.iter().find(|(name, _)| *name == "discord-dark.toml").expect("bundled");
        for theme in [toml::from_str::<Theme>(text).unwrap(), Theme::default()] {
            assert!(luma(theme.servers_bg()) < luma(theme.panel), "{}", theme.name);
            assert!(luma(theme.panel) < luma(theme.background), "{}", theme.name);
            assert!(luma(theme.floating()) < luma(theme.servers_bg()));
        }
    }

    #[test]
    fn bundled_themes_update_only_when_the_user_has_not_edited_them() {
        let old = "name = \"nord\"";
        let recorded = content_hash(old);
        assert!(bundled_copy_is_pristine("nord.toml", None, None));
        assert!(bundled_copy_is_pristine("nord.toml", Some(old), Some(&recorded)));
        assert!(!bundled_copy_is_pristine("nord.toml", Some("name = \"mine\""), Some(&recorded)));
        assert!(!bundled_copy_is_pristine("nord.toml", Some(old), None));
        assert!(bundled_copy_is_pristine("discord-dark.toml", Some(RETIRED_DISCORD_DARK), None));
        let manifest = parse_manifest(&format!("nord.toml {recorded}\ncyberpunk.toml abc\n"));
        assert_eq!(manifest.get("nord.toml"), Some(&recorded));
        assert_eq!(content_hash("x"), content_hash("x"));
        assert_ne!(content_hash("x"), content_hash("y"));
    }

    #[test]
    fn every_bundled_theme_sets_the_new_roles() {
        for (name, text) in Theme::BUNDLED {
            let theme: Theme = toml::from_str(text).unwrap();
            assert!(theme.field.is_some() && theme.floating.is_some() && theme.line.is_some(), "{name}");
        }
    }

    #[test]
    fn solid_themes_layer_rail_sidebar_chat_by_lightness() {
        for (name, text) in Theme::BUNDLED.iter().filter(|(name, _)| !name.contains("gradient") && *name != "cyberpunk.toml") {
            let theme: Theme = toml::from_str(text).unwrap();
            assert!(luma(theme.servers_bg()) < luma(theme.panel) && luma(theme.panel) < luma(theme.background), "{name}");
        }
    }

    #[test]
    fn the_retired_default_is_recognised_for_upgrade() {
        let retired: Theme = toml::from_str(RETIRED_DISCORD_DARK).unwrap();
        assert!(luma(retired.background) < luma(retired.panel));
    }

    #[test]
    fn hovers_are_neutral_and_every_slot_is_themed() {
        let theme = Theme::default();
        for slot in [imgui::StyleColor::Header, imgui::StyleColor::HeaderHovered, imgui::StyleColor::FrameBgHovered, imgui::StyleColor::TabHovered] {
            let color = theme.style_color(slot);
            assert!((color[0] - color[2]).abs() < 0.05, "{slot:?} is tinted: {color:?}");
        }
        assert_eq!(theme.style_color(imgui::StyleColor::PlotHistogram), theme.accent);
        assert_eq!(theme.style_color(imgui::StyleColor::ScrollbarBg)[3], 0.0);
    }

    #[test]
    fn themes_without_the_new_tokens_still_derive_them() {
        for (name, text) in Theme::BUNDLED {
            let theme: Theme = toml::from_str(text).unwrap();
            let light = theme.is_light();
            let floating = theme.floating();
            assert_eq!(light, luma(floating) > 0.5, "{name}: popups should match the theme's lightness");
        }
    }

    #[test]
    fn every_bundled_theme_parses() {
        for (name, text) in Theme::BUNDLED {
            assert!(toml::from_str::<Theme>(text).is_ok(), "{name} does not parse");
        }
    }

    #[test]
    fn the_cyberpunk_theme_asks_for_rajdhani() {
        let (_, text) = Theme::BUNDLED.iter().find(|(name, _)| *name == "cyberpunk.toml").expect("bundled");
        let theme: Theme = toml::from_str(text).unwrap();
        let fonts = theme.fonts.clone().expect("fonts table");
        assert_eq!(fonts.regular.as_ref().and_then(|f| f.google.as_deref()), Some("Rajdhani"));
        assert_eq!(fonts.bold.as_ref().and_then(|f| f.weight), Some(700));
        assert_eq!(fonts.specs().count(), 2);
        let gradient = theme.global_gradient().expect("diagonal gradient");
        assert_eq!((gradient.angle, gradient.ratio), (135.0, Some(GradientRatio::Stops([0.15, 0.55]))));
    }

    #[test]
    fn bundled_gradient_themes_use_the_new_keys() {
        for (name, text) in Theme::BUNDLED.iter().filter(|(name, _)| name.contains("gradient")) {
            let theme: Theme = toml::from_str(text).unwrap();
            assert!(theme.gradient.is_none() && theme.global_gradient().is_some(), "{name}");
        }
    }

    #[test]
    fn fonts_survive_a_save_and_themes_without_them_stay_clean() {
        let mut theme = Theme::default();
        assert!(!toml::to_string_pretty(&theme).unwrap().contains("fonts"));
        theme.fonts = Some(ThemeFonts {
            size: Some(18.0),
            mono: Some(FontSpec { path: Some("fonts/mono.ttf".into()), ..Default::default() }),
            ..Default::default()
        });
        let back: Theme = toml::from_str(&toml::to_string_pretty(&theme).unwrap()).unwrap();
        assert_eq!(back.fonts, theme.fonts);
    }
}

#[cfg(test)]
mod gradient_tests {
    use super::*;

    fn gradient(angle: f32, ratio: Option<GradientRatio>) -> Gradient {
        Gradient { start: [1.0, 0.0, 0.0, 1.0], end: [0.0, 0.0, 1.0, 1.0], angle, ratio }
    }

    #[test]
    fn css_angles_run_corner_to_corner() {
        let size = [200.0, 100.0];
        let right = gradient(90.0, None);
        assert!((right.axis_position([0.0, 50.0], size) - 0.0).abs() < 1e-5);
        assert!((right.axis_position([200.0, 50.0], size) - 1.0).abs() < 1e-5);
        let down = gradient(180.0, None);
        assert!((down.axis_position([100.0, 0.0], size)).abs() < 1e-5);
        assert!((down.axis_position([100.0, 100.0], size) - 1.0).abs() < 1e-5);
        let diagonal = gradient(135.0, None);
        assert!((diagonal.axis_position([0.0, 0.0], size)).abs() < 1e-5);
        assert!((diagonal.axis_position([200.0, 100.0], size) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn stops_hold_the_ends_solid_like_css() {
        let g = gradient(180.0, Some(GradientRatio::Stops([0.10, 0.65])));
        assert_eq!(g.blend(0.05), 0.0);
        assert_eq!(g.blend(0.8), 1.0);
        assert!((g.blend(0.375) - 0.5).abs() < 1e-5);
        assert!(!g.is_linear());
    }

    #[test]
    fn a_midpoint_moves_the_half_way_blend() {
        let g = gradient(90.0, Some(GradientRatio::Midpoint(0.25)));
        assert!((g.blend(0.25) - 0.5).abs() < 1e-5);
        assert!(g.blend(0.1) < 0.25 && g.blend(0.9) > 0.85);
        assert!(gradient(90.0, Some(GradientRatio::Midpoint(0.5))).is_linear());
    }

    #[test]
    fn both_ratio_shapes_parse_from_toml() {
        #[derive(Deserialize)]
        struct Holder {
            ratio: GradientRatio,
        }
        assert_eq!(toml::from_str::<Holder>("ratio = 0.3").unwrap().ratio, GradientRatio::Midpoint(0.3));
        assert_eq!(toml::from_str::<Holder>("ratio = [0.1, 0.65]").unwrap().ratio, GradientRatio::Stops([0.1, 0.65]));
    }

    #[test]
    fn old_gradient_files_become_global_gradients() {
        let mut theme: Theme = toml::from_str(
            "name = 'old'\nbackground = [0,0,0,1]\npanel = [0,0,0,1]\ntext = [1,1,1,1]\ntext_muted = [1,1,1,1]\naccent = [1,0,0,1]\naccent_hover = [1,0,0,1]\nborder = [0,0,0,1]\ngradient = [[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]]",
        )
        .unwrap();
        theme.migrate_legacy_gradient();
        let g = theme.global_gradient().expect("enabled");
        assert_eq!((g.start, g.end, g.angle), ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], DEFAULT_GRADIENT_ANGLE));
        let saved = toml::to_string_pretty(&theme).unwrap();
        assert!(saved.contains("use_global_gradient = true") && !saved.contains("\ngradient ="), "{saved}");
    }
}
