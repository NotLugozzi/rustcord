use crate::theme::{FontSpec, ThemeFonts};
use std::path::PathBuf;

pub const DEFAULT_SIZE: f32 = 16.0;
const MIN_SIZE: f32 = 10.0;
const MAX_SIZE: f32 = 28.0;

#[derive(Default)]
pub struct CustomFonts {
    pub size: f32,
    pub regular: Option<Vec<u8>>,
    pub bold: Option<Vec<u8>>,
    pub italic: Option<Vec<u8>>,
    pub bold_italic: Option<Vec<u8>>,
    pub mono: Option<Vec<u8>>,
}

pub enum Resolution {
    Ready(CustomFonts),
    Waiting(Vec<FontSpec>),
}

pub fn is_font_file(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0x00, 0x01, 0x00, 0x00] | b"OTTO" | b"true" | b"ttcf"))
}

pub fn google_css_url(family: &str, weight: u16, italic: bool) -> String {
    let family: String = family.trim().split_whitespace().collect::<Vec<_>>().join("+");
    format!("https://fonts.googleapis.com/css2?family={family}:ital,wght@{},{weight}", u8::from(italic))
}

pub fn truetype_url(css: &str) -> Option<String> {
    css.split("src:").skip(1).find_map(|src| {
        let declaration = src.split(';').next()?;
        if !declaration.contains("truetype") && !declaration.contains("opentype") {
            return None;
        }
        let start = declaration.find("url(")? + 4;
        let end = declaration[start..].find(')')? + start;
        Some(declaration[start..end].trim_matches(|c| c == '\'' || c == '"').to_string())
    })
}

pub fn cache_key(spec: &FontSpec) -> Option<String> {
    use std::hash::{Hash, Hasher};
    if let Some(family) = &spec.google {
        let slug: String = family.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
        return Some(format!("google-{slug}-{}{}.ttf", spec.weight.unwrap_or(400), if spec.italic { "i" } else { "" }));
    }
    let url = spec.url.as_ref()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    Some(format!("url-{:016x}.font", hasher.finish()))
}

fn cache_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.cache_dir().join("fonts"))
}

fn read_font(path: PathBuf) -> Option<Vec<u8>> {
    std::fs::read(path).ok().filter(|bytes| is_font_file(bytes))
}

pub fn load_local(spec: &FontSpec) -> Option<Vec<u8>> {
    if let Some(path) = &spec.path {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() { path } else { crate::theme::Theme::themes_dir()?.join(path) };
        return read_font(path);
    }
    read_font(cache_dir()?.join(cache_key(spec)?))
}

pub async fn fetch(client: &reqwest::Client, spec: &FontSpec) -> Result<(), String> {
    let url = match (&spec.google, &spec.url) {
        (Some(family), _) => {
            let css_url = google_css_url(family, spec.weight.unwrap_or(400), spec.italic);
            let css = client.get(&css_url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("{family}: {e}"))?;
            let css = css.text().await.map_err(|e| format!("{family}: {e}"))?;
            truetype_url(&css).ok_or_else(|| format!("Google Fonts has no TrueType file for {family}"))?
        }
        (None, Some(url)) => url.clone(),
        (None, None) => return Err("font has no google family or url to download".into()),
    };
    let response = client.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("{url}: {e}"))?;
    let bytes = response.bytes().await.map_err(|e| format!("{url}: {e}"))?;
    if !is_font_file(&bytes) {
        return Err(format!("{url} is not a TTF or OTF font (woff/woff2 are not supported)"));
    }
    let (Some(dir), Some(key)) = (cache_dir(), cache_key(spec)) else { return Ok(()) };
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    std::fs::write(dir.join(key), &bytes).map_err(|e| format!("could not cache font: {e}"))
}

pub fn resolve(fonts: Option<&ThemeFonts>, failed: &dyn Fn(&FontSpec) -> bool) -> Resolution {
    let Some(fonts) = fonts else { return Resolution::Ready(CustomFonts { size: DEFAULT_SIZE, ..Default::default() }) };
    let mut missing = Vec::new();
    let mut load = |spec: &Option<FontSpec>| {
        let spec = spec.as_ref()?;
        match load_local(spec) {
            Some(bytes) => Some(bytes),
            None => {
                if spec.path.is_none() && !failed(spec) {
                    missing.push(spec.clone());
                }
                None
            }
        }
    };
    let regular = load(&fonts.regular);
    let bold = load(&fonts.bold);
    let italic = load(&fonts.italic);
    let bold_italic = load(&fonts.bold_italic);
    let mono = load(&fonts.mono);
    if !missing.is_empty() {
        return Resolution::Waiting(missing);
    }
    let size = fonts.size.unwrap_or(DEFAULT_SIZE).clamp(MIN_SIZE, MAX_SIZE);
    Resolution::Ready(CustomFonts {
        size,
        italic: italic.or_else(|| regular.clone()),
        bold_italic: bold_italic.or_else(|| bold.clone()),
        regular,
        bold,
        mono,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_urls_ask_for_one_weight_and_style() {
        assert_eq!(google_css_url("Rajdhani", 500, false), "https://fonts.googleapis.com/css2?family=Rajdhani:ital,wght@0,500");
        assert_eq!(google_css_url(" Fira  Sans ", 700, true), "https://fonts.googleapis.com/css2?family=Fira+Sans:ital,wght@1,700");
    }

    #[test]
    fn the_truetype_source_is_picked_out_of_the_css() {
        let css = "@font-face {\n  font-family: 'Rajdhani';\n  src: url(https://fonts.gstatic.com/s/rajdhani/v17/LDI2.ttf) format('truetype');\n}";
        assert_eq!(truetype_url(css).as_deref(), Some("https://fonts.gstatic.com/s/rajdhani/v17/LDI2.ttf"));
        assert_eq!(truetype_url("src: url(x.woff2) format('woff2');"), None);
    }

    #[test]
    fn only_real_font_files_are_accepted() {
        assert!(is_font_file(&[0, 1, 0, 0, 9]));
        assert!(is_font_file(b"OTTOxxxx"));
        assert!(!is_font_file(b"wOF2xxxx"));
        assert!(!is_font_file(b"<html>"));
    }

    #[test]
    fn cache_keys_are_stable_and_filesystem_safe() {
        let spec = FontSpec { google: Some("Fira Sans".into()), weight: Some(700), italic: true, ..Default::default() };
        assert_eq!(cache_key(&spec).as_deref(), Some("google-fira-sans-700i.ttf"));
        let url = FontSpec { url: Some("https://example.com/a.ttf".into()), ..Default::default() };
        assert_eq!(cache_key(&url), cache_key(&url.clone()));
        assert!(cache_key(&FontSpec::default()).is_none());
    }

    #[test]
    fn a_theme_without_fonts_is_ready_at_the_default_size() {
        match resolve(None, &|_| false) {
            Resolution::Ready(fonts) => assert!(fonts.regular.is_none() && fonts.size == DEFAULT_SIZE),
            Resolution::Waiting(_) => panic!("nothing to wait for"),
        }
    }

    #[test]
    fn uncached_fonts_are_reported_missing_unless_they_already_failed() {
        let fonts = ThemeFonts {
            regular: Some(FontSpec { google: Some("Rajdhani".into()), weight: Some(500), ..Default::default() }),
            ..Default::default()
        };
        assert!(matches!(resolve(Some(&fonts), &|_| false), Resolution::Waiting(missing) if missing.len() == 1));
        assert!(matches!(resolve(Some(&fonts), &|_| true), Resolution::Ready(ready) if ready.regular.is_none()));
    }
}
