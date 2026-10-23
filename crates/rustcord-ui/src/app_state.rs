use rustcord_client::models::{Channel, ChannelType, Guild, Message, Relationship, RelationshipType, User};
use std::collections::HashMap;

fn snowflake_value(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

pub struct ChannelGroup<'a> {
    pub name: Option<&'a str>,
    pub channels: Vec<&'a Channel>,
}

#[derive(Default)]
pub struct AppState {
    pub current_user: Option<User>,
    pub guilds: Vec<Guild>,
    pub dm_channels: Vec<Channel>,
    pub relationships: Vec<Relationship>,
    pub dm_search: String,
    pub show_friends_list: bool,
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
}

const TYPING_EXPIRY: std::time::Duration = std::time::Duration::from_secs(10);

impl AppState {
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
