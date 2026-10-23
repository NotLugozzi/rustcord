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
