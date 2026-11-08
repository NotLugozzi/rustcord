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

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "discord-dark".into(),
            background: [0.212, 0.224, 0.247, 1.0],
            panel: [0.184, 0.192, 0.212, 1.0],
            servers_bg: None,
            user_bar_bg: None,
            text: [0.863, 0.867, 0.871, 1.0],
            text_muted: [0.584, 0.612, 0.639, 1.0],  
            accent: [0.345, 0.396, 0.949, 1.0],      
            accent_hover: [0.278, 0.322, 0.816, 1.0],
            border: [0.133, 0.141, 0.153, 1.0],  
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

    const BUNDLED: [(&'static str, &'static str); 7] = [
        ("discord-dark.toml", include_str!("../../../themes/discord-dark.toml")),
        ("nord.toml", include_str!("../../../themes/nord.toml")),
        ("solarized-light.toml", include_str!("../../../themes/solarized-light.toml")),
        ("aurora-gradient.toml", include_str!("../../../themes/aurora-gradient.toml")),
        ("sunset-gradient.toml", include_str!("../../../themes/sunset-gradient.toml")),
        ("cosmic-gradient.toml", include_str!("../../../themes/cosmic-gradient.toml")),
        ("cyberpunk.toml", include_str!("../../../themes/cyberpunk.toml")),
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
        use imgui::StyleColor::*;
        style[WindowBg] = if self.use_global_gradient { [self.background[0], self.background[1], self.background[2], 0.84] } else { self.background };
        style[ChildBg] = if self.use_global_gradient { [self.panel[0], self.panel[1], self.panel[2], 0.86] } else { self.panel };
        style[PopupBg] = self.panel;
        style[Text] = self.text;
        style[TextDisabled] = self.text_muted;
        style[Button] = self.accent;
        style[ButtonHovered] = self.accent_hover;
        style[ButtonActive] = self.accent_hover;
        style[Header] = [self.accent[0], self.accent[1], self.accent[2], 0.22];
        style[HeaderHovered] = [self.accent_hover[0], self.accent_hover[1], self.accent_hover[2], 0.30];
        style[HeaderActive] = [self.accent_hover[0], self.accent_hover[1], self.accent_hover[2], 0.38];
        style[Border] = self.border;
        style[FrameBg] = self.panel;
        style[FrameBgHovered] = [self.accent[0], self.accent[1], self.accent[2], 0.22];
        style[FrameBgActive] = [self.accent_hover[0], self.accent_hover[1], self.accent_hover[2], 0.30];

        style[ScrollbarGrabHovered] = self.border;
        style[ScrollbarGrabActive] = self.panel;

        style.window_border_size = 0.0;
        style.child_border_size = 0.0;
        style.popup_border_size = 1.0;
        style.frame_border_size = 0.0;

        style.frame_rounding = 5.0;
        style.popup_rounding = 8.0;
        style.child_rounding = 4.0;
        style.scrollbar_rounding = 4.0;
        style.grab_rounding = 4.0;

        style.window_padding = [12.0, 12.0];
        style.frame_padding = [9.0, 6.0];
        style.item_spacing = [8.0, 6.0];
        style.scrollbar_size = 10.0;
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

#[cfg(test)]
mod tests {
    use super::*;

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
