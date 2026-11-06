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

pub struct SearchState {
    pub scope: crate::backend::SearchScope,
    pub query: String,
    pub filters: crate::backend::SearchFilters,
    pub offset: u32,
    pub total: u32,
    pub results: Vec<Message>,
    pub status: SearchStatus,
}

#[derive(PartialEq)]
pub enum SearchStatus {
    Loading,
    Ready,
    Indexing { retry_after: f64 },
    Failed(String),
}

impl SearchState {
    pub fn page(&self) -> u32 {
        self.offset / rustcord_client::rest::SEARCH_PAGE_SIZE
    }

    pub fn pages(&self) -> u32 {
        self.total.div_ceil(rustcord_client::rest::SEARCH_PAGE_SIZE)
    }

    pub fn has_next_page(&self) -> bool {
        self.page() + 1 < self.pages()
    }
}

#[derive(Default)]
pub struct AppState {
    pub current_user: Option<User>,
    pub guilds: Vec<Guild>,
    pub dm_channels: Vec<Channel>,
    pub relationships: Vec<Relationship>,
    pub dm_search: String,
    pub sidebar_tab: usize,
    pub pending_sidebar_tab: Option<usize>,
    pub scroll_to_message: Option<String>,
    pub history_scroll_anchor: Option<(String, String)>,
    pub viewing_jumped_history: bool,
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
    pub messages: HashMap<String, Vec<std::sync::Arc<Message>>>,
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
    pub theme_name_input: String,
    pub theme_save_status: Option<String>,
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
    pub switcher_open: bool,
    pub switcher_focus_input: bool,
    pub switcher_query: String,
    pub switcher_selected: usize,
    pub mention_counts: HashMap<String, u32>,
    pub search_open: bool,
    pub search_follows_selection: bool,
    pub search_focus_input: bool,
    pub search_input: String,
    pub search_author_input: String,
    pub search_author: Option<String>,
    pub search_has: usize,
    pub search: Option<SearchState>,
    pub emoji_picker_filter: String,
    pub emoji_picker_target: Option<(String, String)>,
    pub emoji_picker_category: usize,
    pub emoji_autocomplete_selected: usize,
    pub mention_autocomplete_selected: usize,
    pub gateway_connected: bool,
    pub voice_channel_states: HashMap<String, Vec<rustcord_client::models::VoiceState>>,
    pub active_streams: HashMap<String, String>,
    pub watching_stream: Option<String>,
    pub voice_speaking_users: std::collections::HashSet<String>,
    pub connected_voice_channel: Option<(Option<String>, String)>,
    pub pending_voice_channel: Option<(Option<String>, String)>,
    pub voice_connecting: bool,
    pub screen_sharing: bool,
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
        list.insert(insert_at, std::sync::Arc::new(message));
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
            self.mention_counts.remove(channel_id);
            self.save_read_state();
        }
        is_newer
    }

    pub fn apply_remote_read_state(&mut self, states: &[rustcord_client::gateway::ChannelReadState]) -> usize {
        for state in states {
            match state.mention_count {
                0 => {
                    self.mention_counts.remove(&state.channel_id);
                }
                count => {
                    self.mention_counts.insert(state.channel_id.clone(), count);
                }
            }
        }
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

    pub fn apply_reaction(
        &mut self,
        channel_id: &str,
        message_id: &str,
        user_id: &str,
        emoji: rustcord_client::models::ReactionEmoji,
        added: bool,
    ) {
        let is_me = self.current_user.as_ref().is_some_and(|u| u.id == user_id);
        let Some(msg) = self.messages.get_mut(channel_id).and_then(|l| l.iter_mut().find(|m| m.id == message_id)) else {
            return;
        };
        apply_reaction_to(&mut std::sync::Arc::make_mut(msg).reactions, emoji, is_me, added);
    }

    pub fn clear_reactions(&mut self, channel_id: &str, message_id: &str) {
        if let Some(msg) = self.messages.get_mut(channel_id).and_then(|l| l.iter_mut().find(|m| m.id == message_id)) {
            std::sync::Arc::make_mut(msg).reactions.clear();
        }
    }

    pub fn switcher_entries(&self) -> Vec<crate::switcher::Entry> {
        use crate::switcher::{Entry, Target};
        use rustcord_client::models::ChannelType;

        let mut entries = Vec::new();
        for dm in &self.dm_channels {
            let icon = dm
                .recipients
                .first()
                .map(|u| {
                    rustcord_client::cdn::avatar_url(u, 64)
                        .unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(u))
                })
                .map(crate::switcher::Icon::Image)
                .unwrap_or(crate::switcher::Icon::Channel);
            entries.push(Entry {
                label: Self::dm_display_name(dm),
                context: Some("Direct Message".to_string()),
                icon,
                target: Target::Dm { channel_id: dm.id.clone() },
            });
        }
        for guild in &self.guilds {
            let icon = match rustcord_client::cdn::guild_icon_url(guild, 64) {
                Some(url) => crate::switcher::Icon::Image(url),
                None => crate::switcher::Icon::Initials(crate::switcher::initials(&guild.name)),
            };
            entries.push(Entry {
                label: guild.name.clone(),
                context: Some("Server".to_string()),
                icon,
                target: Target::Guild { guild_id: guild.id.clone() },
            });
            let Some(channels) = self.guild_channels.get(&guild.id) else { continue };
            for channel in channels {
                if !matches!(channel.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement) {
                    continue;
                }
                if !self.can_view_channel(&guild.id, channel) {
                    continue;
                }
                let Some(name) = channel.name.as_ref() else { continue };
                entries.push(Entry {
                    label: format!("#{name}"),
                    context: Some(guild.name.clone()),
                    icon: crate::switcher::Icon::Channel,
                    target: Target::Channel { guild_id: guild.id.clone(), channel_id: channel.id.clone() },
                });
            }
        }
        entries
    }

    pub fn channel_name_map(&self) -> HashMap<String, String> {
        let guild = self.guild_channels.values().flatten();
        let dm = self.dm_channels.iter();
        guild
            .chain(dm)
            .filter_map(|c| c.name.as_ref().map(|n| (c.id.clone(), n.clone())))
            .collect()
    }

    pub fn role_name_map(&self) -> HashMap<String, (String, Option<[f32; 4]>)> {
        let Some(guild_id) = self.selected_guild.as_ref() else { return HashMap::new() };
        let Some(roles) = self.guild_roles.get(guild_id) else { return HashMap::new() };
        roles.iter().map(|r| (r.id.clone(), (r.name.clone(), r.rgba()))).collect()
    }

    pub fn color_for_roles(&self, guild_id: &str, member_role_ids: &[String]) -> Option<[f32; 4]> {
        let roles = self.guild_roles.get(guild_id)?;
        roles
            .iter()
            .filter(|r| member_role_ids.iter().any(|id| id == &r.id))
            .filter(|r| r.color != 0)
            .max_by_key(|r| r.position)
            .and_then(|r| r.rgba())
    }

    pub fn member_color(&self, guild_id: &str, user_id: &str) -> Option<[f32; 4]> {
        let member = self.guild_members.get(guild_id)?.iter().find(|m| m.user.id == user_id)?;
        self.color_for_roles(guild_id, &member.roles)
    }

    pub fn seed_guild_channels(&mut self, guilds: Vec<(String, Vec<Channel>)>) -> usize {
        let mut seeded = 0;
        for (guild_id, channels) in guilds {
            if self.guild_channels.contains_key(&guild_id) {
                continue;
            }
            self.guild_channels.insert(guild_id, channels);
            seeded += 1;
        }
        seeded
    }

    pub fn mention_count(&self, channel_id: &str) -> u32 {
        self.mention_counts.get(channel_id).copied().unwrap_or(0)
    }

    pub fn guild_mention_count(&self, guild_id: &str) -> u32 {
        let Some(channels) = self.guild_channels.get(guild_id) else { return 0 };
        channels.iter().map(|c| self.mention_count(&c.id)).sum()
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
        self.messages.insert(channel_id, messages.into_iter().map(std::sync::Arc::new).collect());
    }

    pub fn prepend_message_history(&mut self, channel_id: String, mut older: Vec<Message>) {
        older.sort_by_key(|m| snowflake_value(&m.id));
        let existing = self.messages.entry(channel_id).or_default();
        let mut combined: Vec<_> = older.into_iter().map(std::sync::Arc::new).collect();
        combined.append(existing);
        *existing = combined;
    }

    pub fn last_message_timestamp(&self, channel_id: &str) -> Option<&str> {
        self.messages.get(channel_id)?.last().map(|m| m.timestamp.as_str())
    }

    pub fn active_messages(&self) -> &[std::sync::Arc<Message>] {
        self.selected_channel
            .as_ref()
            .and_then(|id| self.messages.get(id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn searchable_authors(&self, guild_id: Option<&str>) -> Vec<(String, String)> {
        let mut seen = std::collections::HashSet::new();
        let mut users: Vec<(String, String)> = Vec::new();
        let push = |id: &str, name: String, seen: &mut std::collections::HashSet<String>, out: &mut Vec<(String, String)>| {
            if seen.insert(id.to_string()) {
                out.push((id.to_string(), name));
            }
        };

        if let Some(me) = &self.current_user {
            let name = me.global_name.clone().unwrap_or_else(|| me.username.clone());
            push(&me.id, name, &mut seen, &mut users);
        }

        match guild_id {
            Some(guild_id) => {
                if let Some(members) = self.guild_members.get(guild_id) {
                    for member in members {
                        let name = member
                            .nick
                            .clone()
                            .or_else(|| member.user.global_name.clone())
                            .unwrap_or_else(|| member.user.username.clone());
                        push(&member.user.id, name, &mut seen, &mut users);
                    }
                }
            }
            None => {
                if let Some(channel_id) = &self.selected_channel {
                    if let Some(dm) = self.dm_channels.iter().find(|c| &c.id == channel_id) {
                        for user in &dm.recipients {
                            let name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
                            push(&user.id, name, &mut seen, &mut users);
                        }
                    }
                }
            }
        }

        if let Some(channel_id) = &self.selected_channel {
            if let Some(messages) = self.messages.get(channel_id) {
                for message in messages {
                    let name = message
                        .author
                        .global_name
                        .clone()
                        .unwrap_or_else(|| message.author.username.clone());
                    push(&message.author.id, name, &mut seen, &mut users);
                }
            }
        }

        users.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        users
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
        #[cfg(feature = "screenshare")]
        {
            if voice_state.channel_id.is_none() || voice_state.self_stream == Some(false) {
                self.active_streams.remove(&voice_state.user_id);
            } else if voice_state.self_stream == Some(true) {
                if let (Some(guild_id), Some(channel_id)) = (&voice_state.guild_id, &voice_state.channel_id) {
                    let stream_key = format!("guild:{guild_id}:{channel_id}:{}", voice_state.user_id);
                    self.active_streams.insert(voice_state.user_id.clone(), stream_key);
                }
            }
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


pub fn apply_reaction_to(
    reactions: &mut Vec<rustcord_client::models::Reaction>,
    emoji: rustcord_client::models::ReactionEmoji,
    is_me: bool,
    added: bool,
) {
    let key = emoji.key();
    let existing = reactions.iter().position(|r| r.emoji.key() == key);
    match (existing, added) {
        (Some(idx), true) => {
            reactions[idx].count += 1;
            if is_me {
                reactions[idx].me = true;
            }
        }
        (None, true) => reactions.push(rustcord_client::models::Reaction { emoji, count: 1, me: is_me }),
        (Some(idx), false) => {
            reactions[idx].count = reactions[idx].count.saturating_sub(1);
            if is_me {
                reactions[idx].me = false;
            }
            if reactions[idx].count == 0 {
                reactions.remove(idx);
            }
        }
        (None, false) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcord_client::gateway::ChannelReadState;

    fn search_page(total: u32, offset: u32) -> SearchState {
        SearchState {
            scope: crate::backend::SearchScope::Guild { guild_id: "g".into() },
            query: "q".into(),
            filters: Default::default(),
            offset,
            total,
            results: Vec::new(),
            status: SearchStatus::Ready,
        }
    }

    #[test]
    fn a_partial_last_page_still_counts_as_a_page() {
        let search = search_page(26, 0);
        assert_eq!(search.pages(), 2);
        assert!(search.has_next_page());
    }

    #[test]
    fn an_exactly_full_page_has_no_next_page() {
        let search = search_page(25, 0);
        assert_eq!(search.pages(), 1);
        assert!(!search.has_next_page());
    }

    #[test]
    fn the_last_page_of_many_has_no_next_page() {
        let search = search_page(60, 50);
        assert_eq!(search.page(), 2);
        assert_eq!(search.pages(), 3);
        assert!(!search.has_next_page());
    }

    #[test]
    fn a_middle_page_has_a_next_page() {
        let search = search_page(60, 25);
        assert_eq!(search.page(), 1);
        assert!(search.has_next_page());
    }

    #[test]
    fn an_empty_result_set_has_no_pages() {
        let search = search_page(0, 0);
        assert_eq!(search.pages(), 0);
        assert!(!search.has_next_page());
    }

    fn remote_with_mentions(channel_id: &str, last_message_id: &str, mention_count: u32) -> ChannelReadState {
        ChannelReadState {
            channel_id: channel_id.into(),
            last_message_id: last_message_id.into(),
            mention_count,
        }
    }

    fn test_user(id: &str, username: &str, global: Option<&str>) -> User {
        User {
            id: id.into(),
            username: username.into(),
            global_name: global.map(str::to_string),
            discriminator: "0".into(),
            avatar: None,
            bot: false,
        }
    }

    fn test_member(id: &str, username: &str, nick: Option<&str>) -> rustcord_client::models::GuildMember {
        rustcord_client::models::GuildMember {
            user: test_user(id, username, None),
            nick: nick.map(str::to_string),
            roles: Vec::new(),
            joined_at: "2026-01-01T00:00:00+00:00".into(),
        }
    }

    #[test]
    fn guild_authors_come_from_the_member_list() {
        let mut state = AppState::default();
        state.guild_members.insert("g".into(), vec![test_member("1", "ada", None)]);
        let authors = state.searchable_authors(Some("g"));
        assert!(authors.iter().any(|(id, name)| id == "1" && name == "ada"), "{authors:?}");
    }

    #[test]
    fn a_guild_nickname_wins_over_the_username() {
        let mut state = AppState::default();
        state.guild_members.insert("g".into(), vec![test_member("1", "ada", Some("Countess"))]);
        let authors = state.searchable_authors(Some("g"));
        assert_eq!(authors.iter().find(|(id, _)| id == "1").unwrap().1, "Countess");
    }

    #[test]
    fn an_author_is_never_listed_twice() {
        let mut state = AppState::default();
        state.current_user = Some(test_user("1", "me", None));
        state.guild_members.insert("g".into(), vec![test_member("1", "me", None)]);
        let authors = state.searchable_authors(Some("g"));
        assert_eq!(authors.iter().filter(|(id, _)| id == "1").count(), 1, "{authors:?}");
    }

    #[test]
    fn authors_are_sorted_case_insensitively_by_name() {
        let mut state = AppState::default();
        state.guild_members.insert(
            "g".into(),
            vec![test_member("1", "zoe", None), test_member("2", "Ada", None), test_member("3", "mike", None)],
        );
        let names: Vec<String> = state.searchable_authors(Some("g")).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["Ada", "mike", "zoe"]);
    }

    #[test]
    fn the_signed_in_user_is_always_offered_as_an_author() {
        let mut state = AppState::default();
        state.current_user = Some(test_user("me", "luna", Some("Luna")));
        let authors = state.searchable_authors(None);
        assert!(authors.iter().any(|(id, name)| id == "me" && name == "Luna"), "{authors:?}");
    }

    #[test]
    fn a_dm_offers_its_recipients() {
        let mut state = AppState::default();
        let mut dm = test_channel("dm", "dm");
        dm.kind = ChannelType::Dm;
        dm.recipients = vec![test_user("2", "friend", None)];
        state.dm_channels.push(dm);
        state.selected_channel = Some("dm".into());
        let authors = state.searchable_authors(None);
        assert!(authors.iter().any(|(id, _)| id == "2"), "{authors:?}");
    }

    #[test]
    fn a_remote_mention_count_is_adopted() {
        let mut state = AppState::default();
        state.apply_remote_read_state(&[remote_with_mentions("c", "100", 4)]);
        assert_eq!(state.mention_count("c"), 4);
    }

    #[test]
    fn a_zero_mention_count_clears_a_stale_badge() {
        let mut state = AppState::default();
        state.apply_remote_read_state(&[remote_with_mentions("c", "100", 4)]);
        state.apply_remote_read_state(&[remote_with_mentions("c", "200", 0)]);
        assert_eq!(state.mention_count("c"), 0);
    }

    #[test]
    fn reading_a_channel_clears_its_mention_badge() {
        let mut state = AppState::default();
        state.apply_remote_read_state(&[remote_with_mentions("c", "100", 7)]);
        state.mark_channel_read("c", "200");
        assert_eq!(state.mention_count("c"), 0);
    }

    #[test]
    fn an_unchanged_read_marker_leaves_the_badge_alone() {
        let mut state = AppState::default();
        state.apply_remote_read_state(&[remote_with_mentions("c", "200", 3)]);
        state.mark_channel_read("c", "100");
        assert_eq!(state.mention_count("c"), 3);
    }

    #[test]
    fn seeding_never_overwrites_channels_already_loaded() {
        let mut state = AppState::default();
        state.guild_channels.insert("g".into(), Vec::new());
        let seeded = state.seed_guild_channels(vec![("g".into(), vec![test_channel("c", "general")])]);
        assert_eq!(seeded, 0);
        assert!(state.guild_channels["g"].is_empty());
    }

    #[test]
    fn seeding_fills_in_a_guild_never_visited() {
        let mut state = AppState::default();
        let seeded = state.seed_guild_channels(vec![("g".into(), vec![test_channel("c", "general")])]);
        assert_eq!(seeded, 1);
        assert_eq!(state.guild_channels["g"].len(), 1);
    }

    fn test_channel(id: &str, name: &str) -> Channel {
        Channel {
            id: id.into(),
            kind: ChannelType::GuildText,
            guild_id: Some("g".into()),
            name: Some(name.into()),
            topic: None,
            position: Some(0),
            parent_id: None,
            recipients: Vec::new(),
            last_message_id: Some("999".into()),
            permission_overwrites: Vec::new(),
        }
    }

    fn remote(channel_id: &str, last_message_id: &str) -> ChannelReadState {
        ChannelReadState { channel_id: channel_id.into(), last_message_id: last_message_id.into(), mention_count: 0 }
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

    fn role(id: &str, name: &str, position: i32, color: u32) -> rustcord_client::models::Role {
        rustcord_client::models::Role { id: id.into(), name: name.into(), permissions: 0, position, color }
    }

    #[test]
    fn the_highest_coloured_role_wins() {
        let mut state = AppState::default();
        state.guild_roles.insert(
            "g".into(),
            vec![role("low", "Member", 1, 0x00FF00), role("high", "Admin", 5, 0xFF0000)],
        );
        let c = state.color_for_roles("g", &["low".into(), "high".into()]).expect("a colour");
        assert_eq!(c[0], 1.0, "expected red from the higher role, got {c:?}");
        assert_eq!(c[1], 0.0);
    }

    #[test]
    fn an_uncoloured_higher_role_does_not_mask_a_coloured_lower_one() {
        let mut state = AppState::default();
        state.guild_roles.insert(
            "g".into(),
            vec![role("low", "Member", 1, 0x00FF00), role("high", "Muted", 9, 0)],
        );
        let c = state.color_for_roles("g", &["low".into(), "high".into()]).expect("a colour");
        assert_eq!(c[1], 1.0, "expected the green lower role, got {c:?}");
    }

    #[test]
    fn a_member_with_no_coloured_roles_has_no_colour() {
        let mut state = AppState::default();
        state.guild_roles.insert("g".into(), vec![role("a", "Default", 0, 0)]);
        assert!(state.color_for_roles("g", &["a".into()]).is_none());
        assert!(state.color_for_roles("g", &[]).is_none());
    }

    #[test]
    fn roles_from_another_guild_are_ignored() {
        let mut state = AppState::default();
        state.guild_roles.insert("g".into(), vec![role("a", "Admin", 3, 0xFF0000)]);
        assert!(state.color_for_roles("other", &["a".into()]).is_none());
        assert!(state.color_for_roles("g", &["unknown".into()]).is_none());
    }

    #[test]
    fn role_colour_decodes_rgb_channels_in_order() {
        assert_eq!(role("a", "a", 0, 0x336699).rgba(), Some([0x33 as f32 / 255.0, 0x66 as f32 / 255.0, 0x99 as f32 / 255.0, 1.0]));
        assert_eq!(role("a", "a", 0, 0).rgba(), None, "colour 0 means inherit, not black");
    }

    #[test]
    fn the_role_name_map_carries_colours() {
        let mut state = AppState::default();
        state.selected_guild = Some("g".into());
        state.guild_roles.insert("g".into(), vec![role("r", "Mods", 2, 0xFF0000), role("p", "Plain", 1, 0)]);
        let map = state.role_name_map();
        assert_eq!(map.get("r").map(|(n, c)| (n.as_str(), c.is_some())), Some(("Mods", true)));
        assert_eq!(map.get("p").map(|(n, c)| (n.as_str(), c.is_some())), Some(("Plain", false)));
    }

    #[test]
    fn the_role_name_map_is_empty_in_a_dm() {
        let mut state = AppState::default();
        state.guild_roles.insert("g".into(), vec![role("r", "Mods", 2, 0xFF0000)]);
        state.selected_guild = None;
        assert!(state.role_name_map().is_empty());
    }

    fn unicode_emoji(name: &str) -> rustcord_client::models::ReactionEmoji {
        rustcord_client::models::ReactionEmoji { id: None, name: Some(name.into()), animated: false }
    }

    fn custom_emoji(name: &str, id: &str) -> rustcord_client::models::ReactionEmoji {
        rustcord_client::models::ReactionEmoji { id: Some(id.into()), name: Some(name.into()), animated: false }
    }

    #[test]
    fn a_first_reaction_creates_the_entry() {
        let mut reactions = Vec::new();
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), false, true);
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].count, 1);
        assert!(!reactions[0].me);
    }

    #[test]
    fn my_own_reaction_sets_the_me_flag() {
        let mut reactions = Vec::new();
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), true, true);
        assert!(reactions[0].me);
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), false, true);
        assert_eq!(reactions[0].count, 2);
        assert!(reactions[0].me, "another user reacting must not clear my flag");
    }

    #[test]
    fn removing_the_last_reaction_drops_the_entry() {
        let mut reactions = Vec::new();
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), true, true);
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), true, false);
        assert!(reactions.is_empty(), "a zero-count reaction must not linger: {reactions:?}");
    }

    #[test]
    fn removing_one_of_several_keeps_the_entry() {
        let mut reactions = Vec::new();
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), false, true);
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), true, true);
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), true, false);
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].count, 1);
        assert!(!reactions[0].me, "my flag should clear when I remove mine");
    }

    #[test]
    fn a_remove_for_an_absent_emoji_is_ignored() {
        let mut reactions = Vec::new();
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), true, false);
        assert!(reactions.is_empty());
    }

    #[test]
    fn a_count_never_underflows() {
        let mut reactions = vec![rustcord_client::models::Reaction {
            emoji: unicode_emoji("\u{1F44D}"),
            count: 0,
            me: false,
        }];
        apply_reaction_to(&mut reactions, unicode_emoji("\u{1F44D}"), false, false);
        assert!(reactions.is_empty());
    }

    #[test]
    fn custom_and_unicode_emoji_with_the_same_name_are_distinct() {
        let mut reactions = Vec::new();
        apply_reaction_to(&mut reactions, unicode_emoji("smile"), false, true);
        apply_reaction_to(&mut reactions, custom_emoji("smile", "123"), false, true);
        assert_eq!(reactions.len(), 2, "id must participate in identity: {reactions:?}");
    }

    #[test]
    fn the_rest_path_for_a_custom_emoji_includes_its_id() {
        assert_eq!(custom_emoji("smile", "123").reaction_path(), "smile:123");
        assert_eq!(unicode_emoji("\u{1F44D}").reaction_path(), "\u{1F44D}");
    }

    fn test_guild(id: &str, name: &str) -> rustcord_client::models::Guild {
        rustcord_client::models::Guild { id: id.into(), name: name.into(), icon: None, unavailable: false }
    }

    #[test]
    fn switcher_entries_cover_guilds_channels_and_dms() {
        let mut state = AppState::default();
        state.guilds.push(test_guild("g1", "My Server"));
        state.guild_channels.insert("g1".into(), vec![guild_channel("c1", "g1", "general")]);
        state.dm_channels.push(Channel {
            id: "d1".into(),
            kind: ChannelType::Dm,
            guild_id: None,
            name: None,
            topic: None,
            position: None,
            parent_id: None,
            recipients: vec![rustcord_client::models::User {
                id: "u1".into(),
                username: "friend".into(),
                global_name: None,
                discriminator: "0".into(),
                avatar: None,
                bot: false,
            }],
            last_message_id: None,
            permission_overwrites: Vec::new(),
        });

        let entries = state.switcher_entries();
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"My Server"), "{labels:?}");
        assert!(labels.contains(&"#general"), "{labels:?}");
        assert!(labels.contains(&"friend"), "{labels:?}");
    }

    #[test]
    fn switcher_skips_voice_and_category_channels() {
        let mut state = AppState::default();
        state.guilds.push(test_guild("g1", "S"));
        let mut voice = guild_channel("v1", "g1", "Lobby");
        voice.kind = ChannelType::GuildVoice;
        let mut category = guild_channel("cat1", "g1", "TEXT");
        category.kind = ChannelType::GuildCategory;
        state.guild_channels.insert("g1".into(), vec![voice, category, guild_channel("c1", "g1", "general")]);

        let labels: Vec<String> = state.switcher_entries().into_iter().map(|e| e.label).collect();
        assert!(labels.contains(&"#general".to_string()), "{labels:?}");
        assert!(!labels.iter().any(|l| l.contains("Lobby")), "voice channel leaked: {labels:?}");
        assert!(!labels.iter().any(|l| l.contains("TEXT")), "category leaked: {labels:?}");
    }

    #[test]
    fn a_channel_entry_carries_its_guild_for_switching() {
        let mut state = AppState::default();
        state.guilds.push(test_guild("g1", "S"));
        state.guild_channels.insert("g1".into(), vec![guild_channel("c1", "g1", "general")]);
        let entry = state.switcher_entries().into_iter().find(|e| e.label == "#general").expect("the channel");
        assert_eq!(
            entry.target,
            crate::switcher::Target::Channel { guild_id: "g1".into(), channel_id: "c1".into() },
            "a channel in another guild must switch guild too"
        );
    }

    #[test]
    fn a_guild_with_no_loaded_channels_still_appears() {
        let mut state = AppState::default();
        state.guilds.push(test_guild("g1", "Unloaded"));
        let labels: Vec<String> = state.switcher_entries().into_iter().map(|e| e.label).collect();
        assert_eq!(labels, vec!["Unloaded".to_string()]);
    }
}
