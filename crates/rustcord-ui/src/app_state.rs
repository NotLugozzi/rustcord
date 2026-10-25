use rustcord_client::models::{Channel, ChannelType, Emoji, Guild, Message, Presence, Relationship, RelationshipType, User};
use std::collections::HashMap;

fn snowflake_value(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

pub struct ChannelGroup<'a> {
    pub name: Option<&'a str>,
    pub channels: Vec<&'a Channel>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct DmGroup {
    pub name: String,
    pub channels: Vec<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct GroupsFile {
    groups: Vec<DmGroup>,
}

fn groups_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.config_dir().join("dm_groups.toml"))
}

fn notes_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.config_dir().join("notes.toml"))
}

pub fn load_notes() -> HashMap<String, String> {
    let text = notes_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    toml::from_str(&text).unwrap_or_default()
}

pub fn save_notes(notes: &HashMap<String, String>) {
    let Some(path) = notes_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = toml::to_string(notes) {
        let _ = std::fs::write(path, text);
    }
}

pub fn load_groups() -> Vec<DmGroup> {
    let text = groups_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    toml::from_str::<GroupsFile>(&text).map(|f| f.groups).unwrap_or_default()
}

#[derive(Default)]
pub struct AppState {
    pub current_user: Option<User>,
    pub guilds: Vec<Guild>,
    pub dm_channels: Vec<Channel>,
    pub relationships: Vec<Relationship>,
    pub dm_search: String,
    pub sidebar_tab: usize,
    pub sidebar_width: f32,
    pub groups: Vec<DmGroup>,
    pub new_group_name: String,
    pub context_dm: Option<String>,
    pub profiles: HashMap<String, rustcord_client::models::UserProfile>,
    pub profiles_requested: std::collections::HashSet<String>,
    pub notes: HashMap<String, String>,
    pub profile_tab: usize,
    pub rpc_apps: Vec<crate::presence::RpcAppInfo>,
    pub manual_activity: [String; 3],
    pub notifications_enabled: bool,
    pub window_focused: bool,
    pub editing: Option<(String, String)>,
    pub focus_compose: bool,
    pub my_status: rustcord_client::models::PresenceStatus,
    pub custom_status_input: String,
    pub guild_channels: HashMap<String, Vec<Channel>>,
    pub messages: HashMap<String, Vec<Message>>,
    pub selected_guild: Option<String>,
    pub selected_channel: Option<String>,
    pub compose_buf: String,
    pub email_input: String,
    pub password_input: String,
    pub mfa_code_input: String,
    pub mfa_ticket: Option<String>,
    pub login_error: Option<String>,
    pub login_status: Option<String>,
    pub logged_in: bool,
    pub theme: crate::theme::Theme,
    pub status_message: Option<String>,
    pub last_seen_message_count: HashMap<String, usize>,
    pub loading_older_messages: bool,
    pub pending_attachment: Option<std::path::PathBuf>,
    pub revealed_spoilers: std::collections::HashSet<usize>,
    pub typing_users: HashMap<String, HashMap<String, std::time::Instant>>,
    pub last_typing_sent: Option<std::time::Instant>,
    pub show_settings: bool,
    pub profile_popout: Option<User>,
    pub image_lightbox: Option<String>,
    pub reply_target: Option<Message>,
    pub presences: HashMap<String, Presence>,
    pub guild_emojis: HashMap<String, Vec<Emoji>>,
    pub emoji_picker_filter: String,
    pub emoji_picker_category: usize,
    pub emoji_autocomplete_selected: usize,
    pub gateway_connected: bool,
    pub voice_channel_states: HashMap<String, Vec<rustcord_client::models::VoiceState>>,
    pub connected_voice_channel: Option<(Option<String>, String)>,
    pub pending_voice_channel: Option<(Option<String>, String)>,
    pub voice_connecting: bool,
    pub self_muted: bool,
    pub self_deafened: bool,
    pub voice_input_devices: Vec<String>,
    pub voice_output_devices: Vec<String>,
    pub selected_input_device: Option<String>,
    pub selected_output_device: Option<String>,
    pub show_voice_device_picker: bool,
}

