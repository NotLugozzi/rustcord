use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

pub type Snowflake = String;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub id: Snowflake,
    pub username: String,
    pub discriminator: String,
    #[serde(default, deserialize_with = "non_blank_string")]
    pub global_name: Option<String>,
    pub avatar: Option<String>,
    #[serde(default)]
    pub bot: bool,
    #[serde(default)]
    pub primary_guild: Option<PrimaryGuild>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PrimaryGuild {
    pub identity_guild_id: Option<Snowflake>,
    #[serde(default)]
    pub identity_enabled: Option<bool>,
    pub tag: Option<String>,
    pub badge: Option<String>,
}

impl PrimaryGuild {
    pub fn visible_tag(&self) -> Option<&str> {
        if self.identity_enabled != Some(true) {
            return None;
        }
        self.tag.as_deref().filter(|t| !t.is_empty())
    }

    pub fn badge_url(&self, size: u32) -> Option<String> {
        let guild_id = self.identity_guild_id.as_deref()?;
        let badge = self.badge.as_deref()?;
        Some(format!("https://cdn.discordapp.com/guild-tag-badges/{guild_id}/{badge}.png?size={size}"))
    }
}

fn non_blank_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.filter(|s| !s.trim().is_empty()))
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name.as_deref().unwrap_or(&self.username)
    }

    pub fn server_tag(&self) -> Option<&PrimaryGuild> {
        self.primary_guild.as_ref().filter(|g| g.visible_tag().is_some())
    }
}

#[cfg(test)]
mod guild_settings_tests {
    use super::*;

    #[test]
    fn overrides_parse_from_an_array_or_a_map() {
        let from_array: GuildSettings = serde_json::from_value(serde_json::json!({
            "guild_id": "g", "muted": false, "message_notifications": 1,
            "channel_overrides": [{ "channel_id": "c", "muted": true, "message_notifications": 3 }]
        }))
        .unwrap();
        assert!(from_array.channel_override("c").unwrap().muted);
        let from_map: GuildSettings = serde_json::from_value(serde_json::json!({
            "guild_id": null, "channel_overrides": { "d": { "muted": true } }
        }))
        .unwrap();
        assert_eq!(from_map.key(), "@me");
        assert_eq!(from_map.message_notifications, NOTIFY_INHERIT);
        assert_eq!(from_map.channel_override("d").unwrap().message_notifications, NOTIFY_INHERIT);
    }

    #[test]
    fn mute_patches_carry_a_config_only_when_muting() {
        let on = mute_fields(true, Some("2026-11-08T13:00:00+00:00".into()), 3600);
        assert_eq!(on["mute_config"]["selected_time_window"], 3600);
        let off = mute_fields(false, None, -1);
        assert!(off["mute_config"].is_null());
        let channel = channel_override_patch("c", mute_fields(true, None, -1));
        assert_eq!(channel["channel_overrides"]["c"]["muted"], true);
    }
}

#[cfg(test)]
mod sticker_tests {
    use super::*;

    fn gif_message(content: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "c", "author": { "id": "u", "username": "u", "discriminator": "0" },
            "content": content, "timestamp": "2026-01-01T00:00:00+00:00", "edited_timestamp": null,
            "embeds": [{ "type": "gifv", "url": "https://klipy.com/gifs/x", "video": { "url": "https://x/y.mp4" } }]
        }))
        .unwrap()
    }

    #[test]
    fn only_a_bare_gif_link_is_hidden() {
        assert!(gif_message("https://klipy.com/gifs/x").is_only_gif_link());
        assert!(gif_message(" https://klipy.com/gifs/x\n").is_only_gif_link());
        assert!(!gif_message("look https://klipy.com/gifs/x").is_only_gif_link());
        assert!(!gif_message("").is_only_gif_link());
    }

    #[test]
    fn a_message_without_stickers_still_parses() {
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "c", "author": { "id": "u", "username": "u", "discriminator": "0" },
            "content": "hi", "timestamp": "2026-01-01T00:00:00+00:00", "edited_timestamp": null
        }))
        .unwrap();
        assert!(m.sticker_items.is_empty());
    }

    #[test]
    fn sticker_items_parse_from_a_message() {
        let m: Message = serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": "c", "author": { "id": "u", "username": "u", "discriminator": "0" },
            "content": "", "timestamp": "2026-01-01T00:00:00+00:00", "edited_timestamp": null,
            "sticker_items": [{ "id": "s", "name": "Wave", "format_type": 3 }]
        }))
        .unwrap();
        assert_eq!(m.sticker_items[0].format_type, STICKER_FORMAT_LOTTIE);
    }

    #[test]
    fn stickers_match_on_name_or_tags() {
        let s: Sticker = serde_json::from_value(serde_json::json!({ "id": "1", "name": "Wumpus Wave", "format_type": 1, "tags": "hello, hi" })).unwrap();
        assert!(s.available);
        assert!(s.matches(""));
        assert!(s.matches("wave"));
        assert!(s.matches("hello"));
        assert!(!s.matches("bye"));
    }

    #[test]
    fn a_gif_thumbnail_prefers_the_static_preview() {
        let g: Gif = serde_json::from_value(serde_json::json!({ "url": "https://tenor.com/x", "src": "a.gif", "gif_src": "b.gif", "preview": "c.png", "width": 1, "height": 1 })).unwrap();
        assert_eq!(g.thumbnail(), "c.png");
        let g: Gif = serde_json::from_value(serde_json::json!({ "url": "https://tenor.com/x", "src": "a.gif" })).unwrap();
        assert_eq!(g.thumbnail(), "a.gif");
    }
}

