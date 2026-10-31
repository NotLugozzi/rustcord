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

pub fn custom_emoji_url(emoji: &Emoji, size: u32) -> String {
    let ext = if emoji.animated { "gif" } else { "png" };
    format!("{CDN_BASE}/emojis/{}.{ext}?size={size}", emoji.id)
}
