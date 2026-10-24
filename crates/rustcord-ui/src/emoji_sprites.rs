use std::collections::HashMap;
use std::sync::LazyLock;

pub const SHEET_BYTES: &[u8] = include_bytes!("../assets/emoji/sheet.png");
const INDEX_JSON: &str = include_str!("../assets/emoji/index.json");

#[derive(Clone, Copy)]
pub struct SpriteRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

static INDEX: LazyLock<HashMap<u32, SpriteRect>> = LazyLock::new(|| {
    let raw: HashMap<String, [f32; 4]> = serde_json::from_str(INDEX_JSON).expect("invalid emoji index.json");
    raw.into_iter()
        .filter_map(|(k, [x, y, w, h])| {
            let codepoint = u32::from_str_radix(&k, 16).ok()?;
            Some((codepoint, SpriteRect { x, y, w, h }))
        })
        .collect()
});

pub fn lookup(ch: char) -> Option<SpriteRect> {
    INDEX.get(&(ch as u32)).copied()
}

pub const SHEET_URL: &str = "bundled://emoji-sheet";
