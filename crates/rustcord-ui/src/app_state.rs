use rustcord_client::models::{Channel, ChannelType, Emoji, Guild, Message, Presence, Relationship, RelationshipType, User};
use std::collections::HashMap;

fn merge_read_state(
    local: &mut HashMap<String, String>,
    remote: &[rustcord_client::gateway::ChannelReadState],
) -> usize {
    let mut advanced = 0;
    for state in remote {
        let is_newer = local
            .get(&state.channel_id)
            .is_none_or(|seen| snowflake_value(&state.last_message_id) > snowflake_value(seen));
        if is_newer {
            local.insert(state.channel_id.clone(), state.last_message_id.clone());
            advanced += 1;
        }
    }
    advanced
}

fn snowflake_value(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

pub struct ChannelGroup<'a> {
    pub name: Option<&'a str>,
    pub channels: Vec<(&'a Channel, bool)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum SettingsCategory {
    #[default]
    Account,
    Notifications,
    Activity,
    VoiceVideo,
    Keybinds,
    Appearance,
    About,
}

impl SettingsCategory {
    pub const ALL: [SettingsCategory; 7] = [
        SettingsCategory::Account,
        SettingsCategory::Notifications,
        SettingsCategory::Activity,
        SettingsCategory::VoiceVideo,
        SettingsCategory::Keybinds,
        SettingsCategory::Appearance,
        SettingsCategory::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsCategory::Account => "Account",
            SettingsCategory::Notifications => "Notifications",
            SettingsCategory::Activity => "Activity",
            SettingsCategory::VoiceVideo => "Voice & Video",
            SettingsCategory::Keybinds => "Keybinds",
            SettingsCategory::Appearance => "Appearance",
            SettingsCategory::About => "About",
        }
    }
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

fn config_dir() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.config_dir().to_path_buf())
}

fn groups_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("dm_groups.toml"))
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
    config_dir().map(|d| d.join("server_rail.toml"))
}

fn notes_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("notes.toml"))
}

