use std::collections::HashSet;
use std::sync::LazyLock;

const INDEX_JSON: &str = include_str!("../assets/emoji/index.json");

pub const BASE_URL: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@15.1.0/assets/72x72";

static KNOWN: LazyLock<HashSet<u32>> = LazyLock::new(|| {
    let raw: std::collections::HashMap<String, serde_json::Value> = serde_json::from_str(INDEX_JSON).expect("invalid emoji index.json");
    raw.keys().filter_map(|k| u32::from_str_radix(k, 16).ok()).collect()
});

pub fn is_emoji(ch: char) -> bool {
    KNOWN.contains(&(ch as u32))
}

pub fn url(ch: char) -> String {
    format!("{BASE_URL}/{:x}.png", ch as u32)
}