#[cfg(test)]
mod user_tests {
    use super::*;

    fn user(json: serde_json::Value) -> User {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_blank_display_name_falls_back_to_the_username() {
        let u = user(serde_json::json!({ "id": "1", "username": "olbiaphleet", "discriminator": "0", "global_name": "  ", "avatar": null }));
        assert_eq!(u.display_name(), "olbiaphleet");
        let u = user(serde_json::json!({ "id": "1", "username": "olbiaphleet", "discriminator": "0", "avatar": null }));
        assert_eq!(u.display_name(), "olbiaphleet");
    }

    #[test]
    fn server_tags_only_show_when_identity_is_enabled() {
        let tagged = |enabled: bool| {
            user(serde_json::json!({
                "id": "1", "username": "luna", "discriminator": "0", "avatar": null,
                "primary_guild": { "identity_guild_id": "9", "identity_enabled": enabled, "tag": "BCAN", "badge": "abc" }
            }))
        };
        assert_eq!(tagged(true).server_tag().and_then(|g| g.visible_tag()), Some("BCAN"));
        assert!(tagged(false).server_tag().is_none());
        assert_eq!(
            tagged(true).server_tag().and_then(|g| g.badge_url(16)).as_deref(),
            Some("https://cdn.discordapp.com/guild-tag-badges/9/abc.png?size=16")
        );
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Guild {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub unavailable: bool,
    #[serde(default)]
    pub owner_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Invite {
    pub code: String,
    #[serde(default)]
    pub guild: Option<InviteGuild>,
    #[serde(default)]
    pub channel: Option<InviteChannel>,
    #[serde(default)]
    pub approximate_member_count: Option<u64>,
    #[serde(default)]
    pub approximate_presence_count: Option<u64>,
    #[serde(default)]
    pub inviter: Option<User>,
    #[serde(default)]
    pub uses: Option<u32>,
    #[serde(default)]
    pub max_uses: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InviteGuild {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
}

impl InviteGuild {
    pub fn to_guild(&self) -> Guild {
        Guild { id: self.id.clone(), name: self.name.clone(), icon: self.icon.clone(), unavailable: false, owner_id: None }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InviteChannel {
    pub id: Snowflake,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: ChannelType,
}

impl InviteChannel {
    pub fn is_voice(&self) -> bool {
        matches!(self.kind, ChannelType::GuildVoice | ChannelType::GuildStageVoice)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Emoji {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub animated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    GuildText,
    Dm,
    GuildVoice,
    GroupDm,
    GuildCategory,
    GuildAnnouncement,
    GuildAnnouncementThread,
    GuildPublicThread,
    GuildPrivateThread,
    GuildStageVoice,
    GuildDirectory,
    GuildForum,
    GuildMedia,
    Unknown(u8),
}

impl<'de> Deserialize<'de> for ChannelType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match u8::deserialize(deserializer)? {
            0 => ChannelType::GuildText,
            1 => ChannelType::Dm,
            2 => ChannelType::GuildVoice,
            3 => ChannelType::GroupDm,
            4 => ChannelType::GuildCategory,
            5 => ChannelType::GuildAnnouncement,
            10 => ChannelType::GuildAnnouncementThread,
            11 => ChannelType::GuildPublicThread,
            12 => ChannelType::GuildPrivateThread,
            13 => ChannelType::GuildStageVoice,
            14 => ChannelType::GuildDirectory,
            15 => ChannelType::GuildForum,
            16 => ChannelType::GuildMedia,
            other => ChannelType::Unknown(other),
        })
    }
}

impl Serialize for ChannelType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value: u8 = match self {
            ChannelType::GuildText => 0,
            ChannelType::Dm => 1,
            ChannelType::GuildVoice => 2,
            ChannelType::GroupDm => 3,
            ChannelType::GuildCategory => 4,
            ChannelType::GuildAnnouncement => 5,
            ChannelType::GuildAnnouncementThread => 10,
            ChannelType::GuildPublicThread => 11,
            ChannelType::GuildPrivateThread => 12,
            ChannelType::GuildStageVoice => 13,
            ChannelType::GuildDirectory => 14,
            ChannelType::GuildForum => 15,
            ChannelType::GuildMedia => 16,
            ChannelType::Unknown(other) => *other,
        };
        value.serialize(serializer)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceState {
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub user_id: Snowflake,
    pub session_id: String,
    #[serde(default)]
    pub self_mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
    #[serde(default)]
    pub self_stream: Option<bool>,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub deaf: bool,
    #[serde(default)]
    pub member: Option<VoiceStateMember>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VoiceStateMember {
    pub nick: Option<String>,
    pub user: Option<User>,
}

impl VoiceState {
    pub fn display_name(&self) -> String {
        if let Some(member) = &self.member {
            if let Some(nick) = &member.nick {
                return nick.clone();
            }
            if let Some(user) = &member.user {
                return user.global_name.clone().unwrap_or_else(|| user.username.clone());
            }
        }
        self.user_id.clone()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Channel {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: ChannelType,
    pub guild_id: Option<Snowflake>,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub position: Option<i32>,
    pub parent_id: Option<Snowflake>,
    #[serde(default)]
    pub recipients: Vec<User>,
    pub last_message_id: Option<Snowflake>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permission_overwrites: Vec<PermissionOverwrite>,
}

fn deserialize_permissions_bits<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrInt {
        String(String),
        Int(u64),
    }
    match StringOrInt::deserialize(deserializer)? {
        StringOrInt::String(s) => s.parse().map_err(serde::de::Error::custom),
        StringOrInt::Int(n) => Ok(n),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteKind {
    Role,
    Member,
}

impl<'de> Deserialize<'de> for OverwriteKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match u8::deserialize(deserializer)? {
            0 => OverwriteKind::Role,
            _ => OverwriteKind::Member,
        })
    }
}

impl Serialize for OverwriteKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value: u8 = match self {
            OverwriteKind::Role => 0,
            OverwriteKind::Member => 1,
        };
        value.serialize(serializer)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PermissionOverwrite {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: OverwriteKind,
    #[serde(deserialize_with = "deserialize_permissions_bits")]
    pub allow: u64,
    #[serde(deserialize_with = "deserialize_permissions_bits")]
    pub deny: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Role {
    pub id: Snowflake,
    pub name: String,
    #[serde(deserialize_with = "deserialize_permissions_bits")]
    pub permissions: u64,
    pub position: i32,
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub managed: bool,
}

impl Role {
    pub fn rgba(&self) -> Option<[f32; 4]> {
        if self.color == 0 {
            return None;
        }
        let c = self.color;
        Some([
            ((c >> 16) & 255) as f32 / 255.0,
            ((c >> 8) & 255) as f32 / 255.0,
            (c & 255) as f32 / 255.0,
            1.0,
        ])
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuildMember {
    pub user: User,
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<Snowflake>,
    pub joined_at: String,
    #[serde(default)]
    pub communication_disabled_until: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SelfGuildMember {
    #[serde(default)]
    pub roles: Vec<Snowflake>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReactionEmoji {
    pub id: Option<Snowflake>,
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

impl ReactionEmoji {
    pub fn key(&self) -> String {
        match (&self.id, &self.name) {
            (Some(id), Some(name)) => format!("{name}:{id}"),
            (Some(id), None) => format!(":{id}"),
            (None, Some(name)) => name.clone(),
            (None, None) => String::new(),
        }
    }

    pub fn reaction_path(&self) -> String {
        match &self.id {
            Some(id) => format!("{}:{}", self.name.as_deref().unwrap_or("_"), id),
            None => self.name.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Reaction {
    pub emoji: ReactionEmoji,
    pub count: u32,
    #[serde(default)]
    pub me: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PartialMember {
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<Snowflake>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub author: User,
    #[serde(default)]
    pub member: Option<PartialMember>,
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub referenced_message: Option<Box<Message>>,
    #[serde(default, deserialize_with = "lenient_embeds")]
    pub embeds: Vec<Embed>,
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub call: Option<CallInfo>,
    #[serde(default)]
    pub webhook_id: Option<Snowflake>,
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
    #[serde(default)]
    pub sticker_items: Vec<StickerItem>,
}

impl Message {
    pub fn is_only_gif_link(&self) -> bool {
        let content = self.content.trim();
        !content.is_empty()
            && self
                .embeds
                .iter()
                .any(|e| e.kind.as_deref() == Some("gifv") && e.url.as_deref() == Some(content))
    }
}

pub const NOTIFY_ALL: u8 = 0;
pub const NOTIFY_MENTIONS: u8 = 1;
pub const NOTIFY_NOTHING: u8 = 2;
pub const NOTIFY_INHERIT: u8 = 3;

fn inherit_notifications() -> u8 {
    NOTIFY_INHERIT
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct MuteConfig {
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub selected_time_window: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChannelOverride {
    pub channel_id: Snowflake,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub mute_config: Option<MuteConfig>,
    #[serde(default = "inherit_notifications")]
    pub message_notifications: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuildSettings {
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub mute_config: Option<MuteConfig>,
    #[serde(default = "inherit_notifications")]
    pub message_notifications: u8,
    #[serde(default, deserialize_with = "lenient_overrides")]
    pub channel_overrides: Vec<ChannelOverride>,
}

impl GuildSettings {
    pub fn key(&self) -> &str {
        self.guild_id.as_deref().unwrap_or("@me")
    }

    pub fn channel_override(&self, channel_id: &str) -> Option<&ChannelOverride> {
        self.channel_overrides.iter().find(|o| o.channel_id == channel_id)
    }
}

fn lenient_overrides<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<ChannelOverride>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let entries: Vec<serde_json::Value> = match raw {
        Some(serde_json::Value::Array(items)) => items,
        Some(serde_json::Value::Object(map)) => map
            .into_iter()
            .map(|(channel_id, mut entry)| {
                if let Some(obj) = entry.as_object_mut() {
                    obj.entry("channel_id").or_insert(serde_json::Value::String(channel_id));
                }
                entry
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(entries.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

pub fn mute_fields(muted: bool, end_time: Option<String>, window_secs: i64) -> serde_json::Value {
    let config = muted.then(|| serde_json::json!({ "selected_time_window": window_secs, "end_time": end_time }));
    serde_json::json!({ "muted": muted, "mute_config": config })
}

pub fn channel_override_patch(channel_id: &str, fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "channel_overrides": { channel_id: fields } })
}

pub const STICKER_FORMAT_LOTTIE: u8 = 3;
pub const STICKER_FORMAT_GIF: u8 = 4;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StickerItem {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub format_type: u8,
}

fn available_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sticker {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub format_type: u8,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default = "available_by_default")]
    pub available: bool,
}

impl Sticker {
    pub fn matches(&self, filter: &str) -> bool {
        filter.is_empty()
            || self.name.to_lowercase().contains(filter)
            || self.tags.as_deref().is_some_and(|t| t.to_lowercase().contains(filter))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StickerPack {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub stickers: Vec<Sticker>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Gif {
    pub url: String,
    pub src: String,
    #[serde(default)]
    pub gif_src: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

impl Gif {
    pub fn thumbnail(&self) -> &str {
        self.preview.as_deref().or(self.gif_src.as_deref()).unwrap_or(&self.src)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageReference {
    pub message_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
}

fn lenient_embeds<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Embed>, D::Error> {
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Embed {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub timestamp: Option<String>,
    pub footer: Option<EmbedText>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub provider: Option<EmbedText>,
    pub author: Option<EmbedText>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EmbedText {
    #[serde(alias = "name", default)]
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EmbedMedia {
    #[serde(default)]
    pub url: String,
    pub proxy_url: Option<String>,
}

impl EmbedMedia {
    pub fn best_url(&self) -> &str {
        self.proxy_url.as_deref().filter(|u| !u.is_empty()).unwrap_or(&self.url)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EmbedField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CallInfo {
    #[serde(default)]
    pub participants: Vec<Snowflake>,
    pub ended_timestamp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize_repr, Serialize_repr)]
#[repr(u8)]
pub enum RelationshipType {
    Friend = 1,
    Blocked = 2,
    IncomingRequest = 3,
    OutgoingRequest = 4,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Relationship {
    pub id: Snowflake,
    #[serde(rename = "type")]
    pub kind: RelationshipType,
    pub user: User,
    #[serde(default)]
    pub since: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Badge {
    pub description: String,
    pub icon_url: String,
}

#[derive(Debug, Clone, Default)]
pub struct UserProfile {
    pub bio: Option<String>,
    pub banner_url: Option<String>,
    pub badges: Vec<Badge>,
    pub mutual_guild_ids: Vec<String>,
    pub mutual_friends: Vec<User>,
}

impl UserProfile {
    pub fn from_value(v: &serde_json::Value) -> Self {
        let str_of = |x: &serde_json::Value| x.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let user = &v["user"];
        let banner_url = str_of(&user["banner"]).map(|hash| {
            let ext = if hash.starts_with("a_") { "gif" } else { "png" };
            format!("https://cdn.discordapp.com/banners/{}/{hash}.{ext}?size=480", user["id"].as_str().unwrap_or_default())
        });
        let badges = v["badges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| {
                Some(Badge {
                    description: b["description"].as_str().unwrap_or_default().to_string(),
                    icon_url: format!("https://cdn.discordapp.com/badge-icons/{}.png", b["icon"].as_str()?),
                })
            })
            .collect();
        let mutual_guild_ids =
            v["mutual_guilds"].as_array().into_iter().flatten().filter_map(|g| str_of(&g["id"])).collect();
        let mutual_friends = v["mutual_friends"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|u| serde_json::from_value(u.clone()).ok())
            .collect();
        Self {
            bio: str_of(&v["user_profile"]["bio"]).or_else(|| str_of(&user["bio"])),
            banner_url,
            badges,
            mutual_guild_ids,
            mutual_friends,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceStatus {
    Online,
    Idle,
    Dnd,
    Offline,
    Invisible,
}

impl Default for PresenceStatus {
    fn default() -> Self {
        Self::Offline
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ClientStatus {
    pub desktop: Option<PresenceStatus>,
    pub mobile: Option<PresenceStatus>,
    pub web: Option<PresenceStatus>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Presence {
    pub user: PartialUser,
    #[serde(default)]
    pub status: PresenceStatus,
    #[serde(default)]
    pub client_status: ClientStatus,
    #[serde(default)]
    pub activities: Vec<Activity>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Activity {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: u8,
    pub details: Option<String>,
    pub state: Option<String>,
    pub application_id: Option<String>,
    #[serde(default)]
    pub assets: Option<ActivityAssets>,
    #[serde(default)]
    pub emoji: Option<ActivityEmoji>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ActivityEmoji {
    pub name: Option<String>,
    pub id: Option<Snowflake>,
    #[serde(default)]
    pub animated: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ActivityAssets {
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
}

pub fn activity_image_url(application_id: Option<&str>, key: &str) -> Option<String> {
    if let Some(rest) = key.strip_prefix("mp:") {
        return Some(format!("https://media.discordapp.net/{rest}"));
    }
    if let Some(id) = key.strip_prefix("spotify:") {
        return Some(format!("https://i.scdn.co/image/{id}"));
    }
    if key.starts_with("http") {
        return Some(key.to_string());
    }
    key.chars().all(|c| c.is_ascii_digit()).then_some(())?;
    Some(format!("https://cdn.discordapp.com/app-assets/{}/{key}.png", application_id?))
}

impl Activity {
    pub fn summary_title(&self) -> String {
        self.summary().split(" - ").next().unwrap_or_default().to_string()
    }

    pub fn large_image_url(&self) -> Option<String> {
        let key = self.assets.as_ref()?.large_image.as_deref()?;
        activity_image_url(self.application_id.as_deref(), key)
    }

    pub fn summary(&self) -> String {
        if self.kind == 4 {
            return self.state.clone().unwrap_or_default();
        }
        let verb = match self.kind {
            1 => "Streaming",
            2 => "Listening to",
            3 => "Watching",
            5 => "Competing in",
            _ => "Playing",
        };
        let mut out = format!("{verb} {}", self.name);
        for extra in [&self.details, &self.state].into_iter().flatten() {
            out.push_str(" - ");
            out.push_str(extra);
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PartialUser {
    pub id: Snowflake,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MergedPresences {
    #[serde(default)]
    pub friends: Vec<Presence>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Attachment {
    pub id: Snowflake,
    pub filename: String,
    pub url: String,
    pub size: u64,
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}