fn read_state_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("read_state.toml"))
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
    pub guild_roles: HashMap<String, Vec<rustcord_client::models::Role>>,
    pub my_guild_roles: HashMap<String, Vec<String>>,
    pub guild_members: HashMap<String, Vec<rustcord_client::models::GuildMember>>,
    pub guild_members_requested: std::collections::HashSet<String>,
    pub show_members_panel: bool,
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
    pub mic_gain: f32,
    pub denoise: bool,
    pub user_volumes: HashMap<String, f32>,
    pub history_exhausted: std::collections::HashSet<String>,
    pub incoming_calls: Vec<String>,
    pub ringtone_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub pending_attachment: Option<std::path::PathBuf>,
    pub revealed_spoilers: std::collections::HashSet<usize>,
    pub typing_users: HashMap<String, HashMap<String, std::time::Instant>>,
    pub last_typing_sent: Option<std::time::Instant>,
    pub show_settings: bool,
    pub settings_category: SettingsCategory,
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
    pub voice_stats: rustcord_client::voice_gateway::VoiceStats,
    pub voice_history: std::collections::VecDeque<rustcord_client::voice_gateway::VoiceStats>,
    pub show_voice_info: bool,
    pub voice_joined_at: Option<std::time::Instant>,
    pub selected_input_device: Option<String>,
    pub selected_output_device: Option<String>,
    pub show_voice_device_picker: bool,
    pub rail_folders: Vec<GuildFolder>,
    pub rail_order: Vec<RailItem>,
    pub renaming_folder: Option<usize>,
    pub folder_rename_buf: String,
    pub last_read_message_id: HashMap<String, String>,
    pub join_invite_input: String,
    pub create_server_input: String,
    pub show_hidden_channels: bool,
    pub keybinds: crate::keybinds::Keybinds,
    pub rebinding_action: Option<crate::keybinds::Action>,
    pub mic_test_running: bool,
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
        acts.iter().find(|a| a.kind != 4).or_else(|| acts.first()).map(|a| a.summary_title()).filter(|s| !s.is_empty())
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

    pub fn mark_channel_read(&mut self, channel_id: &str, message_id: &str) -> bool {
        let is_newer = self
            .last_read_message_id
            .get(channel_id)
            .is_none_or(|seen| snowflake_value(message_id) > snowflake_value(seen));
        if is_newer {
            self.last_read_message_id.insert(channel_id.to_string(), message_id.to_string());
            self.save_read_state();
        }
        is_newer
    }

    pub fn apply_remote_read_state(&mut self, states: &[rustcord_client::gateway::ChannelReadState]) -> usize {
        let advanced = merge_read_state(&mut self.last_read_message_id, states);
        if advanced > 0 {
            self.save_read_state();
        }
        advanced
    }

    pub fn upsert_channel(&mut self, channel: Channel) {
        match channel.guild_id.clone() {
            Some(guild_id) => {
                let channels = self.guild_channels.entry(guild_id).or_default();
                match channels.iter_mut().find(|c| c.id == channel.id) {
                    Some(existing) => *existing = channel,
                    None => channels.push(channel),
                }
            }
            None => match self.dm_channels.iter_mut().find(|c| c.id == channel.id) {
                Some(existing) => *existing = channel,
                None => self.dm_channels.insert(0, channel),
            },
        }
    }

    pub fn remove_channel(&mut self, channel_id: &str, guild_id: Option<&str>) {
        match guild_id {
            Some(guild_id) => {
                if let Some(channels) = self.guild_channels.get_mut(guild_id) {
                    channels.retain(|c| c.id != channel_id);
                }
            }
            None => self.dm_channels.retain(|c| c.id != channel_id),
        }
        self.forget_channel(channel_id);
    }

    pub fn remove_guild(&mut self, guild_id: &str) {
        self.guilds.retain(|g| g.id != guild_id);
        if let Some(channels) = self.guild_channels.remove(guild_id) {
            for channel in channels {
                self.forget_channel(&channel.id);
            }
        }
        self.guild_roles.remove(guild_id);
        self.my_guild_roles.remove(guild_id);
        self.guild_emojis.remove(guild_id);
        self.guild_members.remove(guild_id);
        if self.selected_guild.as_deref() == Some(guild_id) {
            self.selected_guild = None;
            self.selected_channel = None;
        }
        self.reconcile_rail_order();
        self.save_rail();
    }

    fn forget_channel(&mut self, channel_id: &str) {
        self.messages.remove(channel_id);
        self.typing_users.remove(channel_id);
        self.last_read_message_id.remove(channel_id);
        if self.selected_channel.as_deref() == Some(channel_id) {
            self.selected_channel = None;
        }
    }

    pub fn update_guild(&mut self, guild_id: &str, name: Option<String>, icon: Option<String>) {
        if let Some(guild) = self.guilds.iter_mut().find(|g| g.id == guild_id) {
            if let Some(name) = name {
                guild.name = name;
            }
            guild.icon = icon;
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

    pub fn audio_settings(&self) -> crate::settings::Settings {
        crate::settings::Settings {
            mic_gain: Some(self.mic_gain),
            denoise: self.denoise,
            user_volumes: self.user_volumes.clone(),
            keybinds: self.keybinds.to_map(),
            ..Default::default()
        }
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

    pub fn ding_for_voice_state(&self, voice_state: &rustcord_client::models::VoiceState) {
        let Some((_, my_channel)) = &self.connected_voice_channel else { return };
        if self.current_user.as_ref().is_some_and(|u| u.id == voice_state.user_id) || self.self_deafened {
            return;
        }
        if self.voice_joined_at.is_none_or(|t| t.elapsed() < std::time::Duration::from_secs(2)) {
            return;
        }
        let was_in = self
            .voice_channel_states
            .values()
            .flatten()
            .any(|s| s.user_id == voice_state.user_id && s.channel_id.as_ref() == Some(my_channel));
        let is_in = voice_state.channel_id.as_ref() == Some(my_channel);
        if was_in != is_in {
            rustcord_client::voice_audio::play_ding(is_in, self.selected_output_device.clone());
        }
    }

    pub fn apply_voice_state(&mut self, voice_state: rustcord_client::models::VoiceState) {
        if let Some(user) = voice_state.member.as_ref().and_then(|m| m.user.clone()) {
            self.known_users.insert(voice_state.user_id.clone(), user);
        }
        let Some(guild_id) = voice_state.guild_id.clone().or_else(|| voice_state.channel_id.clone()) else {
            for states in self.voice_channel_states.values_mut() {
                states.retain(|s| s.user_id != voice_state.user_id);
            }
            return;
        };
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

    pub fn my_base_permissions(&self, guild_id: &str) -> u64 {
        let Some(roles) = self.guild_roles.get(guild_id) else { return 0 };
        let my_role_ids = self.my_guild_roles.get(guild_id).map(Vec::as_slice).unwrap_or(&[]);
        rustcord_client::permissions::compute_base_permissions(my_role_ids, guild_id, roles)
    }

    pub fn can_view_channel(&self, guild_id: &str, channel: &Channel) -> bool {
        let Some(me) = &self.current_user else { return true };
        if self.guild_roles.get(guild_id).is_none() {
            return true;
        }
        let my_role_ids = self.my_guild_roles.get(guild_id).map(Vec::as_slice).unwrap_or(&[]);
        let base = self.my_base_permissions(guild_id);
        let permissions = rustcord_client::permissions::compute_channel_permissions(
            base,
            &me.id,
            my_role_ids,
            guild_id,
            &channel.permission_overwrites,
        );
        rustcord_client::permissions::has_permission(permissions, rustcord_client::permissions::VIEW_CHANNEL)
    }

    pub fn grouped_channels_for_selected_guild(&self) -> Vec<ChannelGroup<'_>> {
        let Some(guild_id) = &self.selected_guild else { return Vec::new() };
        let Some(channels) = self.guild_channels.get(guild_id) else { return Vec::new() };

        let include_hidden = self.show_hidden_channels;
        let mut categories: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.kind == ChannelType::GuildCategory)
            .filter(|c| include_hidden || self.can_view_channel(guild_id, c))
            .collect();
        categories.sort_by_key(|c| c.position.unwrap_or(i32::MAX));

        let text_channels = |parent_id: Option<&str>| -> Vec<(&Channel, bool)> {
            let mut matching: Vec<(&Channel, bool)> = channels
                .iter()
                .filter(|c| matches!(c.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement))
                .filter(|c| c.parent_id.as_deref() == parent_id)
                .map(|c| (c, self.can_view_channel(guild_id, c)))
                .filter(|(_, visible)| include_hidden || *visible)
                .collect();
            matching.sort_by_key(|(c, _)| c.position.unwrap_or(i32::MAX));
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustcord_client::gateway::ChannelReadState;

    fn remote(channel_id: &str, last_message_id: &str) -> ChannelReadState {
        ChannelReadState { channel_id: channel_id.into(), last_message_id: last_message_id.into() }
    }

    fn guild_channel(id: &str, guild_id: &str, name: &str) -> Channel {
        Channel {
            id: id.into(),
            kind: ChannelType::GuildText,
            guild_id: Some(guild_id.into()),
            name: Some(name.into()),
            topic: None,
            position: Some(0),
            parent_id: None,
            recipients: Vec::new(),
            last_message_id: None,
            permission_overwrites: Vec::new(),
        }
    }

    #[test]
    fn creating_a_channel_adds_it_to_its_guild() {
        let mut state = AppState::default();
        state.upsert_channel(guild_channel("c1", "g1", "general"));
        assert_eq!(state.guild_channels["g1"].len(), 1);
    }

    #[test]
    fn updating_a_channel_replaces_it_rather_than_duplicating() {
        let mut state = AppState::default();
        state.upsert_channel(guild_channel("c1", "g1", "general"));
        state.upsert_channel(guild_channel("c1", "g1", "renamed"));
        assert_eq!(state.guild_channels["g1"].len(), 1);
        assert_eq!(state.guild_channels["g1"][0].name.as_deref(), Some("renamed"));
    }

    #[test]
    fn deleting_a_channel_clears_its_selection_and_messages() {
        let mut state = AppState::default();
        state.upsert_channel(guild_channel("c1", "g1", "general"));
        state.messages.insert("c1".into(), Vec::new());
        state.selected_channel = Some("c1".into());
        state.remove_channel("c1", Some("g1"));
        assert!(state.guild_channels["g1"].is_empty());
        assert!(!state.messages.contains_key("c1"));
        assert_eq!(state.selected_channel, None);
    }

    #[test]
    fn deleting_a_channel_leaves_an_unrelated_selection_alone() {
        let mut state = AppState::default();
        state.upsert_channel(guild_channel("c1", "g1", "general"));
        state.upsert_channel(guild_channel("c2", "g1", "other"));
        state.selected_channel = Some("c2".into());
        state.remove_channel("c1", Some("g1"));
        assert_eq!(state.selected_channel.as_deref(), Some("c2"));
    }

    #[test]
    fn leaving_a_guild_drops_its_channels_and_selection() {
        let mut state = AppState::default();
        state.guilds.push(Guild { id: "g1".into(), name: "Guild".into(), icon: None, unavailable: false });
        state.upsert_channel(guild_channel("c1", "g1", "general"));
        state.selected_guild = Some("g1".into());
        state.selected_channel = Some("c1".into());
        state.remove_guild("g1");
        assert!(state.guilds.is_empty());
        assert!(!state.guild_channels.contains_key("g1"));
        assert_eq!(state.selected_guild, None);
        assert_eq!(state.selected_channel, None);
    }

    #[test]
    fn updating_a_guild_renames_it_in_place() {
        let mut state = AppState::default();
        state.guilds.push(Guild { id: "g1".into(), name: "Old".into(), icon: None, unavailable: false });
        state.update_guild("g1", Some("New".into()), None);
        assert_eq!(state.guilds[0].name, "New");
    }

    #[test]
    fn updating_an_unknown_guild_is_a_no_op() {
        let mut state = AppState::default();
        state.update_guild("missing", Some("New".into()), None);
        assert!(state.guilds.is_empty());
    }

    #[test]
    fn a_dm_channel_upsert_does_not_need_a_guild() {
        let mut state = AppState::default();
        let mut dm = guild_channel("d1", "ignored", "dm");
        dm.guild_id = None;
        dm.kind = ChannelType::Dm;
        state.upsert_channel(dm);
        assert_eq!(state.dm_channels.len(), 1);
        assert!(state.guild_channels.is_empty());
    }

    #[test]
    fn a_newer_remote_read_advances_local() {
        let mut local = HashMap::from([("c".to_string(), "100".to_string())]);
        assert_eq!(merge_read_state(&mut local, &[remote("c", "200")]), 1);
        assert_eq!(local["c"], "200");
    }

    #[test]
    fn an_older_remote_read_does_not_regress_local() {
        let mut local = HashMap::from([("c".to_string(), "200".to_string())]);
        assert_eq!(merge_read_state(&mut local, &[remote("c", "100")]), 0);
        assert_eq!(local["c"], "200");
    }

    #[test]
    fn an_unknown_channel_is_adopted() {
        let mut local = HashMap::new();
        assert_eq!(merge_read_state(&mut local, &[remote("new", "100")]), 1);
        assert_eq!(local["new"], "100");
    }

    #[test]
    fn snowflakes_compare_numerically_not_lexically() {
        let mut local = HashMap::from([("c".to_string(), "9".to_string())]);
        assert_eq!(merge_read_state(&mut local, &[remote("c", "10")]), 1);
        assert_eq!(local["c"], "10");
    }

    #[test]
    fn merging_is_idempotent() {
        let mut local = HashMap::new();
        let states = [remote("c", "100")];
        assert_eq!(merge_read_state(&mut local, &states), 1);
        assert_eq!(merge_read_state(&mut local, &states), 0);
    }
}