const TYPING_EXPIRY: std::time::Duration = std::time::Duration::from_secs(10);

impl AppState {
    pub fn presence_for(&self, user_id: &str) -> rustcord_client::models::PresenceStatus {
        if self.current_user.as_ref().is_some_and(|u| u.id == user_id) {
            return self.my_status;
        }
        self.presences
            .get(user_id)
            .map(|p| {
                use rustcord_client::models::PresenceStatus::*;
                if p.status != Offline {
                    return p.status;
                }
                [p.client_status.desktop, p.client_status.mobile, p.client_status.web]
                    .into_iter()
                    .flatten()
                    .find(|status| *status != Offline && *status != Invisible)
                    .unwrap_or(p.status)
            })
            .unwrap_or(rustcord_client::models::PresenceStatus::Offline)
    }

    pub fn available_emojis(&self) -> Vec<(&Guild, &Emoji)> {
        self.guilds
            .iter()
            .filter_map(|g| self.guild_emojis.get(&g.id).map(|emojis| (g, emojis)))
            .flat_map(|(g, emojis)| emojis.iter().map(move |e| (g, e)))
            .collect()
    }

    pub fn save_groups(&self) {
        let Some(path) = groups_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = GroupsFile { groups: self.groups.iter().map(|g| DmGroup { name: g.name.clone(), channels: g.channels.clone() }).collect() };
        if let Ok(text) = toml::to_string(&file) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn activity_for(&self, user_id: &str) -> Option<String> {
        let acts = &self.presences.get(user_id)?.activities;
        acts.iter().find(|a| a.kind != 4).or_else(|| acts.first()).map(|a| a.summary()).filter(|s| !s.is_empty())
    }

    pub fn platform_for(&self, user_id: &str) -> Option<&'static str> {
        let presence = self.presences.get(user_id)?;
        let cs = &presence.client_status;
        if cs.mobile.is_some() {
            Some("Mobile")
        } else if cs.desktop.is_some() {
            Some("Desktop")
        } else if cs.web.is_some() {
            Some("Web")
        } else {
            None
        }
    }

    pub fn note_typing(&mut self, channel_id: String, user_id: String) {
        self.typing_users
            .entry(channel_id)
            .or_default()
            .insert(user_id, std::time::Instant::now());
    }

    pub fn typing_users_for(&self, channel_id: &str) -> Vec<&str> {
        let Some(users) = self.typing_users.get(channel_id) else { return Vec::new() };
        let now = std::time::Instant::now();
        users
            .iter()
            .filter(|(_, started)| now.duration_since(**started) < TYPING_EXPIRY)
            .map(|(user_id, _)| user_id.as_str())
            .collect()
    }

    pub fn typing_display_name(&self, channel_id: &str, user_id: &str) -> String {
        if let Some(dm) = self.dm_channels.iter().find(|c| c.id == channel_id) {
            if let Some(user) = dm.recipients.iter().find(|u| u.id == user_id) {
                return user.global_name.clone().unwrap_or_else(|| user.username.clone());
            }
        }
        if let Some(messages) = self.messages.get(channel_id) {
            if let Some(msg) = messages.iter().rev().find(|m| m.author.id == user_id) {
                return msg.author.global_name.clone().unwrap_or_else(|| msg.author.username.clone());
            }
        }
        "Someone".to_string()
    }

    pub fn push_message(&mut self, message: Message) {
        if let Some(dm) = self.dm_channels.iter_mut().find(|c| c.id == message.channel_id) {
            dm.last_message_id = Some(message.id.clone());
        }
        let list = self.messages.entry(message.channel_id.clone()).or_default();
        let new_id = snowflake_value(&message.id);
        let insert_at = list.partition_point(|m| snowflake_value(&m.id) <= new_id);
        list.insert(insert_at, message);
    }

    pub fn set_message_history(&mut self, channel_id: String, mut messages: Vec<Message>) {
        messages.sort_by_key(|m| snowflake_value(&m.id));
        self.messages.insert(channel_id, messages);
    }

