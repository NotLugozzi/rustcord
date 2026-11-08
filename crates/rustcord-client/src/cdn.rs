use crate::models::{Emoji, Guild, User};

const CDN_BASE: &str = "https://cdn.discordapp.com";

pub fn avatar_url(user: &User, size: u32) -> Option<String> {
    let hash = user.avatar.as_ref()?;
    let ext = if hash.starts_with("a_") { "gif" } else { "png" };
    Some(format!("{CDN_BASE}/avatars/{}/{hash}.{ext}?size={size}", user.id))
}

pub fn default_avatar_url(user: &User) -> String {
    let index = match user.discriminator.parse::<u64>() {
        Ok(0) | Err(_) => user.id.parse::<u64>().map(|id| (id >> 22) % 6).unwrap_or(0),
        Ok(d) => d % 5,
    };
    format!("{CDN_BASE}/embed/avatars/{index}.png")
}

pub fn guild_icon_url(guild: &Guild, size: u32) -> Option<String> {
    let hash = guild.icon.as_ref()?;
    let ext = if hash.starts_with("a_") { "gif" } else { "png" };
    Some(format!("{CDN_BASE}/icons/{}/{hash}.{ext}?size={size}", guild.id))
}

pub fn channel_icon_url(channel_id: &str, hash: &str, size: u32) -> String {
    format!("{CDN_BASE}/channel-icons/{channel_id}/{hash}.png?size={size}")
}

pub fn sticker_url(id: &str, format_type: u8, size: u32) -> Option<String> {
    let ext = match format_type {
        crate::models::STICKER_FORMAT_LOTTIE => return None,
        crate::models::STICKER_FORMAT_GIF => "gif",
        _ => "png",
    };
    Some(format!("https://media.discordapp.net/stickers/{id}.{ext}?size={size}"))
}

pub fn custom_emoji_url(emoji: &Emoji, size: u32) -> String {
    let ext = if emoji.animated { "gif" } else { "png" };
    format!("{CDN_BASE}/emojis/{}.{ext}?size={size}", emoji.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticker_urls_follow_the_format() {
        assert_eq!(sticker_url("1", 1, 160).as_deref(), Some("https://media.discordapp.net/stickers/1.png?size=160"));
        assert_eq!(sticker_url("1", 2, 160).as_deref(), Some("https://media.discordapp.net/stickers/1.png?size=160"));
        assert_eq!(sticker_url("1", 4, 160).as_deref(), Some("https://media.discordapp.net/stickers/1.gif?size=160"));
        assert!(sticker_url("1", 3, 160).is_none());
    }
}
