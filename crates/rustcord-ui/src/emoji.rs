use std::collections::HashSet;
use std::sync::LazyLock;

const INDEX_JSON: &str = include_str!("../assets/emoji/index.json");

pub const BASE_URL: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@15.1.0/assets/72x72";

pub const BUNDLED: &[(char, &[u8])] = &[
    ('\u{1F4DE}', include_bytes!("../assets/emoji/ui/1f4de.png")),
    ('\u{1F4F5}', include_bytes!("../assets/emoji/ui/1f4f5.png")),
    ('\u{1F3A4}', include_bytes!("../assets/emoji/ui/1f3a4.png")),
    ('\u{1F507}', include_bytes!("../assets/emoji/ui/1f507.png")),
    ('\u{1F3A7}', include_bytes!("../assets/emoji/ui/1f3a7.png")),
    ('\u{1F515}', include_bytes!("../assets/emoji/ui/1f515.png")),
    ('\u{1F92B}', include_bytes!("../assets/emoji/ui/1f92b.png")),
    ('\u{1F4F4}', include_bytes!("../assets/emoji/ui/1f4f4.png")),
    ('\u{2139}', include_bytes!("../assets/emoji/ui/2139.png")),
];

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