    pub fn prepend_message_history(&mut self, channel_id: String, mut older: Vec<Message>) {
        older.sort_by_key(|m| snowflake_value(&m.id));
        let existing = self.messages.entry(channel_id).or_default();
        older.extend(std::mem::take(existing));
        *existing = older;
    }

    pub fn last_message_timestamp(&self, channel_id: &str) -> Option<&str> {
        self.messages.get(channel_id)?.last().map(|m| m.timestamp.as_str())
    }

    pub fn active_messages(&self) -> &[Message] {
        self.selected_channel
            .as_ref()
            .and_then(|id| self.messages.get(id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn dm_display_name(dm: &Channel) -> String {
        dm.recipients
            .first()
            .map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()))
            .unwrap_or_else(|| dm.id.clone())
    }

    pub fn filtered_dm_channels(&self) -> Vec<&Channel> {
        let query = self.dm_search.trim().to_lowercase();
        let mut dms: Vec<&Channel> = self
            .dm_channels
            .iter()
            .filter(|dm| query.is_empty() || Self::dm_display_name(dm).to_lowercase().contains(&query))
            .collect();
        dms.sort_by_key(|dm| {
            std::cmp::Reverse(dm.last_message_id.as_deref().map(snowflake_value).unwrap_or(0))
        });
        dms
    }

    pub fn friends(&self) -> Vec<&Relationship> {
        let query = self.dm_search.trim().to_lowercase();
        self.relationships
            .iter()
            .filter(|r| r.kind == RelationshipType::Friend)
            .filter(|r| {
                query.is_empty()
                    || r.user.username.to_lowercase().contains(&query)
                    || r.user
                        .global_name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub fn apply_voice_state(&mut self, voice_state: rustcord_client::models::VoiceState) {
        let Some(guild_id) = voice_state.guild_id.clone() else { return };
        let states = self.voice_channel_states.entry(guild_id).or_default();
        states.retain(|s| s.user_id != voice_state.user_id);
        if voice_state.channel_id.is_some() {
            states.push(voice_state);
        }
    }

    pub fn voice_channels_for_selected_guild(&self) -> Vec<&Channel> {
        let Some(guild_id) = &self.selected_guild else { return Vec::new() };
        let Some(channels) = self.guild_channels.get(guild_id) else { return Vec::new() };
        let mut voice: Vec<&Channel> = channels
            .iter()
            .filter(|c| matches!(c.kind, ChannelType::GuildVoice | ChannelType::GuildStageVoice))
            .collect();
        voice.sort_by_key(|c| c.position.unwrap_or(i32::MAX));
        voice
    }

    pub fn members_in_voice_channel(&self, guild_id: &str, channel_id: &str) -> Vec<&rustcord_client::models::VoiceState> {
        let Some(states) = self.voice_channel_states.get(guild_id) else { return Vec::new() };
        states.iter().filter(|s| s.channel_id.as_deref() == Some(channel_id)).collect()
    }

    pub fn grouped_channels_for_selected_guild(&self) -> Vec<ChannelGroup<'_>> {
        let Some(guild_id) = &self.selected_guild else { return Vec::new() };
        let Some(channels) = self.guild_channels.get(guild_id) else { return Vec::new() };

        let mut categories: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.kind == ChannelType::GuildCategory)
            .collect();
        categories.sort_by_key(|c| c.position.unwrap_or(i32::MAX));

        let text_channels = |parent_id: Option<&str>| -> Vec<&Channel> {
            let mut matching: Vec<&Channel> = channels
                .iter()
                .filter(|c| matches!(c.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement))
                .filter(|c| c.parent_id.as_deref() == parent_id)
                .collect();
            matching.sort_by_key(|c| c.position.unwrap_or(i32::MAX));
            matching
        };

        let mut groups = Vec::new();
        let uncategorized = text_channels(None);
        if !uncategorized.is_empty() {
            groups.push(ChannelGroup { name: None, channels: uncategorized });
        }
        for category in categories {
            let channels = text_channels(Some(&category.id));
            if !channels.is_empty() {
                groups.push(ChannelGroup {
                    name: Some(category.name.as_deref().unwrap_or("Category")),
                    channels,
                });
            }
        }
        groups
    }
}
