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

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct GuildFolder {
    pub name: String,
    pub color: [f32; 4],
    pub guild_ids: Vec<String>,
    pub expanded: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum RailItem {
    Guild(String),
    Folder(usize),
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct RailFile {
    folders: Vec<GuildFolder>,
    order: Vec<RailItem>,
}

fn rail_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.config_dir().join("server_rail.toml"))
}

fn notes_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.config_dir().join("notes.toml"))
}

fn read_state_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.config_dir().join("read_state.toml"))
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ReadStateFile {
    last_read: HashMap<String, String>,
}

pub fn load_read_state() -> HashMap<String, String> {
    let text = read_state_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    toml::from_str::<ReadStateFile>(&text).map(|f| f.last_read).unwrap_or_default()
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

pub fn load_rail() -> (Vec<GuildFolder>, Vec<RailItem>) {
    let text = rail_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let file = toml::from_str::<RailFile>(&text).unwrap_or_default();
    (file.folders, file.order)
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
    pub context_vc_member: Option<String>,
    pub profiles: HashMap<String, rustcord_client::models::UserProfile>,
    pub profiles_requested: std::collections::HashSet<String>,
    pub known_users: HashMap<String, User>,
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
    pub history_exhausted: std::collections::HashSet<String>,
    pub incoming_calls: Vec<String>,
    pub ringtone_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
    pub mention_autocomplete_selected: usize,
    pub gateway_connected: bool,
    pub voice_channel_states: HashMap<String, Vec<rustcord_client::models::VoiceState>>,
    pub voice_speaking_users: std::collections::HashSet<String>,
    pub connected_voice_channel: Option<(Option<String>, String)>,
    pub pending_voice_channel: Option<(Option<String>, String)>,
    pub voice_connecting: bool,
    pub self_muted: bool,
    pub self_deafened: bool,
    pub voice_input_devices: Vec<String>,
    pub voice_output_devices: Vec<String>,
    pub voice_bitrate_up_bps: u32,
    pub voice_bitrate_down_bps: u32,
    pub selected_input_device: Option<String>,
    pub selected_output_device: Option<String>,
    pub show_voice_device_picker: bool,
    pub rail_folders: Vec<GuildFolder>,
    pub rail_order: Vec<RailItem>,
    pub renaming_folder: Option<usize>,
    pub folder_rename_buf: String,
    pub last_read_message_id: HashMap<String, String>,
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

    pub fn save_rail(&self) {
        let Some(path) = rail_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = RailFile { folders: self.rail_folders.clone(), order: self.rail_order.clone() };
        if let Ok(text) = toml::to_string(&file) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn reconcile_rail_order(&mut self) {
        let known_guild_ids: std::collections::HashSet<String> = self.guilds.iter().map(|g| g.id.clone()).collect();

        for folder in &mut self.rail_folders {
            folder.guild_ids.retain(|id| known_guild_ids.contains(id.as_str()));
        }
        self.compact_folders();

        self.rail_order.retain(|item| match item {
            RailItem::Guild(id) => known_guild_ids.contains(id.as_str()),
            RailItem::Folder(_) => true,
        });

        let mut seen_folders = std::collections::HashSet::new();
        let mut seen_guilds = std::collections::HashSet::new();
        self.rail_order.retain(|item| match item {
            RailItem::Folder(idx) => seen_folders.insert(*idx),
            RailItem::Guild(id) => seen_guilds.insert(id.clone()),
        });

        let placed_guild_ids: std::collections::HashSet<String> = self
            .rail_order
            .iter()
            .filter_map(|item| match item {
                RailItem::Guild(id) => Some(id.clone()),
                RailItem::Folder(_) => None,
            })
            .chain(self.rail_folders.iter().flat_map(|f| f.guild_ids.iter().cloned()))
            .collect();

        for guild in &self.guilds {
            if !placed_guild_ids.contains(&guild.id) {
                self.rail_order.push(RailItem::Guild(guild.id.clone()));
            }
        }
        self.save_rail();
    }

    pub fn guild_has_active_voice(&self, guild_id: &str) -> bool {
        self.voice_channel_states.get(guild_id).is_some_and(|states| !states.is_empty())
    }

    pub fn folder_has_active_voice(&self, folder: &GuildFolder) -> bool {
        folder.guild_ids.iter().any(|id| self.guild_has_active_voice(id))
    }

    fn remove_guild_from_rail(&mut self, guild_id: &str) {
        self.rail_order.retain(|item| !matches!(item, RailItem::Guild(id) if id == guild_id));
        for folder in &mut self.rail_folders {
            folder.guild_ids.retain(|id| id != guild_id);
        }
    }

    fn compact_folders(&mut self) {
        let mut new_index = vec![None; self.rail_folders.len()];
        let mut compacted = Vec::with_capacity(self.rail_folders.len());
        for (old_idx, folder) in std::mem::take(&mut self.rail_folders).into_iter().enumerate() {
            if folder.guild_ids.is_empty() {
                continue;
            }
            new_index[old_idx] = Some(compacted.len());
            compacted.push(folder);
        }
        self.rail_folders = compacted;

        self.rail_order.retain_mut(|item| {
            if let RailItem::Folder(idx) = item {
                match new_index.get(*idx).copied().flatten() {
                    Some(new_idx) => *idx = new_idx,
                    None => return false,
                }
            }
            true
        });
        if let Some(idx) = self.renaming_folder {
            self.renaming_folder = new_index.get(idx).copied().flatten();
        }
    }

    pub fn move_guild_to_rail_index(&mut self, guild_id: &str, target_index: usize) {
        let was_before_target = self
            .rail_order
            .iter()
            .take(target_index)
            .any(|item| matches!(item, RailItem::Guild(id) if id == guild_id));
        self.remove_guild_from_rail(guild_id);
        self.compact_folders();
        let target_index = if was_before_target { target_index.saturating_sub(1) } else { target_index };
        let target_index = target_index.min(self.rail_order.len());
        self.rail_order.insert(target_index, RailItem::Guild(guild_id.to_string()));
        self.save_rail();
    }

    pub fn move_folder_to_rail_index(&mut self, folder_idx: usize, target_index: usize) {
        let Some(current_pos) = self.rail_order.iter().position(|item| matches!(item, RailItem::Folder(idx) if *idx == folder_idx)) else { return };
        let was_before_target = current_pos < target_index;
        self.rail_order.remove(current_pos);
        let target_index = if was_before_target { target_index.saturating_sub(1) } else { target_index };
        let target_index = target_index.min(self.rail_order.len());
        self.rail_order.insert(target_index, RailItem::Folder(folder_idx));
        self.save_rail();
    }

    pub fn move_guild_into_folder(&mut self, guild_id: &str, folder_idx: usize) {
        self.remove_guild_from_rail(guild_id);
        self.compact_folders();
        if let Some(folder) = self.rail_folders.get_mut(folder_idx) {
            folder.guild_ids.push(guild_id.to_string());
        }
        self.save_rail();
    }

    pub fn create_folder_from_guilds(&mut self, dragged_guild_id: &str, target_guild_id: &str) {
        self.remove_guild_from_rail(dragged_guild_id);
        self.compact_folders();
        let Some(target_pos) = self.rail_order.iter().position(|item| matches!(item, RailItem::Guild(id) if id == target_guild_id)) else { return };

        let folder = GuildFolder {
            name: "Folder".to_string(),
            color: self.theme.accent,
            guild_ids: vec![target_guild_id.to_string(), dragged_guild_id.to_string()],
            expanded: false,
        };
        self.rail_folders.push(folder);
        let folder_idx = self.rail_folders.len() - 1;
        self.rail_order[target_pos] = RailItem::Folder(folder_idx);
        self.save_rail();
    }

    pub fn remove_guild_from_folder(&mut self, folder_idx: usize, guild_id: &str) {
        let Some(folder) = self.rail_folders.get_mut(folder_idx) else { return };
        folder.guild_ids.retain(|id| id != guild_id);
        self.compact_folders();
        self.rail_order.push(RailItem::Guild(guild_id.to_string()));
        self.save_rail();
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
        self.known_users.insert(message.author.id.clone(), message.author.clone());
        if let Some(dm) = self.dm_channels.iter_mut().find(|c| c.id == message.channel_id) {
            dm.last_message_id = Some(message.id.clone());
        }
        if let Some(guild_id) = &message.guild_id {
            if let Some(channels) = self.guild_channels.get_mut(guild_id) {
                if let Some(channel) = channels.iter_mut().find(|c| c.id == message.channel_id) {
                    channel.last_message_id = Some(message.id.clone());
                }
            }
        }
        let list = self.messages.entry(message.channel_id.clone()).or_default();
        let new_id = snowflake_value(&message.id);
        let insert_at = list.partition_point(|m| snowflake_value(&m.id) <= new_id);
        list.insert(insert_at, message);
    }

    pub fn save_read_state(&self) {
        let Some(path) = read_state_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = ReadStateFile { last_read: self.last_read_message_id.clone() };
        if let Ok(text) = toml::to_string(&file) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn mark_channel_read(&mut self, channel_id: &str, message_id: &str) {
        let is_newer = self
            .last_read_message_id
            .get(channel_id)
            .is_none_or(|seen| snowflake_value(message_id) > snowflake_value(seen));
        if is_newer {
            self.last_read_message_id.insert(channel_id.to_string(), message_id.to_string());
            self.save_read_state();
        }
    }

    pub fn channel_is_unread(&self, channel_id: &str, last_message_id: Option<&str>) -> bool {
        let Some(last_message_id) = last_message_id else { return false };
        let seen = self.last_read_message_id.get(channel_id);
        match seen {
            Some(seen) => snowflake_value(last_message_id) > snowflake_value(seen),
            None => true,
        }
    }

    pub fn guild_has_unread(&self, guild_id: &str) -> bool {
        let Some(channels) = self.guild_channels.get(guild_id) else { return false };
        channels
            .iter()
            .filter(|c| matches!(c.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement))
            .any(|c| self.channel_is_unread(&c.id, c.last_message_id.as_deref()))
    }

    pub fn folder_has_unread(&self, folder: &GuildFolder) -> bool {
        folder.guild_ids.iter().any(|id| self.guild_has_unread(id))
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

    pub fn mentionable_users_for_active_channel(&self) -> Vec<&User> {
        let Some(channel_id) = &self.selected_channel else { return Vec::new() };
        let mut seen = std::collections::HashSet::new();
        let mut users = Vec::new();
        if let Some(dm) = self.dm_channels.iter().find(|c| &c.id == channel_id) {
            for user in &dm.recipients {
                if seen.insert(user.id.clone()) {
                    users.push(user);
                }
            }
        }
        if let Some(messages) = self.messages.get(channel_id) {
            for message in messages {
                if seen.insert(message.author.id.clone()) {
                    users.push(&message.author);
                }
            }
        }
        users
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

    pub fn sync_ringtone(&mut self) {
        use std::sync::atomic::{AtomicBool, Ordering};
        match (self.incoming_calls.is_empty(), self.ringtone_stop.is_some()) {
            (false, false) => {
                let stop = std::sync::Arc::new(AtomicBool::new(false));
                rustcord_client::voice_audio::play_ringtone(stop.clone());
                self.ringtone_stop = Some(stop);
            }
            (true, true) => {
                if let Some(stop) = self.ringtone_stop.take() {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }

    pub fn apply_voice_state(&mut self, voice_state: rustcord_client::models::VoiceState) {
        if let Some(user) = voice_state.member.as_ref().and_then(|m| m.user.clone()) {
            self.known_users.insert(voice_state.user_id.clone(), user);
        }
        let Some(guild_id) = voice_state.guild_id.clone().or_else(|| voice_state.channel_id.clone()) else { return };
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

    pub fn connected_voice_channel_name(&self) -> Option<String> {
        let (_, channel_id) = self.connected_voice_channel.as_ref()?;
        if let Some(dm) = self.dm_channels.iter().find(|c| &c.id == channel_id) {
            let names: Vec<String> = dm.recipients.iter().map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone())).collect();
            return Some(dm.name.clone().unwrap_or_else(|| names.join(", ")));
        }
        for channels in self.guild_channels.values() {
            if let Some(channel) = channels.iter().find(|c| &c.id == channel_id) {
                return Some(channel.name.clone().unwrap_or_else(|| channel.id.clone()));
            }
        }
        Some(channel_id.clone())
    }

    pub fn members_in_voice_channel(&self, guild_id: &str, channel_id: &str) -> Vec<&rustcord_client::models::VoiceState> {
        let Some(states) = self.voice_channel_states.get(guild_id) else { return Vec::new() };
        states.iter().filter(|s| s.channel_id.as_deref() == Some(channel_id)).collect()
    }

    pub fn voice_state_display_name(&self, voice_state: &rustcord_client::models::VoiceState) -> String {
        let name = voice_state.display_name();
        if name != voice_state.user_id {
            return name;
        }
        if let Some(user) = self.known_users.get(&voice_state.user_id) {
            return user.global_name.clone().unwrap_or_else(|| user.username.clone());
        }
        name
    }

    pub fn set_voice_speaking(&mut self, user_id: String, speaking: bool) {
        if speaking {
            self.voice_speaking_users.insert(user_id);
        } else {
            self.voice_speaking_users.remove(&user_id);
        }
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
