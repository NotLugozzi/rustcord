use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

pub type Snowflake = String;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub id: Snowflake,
    pub username: String,
    pub discriminator: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Guild {
    pub id: Snowflake,
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub unavailable: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Emoji {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub animated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize_repr, Serialize_repr)]
#[repr(u8)]
pub enum ChannelType {
    GuildText = 0,
    Dm = 1,
    GuildVoice = 2,
    GroupDm = 3,
    GuildCategory = 4,
    GuildAnnouncement = 5,
    GuildStageVoice = 13,
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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
    pub author: User,
    pub content: String,
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub referenced_message: Option<Box<Message>>,
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
}
