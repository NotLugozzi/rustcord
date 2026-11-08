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

fn frequently_used_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("frequently_used.toml"))
}

const FREQUENT_KEEP: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub count: u32,
    pub last_used: i64,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct FrequentlyUsed {
    #[serde(default)]
    pub emojis: HashMap<String, Usage>,
    #[serde(default)]
    pub stickers: HashMap<String, Usage>,
}

impl FrequentlyUsed {
    pub fn load() -> Self {
        let text = frequently_used_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = frequently_used_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string(self) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn record(counts: &mut HashMap<String, Usage>, key: &str, now: i64) {
        let usage = counts.entry(key.to_string()).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last_used = now;
        if counts.len() > FREQUENT_KEEP {
            let keep: std::collections::HashSet<String> = Self::ranked(counts).into_iter().take(FREQUENT_KEEP).map(str::to_string).collect();
            counts.retain(|k, _| keep.contains(k));
        }
    }

    pub fn ranked(counts: &HashMap<String, Usage>) -> Vec<&str> {
        let mut entries: Vec<(&String, &Usage)> = counts.iter().collect();
        entries.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(b.1.last_used.cmp(&a.1.last_used)).then(a.0.cmp(b.0)));
        entries.into_iter().map(|(k, _)| k.as_str()).collect()
    }
}

pub struct StickerSection<'a> {
    pub key: String,
    pub name: &'a str,
    pub stickers: Vec<&'a rustcord_client::models::Sticker>,
}

pub fn camera_video_key(user_id: &str) -> String {
    format!("camera:{user_id}")
}

pub const FREQUENT_SECTION: &str = "frequent";
const FREQUENT_STICKERS_SHOWN: usize = 12;
const FREQUENT_EMOJIS_SHOWN: usize = 24;

pub enum FrequentEmoji<'a> {
    Unicode(&'static emojis::Emoji),
    Custom(&'a Emoji),
}

fn custom_emoji_id(glyph: &str) -> Option<&str> {
    let inner = glyph.strip_prefix('<')?.strip_suffix('>')?;
    inner.rsplit(':').next().filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

pub struct ChannelEditor {
    pub guild_id: String,
    pub channel_id: Option<String>,
    pub name: String,
    pub topic: String,
    pub voice: bool,
    pub parent_id: Option<String>,
    pub focus: bool,
}

impl ChannelEditor {
    pub fn create(guild_id: String, parent_id: Option<String>) -> Self {
        Self { guild_id, channel_id: None, name: String::new(), topic: String::new(), voice: false, parent_id, focus: true }
    }

    pub fn edit(guild_id: String, channel: &Channel) -> Self {
        Self {
            guild_id,
            channel_id: Some(channel.id.clone()),
            name: channel.name.clone().unwrap_or_default(),
            topic: channel.topic.clone().unwrap_or_default(),
            voice: matches!(channel.kind, rustcord_client::models::ChannelType::GuildVoice | rustcord_client::models::ChannelType::GuildStageVoice),
            parent_id: channel.parent_id.clone(),
            focus: true,
        }
    }
}

pub struct InviteManager {
    pub guild_id: String,
    pub invites: Option<Result<Vec<rustcord_client::models::Invite>, String>>,
}

pub const MUTE_CHOICES: &[(&str, i64)] = &[
    ("For 15 minutes", 15 * 60),
    ("For 1 hour", 60 * 60),
    ("For 3 hours", 3 * 60 * 60),
    ("For 8 hours", 8 * 60 * 60),
    ("For 24 hours", 24 * 60 * 60),
    ("Until I turn it back on", -1),
];

pub fn mute_active(muted: bool, config: Option<&rustcord_client::models::MuteConfig>, now: chrono::DateTime<chrono::Utc>) -> bool {
    if !muted {
        return false;
    }
    match config.and_then(|c| c.end_time.as_deref()) {
        Some(end) => chrono::DateTime::parse_from_rfc3339(end).map_or(true, |end| end.with_timezone(&chrono::Utc) > now),
        None => true,
    }
}

pub fn mute_patch(muted: bool, window_secs: i64, now: chrono::DateTime<chrono::Utc>) -> serde_json::Value {
    let end_time = (muted && window_secs > 0).then(|| (now + chrono::Duration::seconds(window_secs)).to_rfc3339());
    rustcord_client::models::mute_fields(muted, end_time, window_secs)
}

pub const TIMEOUT_CHOICES: &[(&str, i64)] = &[
    ("60 seconds", 60),
    ("5 minutes", 5 * 60),
    ("10 minutes", 10 * 60),
    ("1 hour", 60 * 60),
    ("1 day", 24 * 60 * 60),
    ("1 week", 7 * 24 * 60 * 60),
];

pub fn active_timeout(until: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Option<chrono::DateTime<chrono::Utc>> {
    let until = chrono::DateTime::parse_from_rfc3339(until?).ok()?.with_timezone(&chrono::Utc);
    (until > now).then_some(until)
}

pub const GIF_SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

#[derive(Default)]
pub struct GifPicker {
    pub query: String,
    pub edited_at: Option<std::time::Instant>,
    pub requested: Option<String>,
    pub shown_query: Option<String>,
    pub results: Vec<rustcord_client::models::Gif>,
    pub error: Option<String>,
}

impl GifPicker {
    pub fn due_query(&self, now: std::time::Instant) -> Option<String> {
        let query = self.query.trim();
        if self.requested.as_deref() == Some(query) {
            return None;
        }
        if self.edited_at.is_some_and(|edited| now.duration_since(edited) < GIF_SEARCH_DEBOUNCE) {
            return None;
        }
        Some(query.to_string())
    }

    pub fn apply(&mut self, query: String, result: Result<Vec<rustcord_client::models::Gif>, String>) -> bool {
        if self.requested.as_deref() != Some(query.as_str()) {
            return false;
        }
        match result {
            Ok(gifs) => {
                self.results = gifs;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.shown_query = Some(query);
        true
    }

    pub fn loading(&self) -> bool {
        self.requested.is_some() && self.requested != self.shown_query
    }
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
    pub message_rows: HashMap<String, std::sync::Arc<crate::message_rows::ChannelRows>>,
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
    pub reached_start: std::collections::HashSet<String>,
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
    pub guild_stickers: HashMap<String, Vec<rustcord_client::models::Sticker>>,
    pub guild_stickers_requested: std::collections::HashSet<String>,
    pub sticker_packs: Vec<rustcord_client::models::StickerPack>,
    pub sticker_packs_requested: bool,
    pub sticker_picker_filter: String,
    pub sticker_picker_section: Option<String>,
    pub frequently_used: FrequentlyUsed,
    pub gif_picker: GifPicker,
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
    pub unwatching_stream: Option<String>,
    pub server_call_minimized: bool,
    pub server_call_focus_stream: bool,
    pub voice_speaking_users: std::collections::HashSet<String>,
    pub connected_voice_channel: Option<(Option<String>, String)>,
    pub pending_voice_channel: Option<(Option<String>, String)>,
    pub voice_connecting: bool,
    pub screen_sharing: bool,
    pub camera_on: bool,
    pub camera_device: Option<String>,
    pub cameras: Vec<(String, String)>,
    pub cameras_loaded: bool,
    pub share_system_audio: bool,
    pub screen_audio_sources: Vec<(u32, String, bool)>,
    pub screen_audio_sources_loaded: bool,
    pub screen_audio_sources_error: Option<String>,
    pub selected_screen_audio_pid: Option<u32>,
    pub screen_share_max_height: u32,
    pub screen_share_fps: u32,
    pub show_screen_share_setup: bool,
    pub self_muted: bool,
    pub self_deafened: bool,
    pub voice_input_devices: Vec<String>,
    pub voice_output_devices: Vec<String>,
    pub voice_stats: rustcord_client::voice_gateway::VoiceStats,
    pub voice_history: std::collections::VecDeque<rustcord_client::voice_gateway::VoiceStats>,
    pub screen_share_stats: rustcord_client::voice_gateway::VoiceStats,
    pub screen_share_history: std::collections::VecDeque<rustcord_client::voice_gateway::VoiceStats>,
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
    pub channel_context: Option<String>,
    pub channel_editor: Option<ChannelEditor>,
    pub invite_manager: Option<InviteManager>,
    pub guild_settings: HashMap<String, rustcord_client::models::GuildSettings>,
    pub guild_context: Option<(String, Option<usize>)>,
    pub keybinds: crate::keybinds::Keybinds,
    pub rebinding_action: Option<crate::keybinds::Action>,
    pub mic_test_running: bool,
    pub clock_24h: Option<bool>,
    pub invites: HashMap<String, crate::invites::InviteEntry>,
    pub pending_invite_voice: Option<(String, String)>,
    pub relationship_busy: std::collections::HashSet<String>,
    pub relationship_confirm: Option<(crate::backend::RelationshipAction, String, String)>,
    pub mutual_server_labels: HashMap<String, String>,
    pub muted_before_deafen: bool,
    pub drafts: HashMap<String, String>,
    pub draft_channel: Option<String>,
    pub compose_placeholder: String,
    pub uploads: Vec<PendingUpload>,
    pub next_upload_id: u64,
    pub pins: HashMap<String, PinsState>,
    pub show_profile_panel: bool,
    pub add_people_selected: std::collections::HashSet<String>,
    pub add_people_filter: String,
}

pub enum PinsState {
    Loading,
    Ready(Vec<Message>),
    Failed,
}

pub struct PendingUpload {
    pub id: u64,
    pub filename: String,
    pub progress: std::sync::Arc<rustcord_client::rest::UploadProgress>,
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

    pub fn status_line_for(&self, user_id: &str) -> Option<(Option<rustcord_client::models::ActivityEmoji>, String)> {
        let activities = &self.presences.get(user_id)?.activities;
        if let Some(custom) = activities.iter().find(|a| a.kind == 4) {
            let text = custom.state.clone().unwrap_or_default();
            if !text.is_empty() || custom.emoji.is_some() {
                return Some((custom.emoji.clone(), text));
            }
        }
        activities.iter().find(|a| a.kind != 4).map(|a| (None, a.summary_title())).filter(|(_, s)| !s.is_empty())
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
        self.message_rows.remove(&message.channel_id);
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
        self.guild_stickers.remove(guild_id);
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
        self.message_rows.remove(channel_id);
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
        self.message_rows.remove(channel_id);
        let Some(msg) = self.messages.get_mut(channel_id).and_then(|l| l.iter_mut().find(|m| m.id == message_id)) else {
            return;
        };
        apply_reaction_to(&mut std::sync::Arc::make_mut(msg).reactions, emoji, is_me, added);
    }

    pub fn clear_reactions(&mut self, channel_id: &str, message_id: &str) {
        self.message_rows.remove(channel_id);
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
        let now = chrono::Utc::now();
        if self.is_guild_muted(guild_id, now) {
            return false;
        }
        let Some(channels) = self.guild_channels.get(guild_id) else { return false };
        channels
            .iter()
            .filter(|c| matches!(c.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement))
            .any(|c| !self.is_channel_muted(Some(guild_id), &c.id, now) && self.channel_is_unread(&c.id, c.last_message_id.as_deref()))
    }

    pub fn channel_shows_unread(&self, guild_id: Option<&str>, channel_id: &str, last_message_id: Option<&str>) -> bool {
        !self.is_channel_muted(guild_id, channel_id, chrono::Utc::now()) && self.channel_is_unread(channel_id, last_message_id)
    }

    pub fn is_guild_muted(&self, guild_id: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.guild_settings.get(guild_id).is_some_and(|s| mute_active(s.muted, s.mute_config.as_ref(), now))
    }

    fn channel_parent(&self, guild_id: &str, channel_id: &str) -> Option<&str> {
        self.guild_channels.get(guild_id)?.iter().find(|c| c.id == channel_id)?.parent_id.as_deref()
    }

    fn channel_and_parent_overrides(&self, guild_id: Option<&str>, channel_id: &str) -> Vec<&rustcord_client::models::ChannelOverride> {
        let Some(settings) = self.guild_settings.get(guild_id.unwrap_or("@me")) else { return Vec::new() };
        let parent = guild_id.and_then(|g| self.channel_parent(g, channel_id));
        [Some(channel_id), parent].into_iter().flatten().filter_map(|id| settings.channel_override(id)).collect()
    }

    pub fn is_channel_muted(&self, guild_id: Option<&str>, channel_id: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.channel_and_parent_overrides(guild_id, channel_id)
            .iter()
            .any(|o| mute_active(o.muted, o.mute_config.as_ref(), now))
    }

    pub fn channel_mute_is_direct(&self, guild_id: Option<&str>, channel_id: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.guild_settings
            .get(guild_id.unwrap_or("@me"))
            .and_then(|s| s.channel_override(channel_id))
            .is_some_and(|o| mute_active(o.muted, o.mute_config.as_ref(), now))
    }

    pub fn guild_notification_level(&self, guild_id: &str) -> u8 {
        use rustcord_client::models::{NOTIFY_INHERIT, NOTIFY_MENTIONS};
        match self.guild_settings.get(guild_id).map(|s| s.message_notifications) {
            Some(level) if level != NOTIFY_INHERIT => level,
            _ => NOTIFY_MENTIONS,
        }
    }

    pub fn channel_notification_override(&self, guild_id: &str, channel_id: &str) -> u8 {
        self.guild_settings
            .get(guild_id)
            .and_then(|s| s.channel_override(channel_id))
            .map_or(rustcord_client::models::NOTIFY_INHERIT, |o| o.message_notifications)
    }

    pub fn notification_level(&self, guild_id: &str, channel_id: &str) -> u8 {
        self.channel_and_parent_overrides(Some(guild_id), channel_id)
            .iter()
            .map(|o| o.message_notifications)
            .find(|level| *level != rustcord_client::models::NOTIFY_INHERIT)
            .unwrap_or_else(|| self.guild_notification_level(guild_id))
    }

    pub fn should_notify(&self, message: &Message, now: chrono::DateTime<chrono::Utc>) -> bool {
        use rustcord_client::models::{NOTIFY_ALL, NOTIFY_MENTIONS};
        let Some(me) = &self.current_user else { return false };
        if message.author.id == me.id {
            return false;
        }
        let guild_id = message.guild_id.as_deref();
        if self.is_channel_muted(guild_id, &message.channel_id, now) {
            return false;
        }
        let Some(guild_id) = guild_id else { return true };
        if self.is_guild_muted(guild_id, now) {
            return false;
        }
        match self.notification_level(guild_id, &message.channel_id) {
            NOTIFY_ALL => true,
            NOTIFY_MENTIONS => message.mentions.iter().any(|u| u.id == me.id),
            _ => false,
        }
    }

    pub fn apply_guild_settings(&mut self, settings: Vec<rustcord_client::models::GuildSettings>) {
        for entry in settings {
            self.guild_settings.insert(entry.key().to_string(), entry);
        }
    }

    pub fn folder_has_unread(&self, folder: &GuildFolder) -> bool {
        folder.guild_ids.iter().any(|id| self.guild_has_unread(id))
    }

    pub fn set_message_history(&mut self, channel_id: String, mut messages: Vec<Message>) {
        messages.sort_by_key(|m| snowflake_value(&m.id));
        self.message_rows.remove(&channel_id);
        self.messages.insert(channel_id, messages.into_iter().map(std::sync::Arc::new).collect());
    }

    pub fn prepend_message_history(&mut self, channel_id: String, mut older: Vec<Message>) {
        older.sort_by_key(|m| snowflake_value(&m.id));
        self.message_rows.remove(&channel_id);
        let existing = self.messages.entry(channel_id).or_default();
        let mut combined: Vec<_> = older.into_iter().map(std::sync::Arc::new).collect();
        combined.append(existing);
        *existing = combined;
    }

    pub fn update_message(
        &mut self,
        channel_id: &str,
        id: &str,
        content: Option<String>,
        edited_timestamp: Option<String>,
        call: Option<rustcord_client::models::CallInfo>,
    ) {
        let Some(msg) = self.messages.get_mut(channel_id).and_then(|l| l.iter_mut().find(|m| m.id == id)) else { return };
        self.message_rows.remove(channel_id);
        let msg = std::sync::Arc::make_mut(msg);
        if let Some(content) = content {
            msg.content = content;
        }
        if edited_timestamp.is_some() {
            msg.edited_timestamp = edited_timestamp;
        }
        if call.is_some() {
            msg.call = call;
        }
    }

    pub fn delete_message(&mut self, channel_id: &str, id: &str) {
        if let Some(list) = self.messages.get_mut(channel_id) {
            list.retain(|m| m.id != id);
            self.message_rows.remove(channel_id);
        }
    }

    pub fn request_stickers(&mut self, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        if !self.sticker_packs_requested {
            self.sticker_packs_requested = true;
            let _ = cmd_tx.send(crate::backend::UiCommand::LoadStickerPacks);
        }
        let mut guild_ids: Vec<String> = self.selected_guild.iter().chain(self.guilds.iter().map(|g| &g.id)).cloned().collect();
        guild_ids.retain(|id| !self.guild_stickers.contains_key(id) && self.guild_stickers_requested.insert(id.clone()));
        if !guild_ids.is_empty() {
            let _ = cmd_tx.send(crate::backend::UiCommand::LoadGuildStickers { guild_ids });
        }
    }

    fn find_sticker(&self, id: &str) -> Option<&rustcord_client::models::Sticker> {
        self.guild_stickers
            .values()
            .flatten()
            .chain(self.sticker_packs.iter().flat_map(|p| &p.stickers))
            .find(|s| s.id == id)
    }

    pub fn sticker_sections(&self, filter: &str) -> Vec<StickerSection<'_>> {
        let filter = filter.trim().to_lowercase();
        let frequent: Vec<&rustcord_client::models::Sticker> = FrequentlyUsed::ranked(&self.frequently_used.stickers)
            .into_iter()
            .filter_map(|id| self.find_sticker(id))
            .take(FREQUENT_STICKERS_SHOWN)
            .collect();
        let selected = self.selected_guild.as_deref();
        let guilds = selected
            .and_then(|id| self.guilds.iter().find(|g| g.id == id))
            .into_iter()
            .chain(self.guilds.iter().filter(|g| Some(g.id.as_str()) != selected))
            .filter_map(|g| self.guild_stickers.get(&g.id).map(|stickers| (g.id.clone(), g.name.as_str(), stickers.iter().collect::<Vec<_>>())));
        let packs = self.sticker_packs.iter().map(|pack| (format!("pack:{}", pack.id), pack.name.as_str(), pack.stickers.iter().collect()));
        let frequent = filter.is_empty().then(|| (FREQUENT_SECTION.to_string(), "Frequently used", frequent));
        frequent
            .into_iter()
            .chain(guilds)
            .chain(packs)
            .map(|(key, name, stickers)| StickerSection {
                key,
                name,
                stickers: stickers.into_iter().filter(|s| s.available && s.matches(&filter)).collect(),
            })
            .filter(|section| !section.stickers.is_empty())
            .collect()
    }

    pub fn record_sticker_use(&mut self, sticker_id: &str) {
        FrequentlyUsed::record(&mut self.frequently_used.stickers, sticker_id, chrono::Utc::now().timestamp());
        self.frequently_used.save();
    }

    pub fn record_emoji_use(&mut self, glyph: &str) {
        FrequentlyUsed::record(&mut self.frequently_used.emojis, glyph, chrono::Utc::now().timestamp());
        self.frequently_used.save();
    }

    pub fn frequent_emojis(&self) -> Vec<FrequentEmoji<'_>> {
        let custom = self.available_emojis();
        FrequentlyUsed::ranked(&self.frequently_used.emojis)
            .into_iter()
            .filter_map(|glyph| match custom_emoji_id(glyph) {
                Some(id) => custom.iter().find(|(_, e)| e.id == id).map(|(_, e)| FrequentEmoji::Custom(e)),
                None => emojis::get(glyph).map(FrequentEmoji::Unicode),
            })
            .take(FREQUENT_EMOJIS_SHOWN)
            .collect()
    }

    pub fn delete_messages(&mut self, channel_id: &str, ids: &[String]) {
        if let Some(list) = self.messages.get_mut(channel_id) {
            list.retain(|m| !ids.contains(&m.id));
            self.message_rows.remove(channel_id);
        }
    }

    pub fn upsert_guild_member(&mut self, guild_id: &str, mut member: rustcord_client::models::GuildMember) {
        if self.current_user.as_ref().is_some_and(|me| me.id == member.user.id) {
            self.my_guild_roles.insert(guild_id.to_string(), member.roles.clone());
        }
        self.known_users.insert(member.user.id.clone(), member.user.clone());
        let Some(members) = self.guild_members.get_mut(guild_id) else { return };
        match members.iter_mut().find(|m| m.user.id == member.user.id) {
            Some(existing) => {
                if member.joined_at.is_empty() {
                    member.joined_at = std::mem::take(&mut existing.joined_at);
                }
                *existing = member;
            }
            None => members.push(member),
        }
    }

    pub fn remove_guild_member(&mut self, guild_id: &str, user_id: &str) {
        if let Some(members) = self.guild_members.get_mut(guild_id) {
            members.retain(|m| m.user.id != user_id);
        }
    }

    pub fn upsert_role(&mut self, guild_id: &str, role: rustcord_client::models::Role) {
        let roles = self.guild_roles.entry(guild_id.to_string()).or_default();
        match roles.iter_mut().find(|r| r.id == role.id) {
            Some(existing) => *existing = role,
            None => roles.push(role),
        }
    }

    pub fn remove_role(&mut self, guild_id: &str, role_id: &str) {
        if let Some(roles) = self.guild_roles.get_mut(guild_id) {
            roles.retain(|r| r.id != role_id);
        }
        if let Some(mine) = self.my_guild_roles.get_mut(guild_id) {
            mine.retain(|r| r != role_id);
        }
        if let Some(members) = self.guild_members.get_mut(guild_id) {
            for member in members {
                member.roles.retain(|r| r != role_id);
            }
        }
    }

    pub fn upsert_relationship(&mut self, relationship: Relationship) {
        self.known_users.insert(relationship.user.id.clone(), relationship.user.clone());
        match self.relationships.iter_mut().find(|r| r.id == relationship.id) {
            Some(existing) => *existing = relationship,
            None => self.relationships.push(relationship),
        }
    }

    pub fn remove_relationship(&mut self, user_id: &str) {
        self.relationships.retain(|r| r.id != user_id);
    }

    pub fn ensure_invite(&mut self, code: &str, cmd_tx: &tokio::sync::mpsc::UnboundedSender<crate::backend::UiCommand>) {
        let now = std::time::Instant::now();
        if self.invites.get(code).is_some_and(|entry| !entry.needs_fetch(now)) {
            return;
        }
        self.invites.insert(
            code.to_string(),
            crate::invites::InviteEntry { state: crate::invites::InviteState::Loading, updated: now },
        );
        let _ = cmd_tx.send(crate::backend::UiCommand::ResolveInvite { code: code.to_string() });
    }

    pub fn add_joined_guild(&mut self, guild: Guild) {
        if !self.guilds.iter().any(|g| g.id == guild.id) {
            self.guilds.push(guild);
            self.reconcile_rail_order();
        }
    }

    pub fn sync_draft_channel(&mut self) {
        if self.draft_channel == self.selected_channel {
            return;
        }
        if let Some((_, draft)) = self.editing.take() {
            self.compose_buf = draft;
        }
        let current = std::mem::take(&mut self.compose_buf);
        if let Some(previous) = self.draft_channel.take() {
            if current.is_empty() {
                self.drafts.remove(&previous);
            } else {
                self.drafts.insert(previous, current);
            }
        }
        self.compose_buf = self.selected_channel.as_ref().and_then(|id| self.drafts.remove(id)).unwrap_or_default();
        self.reply_target = None;
        self.draft_channel = self.selected_channel.clone();
        self.compose_placeholder = self.placeholder_for_selected();
    }

    fn placeholder_for_selected(&self) -> String {
        let Some(channel_id) = self.selected_channel.as_deref() else { return String::new() };
        if let Some(dm) = self.dm_channels.iter().find(|c| c.id == channel_id) {
            return match (dm.kind, dm.recipients.first()) {
                (ChannelType::Dm, Some(user)) => {
                    format!("Message @{}", user.global_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&user.username))
                }
                _ => format!("Message {}", Self::dm_display_name(dm)),
            };
        }
        match self.guild_channels.values().flatten().find(|c| c.id == channel_id).and_then(|c| c.name.as_deref()) {
            Some(name) => format!("Message #{name}"),
            None => "Message".to_string(),
        }
    }

    pub fn start_upload(&mut self, filename: String) -> (u64, std::sync::Arc<rustcord_client::rest::UploadProgress>) {
        self.next_upload_id += 1;
        let progress = std::sync::Arc::new(rustcord_client::rest::UploadProgress::default());
        self.uploads.push(PendingUpload { id: self.next_upload_id, filename, progress: progress.clone() });
        (self.next_upload_id, progress)
    }

    pub fn toggle_mute(&mut self) {
        if self.self_deafened {
            self.self_deafened = false;
            self.self_muted = false;
        } else {
            self.self_muted = !self.self_muted;
        }
    }

    pub fn toggle_deafen(&mut self) {
        if self.self_deafened {
            self.self_deafened = false;
            self.self_muted = self.muted_before_deafen;
        } else {
            self.muted_before_deafen = self.self_muted;
            self.self_deafened = true;
            self.self_muted = true;
        }
    }

    pub fn relationship_with(&self, user_id: &str) -> Option<RelationshipType> {
        self.relationships.iter().find(|r| r.id == user_id || r.user.id == user_id).map(|r| r.kind)
    }

    pub fn note_history_page(&mut self, channel_id: &str, fetched: usize) {
        if fetched < crate::backend::HISTORY_PAGE_SIZE as usize {
            self.reached_start.insert(channel_id.to_string());
        } else {
            self.reached_start.remove(channel_id);
        }
    }

    pub fn rows_for(&mut self, channel_id: &str) -> std::sync::Arc<crate::message_rows::ChannelRows> {
        let today = crate::timefmt::local_today();
        let format = crate::timefmt::TimeFormat::resolve(self.clock_24h);
        if let Some(rows) = self.message_rows.get(channel_id).filter(|rows| !rows.is_stale(today, format)) {
            return rows.clone();
        }
        let messages = self.messages.get(channel_id).map(Vec::as_slice).unwrap_or(&[]);
        let me = self.current_user.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
        let peer = self
            .dm_channels
            .iter()
            .find(|c| c.id == channel_id && c.kind == ChannelType::Dm)
            .and_then(|c| c.recipients.first())
            .map(|u| u.global_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&u.username));
        let rows = std::sync::Arc::new(crate::message_rows::ChannelRows::build(messages, &chrono::Local, today, format, me, peer));
        self.message_rows.insert(channel_id.to_string(), rows.clone());
        rows
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
        if dm.kind == ChannelType::GroupDm {
            if let Some(name) = dm.name.as_deref().filter(|name| !name.trim().is_empty()) {
                return name.to_string();
            }
            let participants = dm.recipients.iter()
                .map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()))
                .collect::<Vec<_>>();
            if !participants.is_empty() {
                return participants.join(", ");
            }
        }
        dm.recipients
            .first()
            .map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()))
            .or_else(|| dm.name.clone().filter(|name| !name.trim().is_empty()))
            .unwrap_or_else(|| dm.id.clone())
    }

    pub fn filtered_dm_channels(&self) -> Vec<&Channel> {
        let query = self.dm_search.trim().to_lowercase();
        let mut dms: Vec<&Channel> = self
            .dm_channels
            .iter()
            .filter(|dm| {
                query.is_empty()
                    || Self::dm_display_name(dm).to_lowercase().contains(&query)
                    || dm.recipients.iter().any(|u| {
                        u.username.to_lowercase().contains(&query)
                            || u.global_name.as_deref().is_some_and(|name| name.to_lowercase().contains(&query))
                    })
            })
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
            clock_24h: self.clock_24h,
            self_mute: self.self_muted,
            self_deaf: self.self_deafened,
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
                if self.unwatching_stream.as_deref().is_some_and(|key| key.ends_with(&format!(":{}", voice_state.user_id))) {
                    self.unwatching_stream = None;
                }
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
        if self.current_user.is_none() || self.guild_roles.get(guild_id).is_none() {
            return true;
        }
        self.has_channel_permission(guild_id, channel, rustcord_client::permissions::VIEW_CHANNEL)
    }

    pub fn is_guild_owner(&self, guild_id: &str) -> bool {
        let Some(me) = &self.current_user else { return false };
        self.guilds.iter().any(|g| g.id == guild_id && g.owner_id.as_deref() == Some(me.id.as_str()))
    }

    pub fn has_guild_permission(&self, guild_id: &str, bit: u64) -> bool {
        self.is_guild_owner(guild_id) || rustcord_client::permissions::has_permission(self.my_base_permissions(guild_id), bit)
    }

    pub fn has_channel_permission(&self, guild_id: &str, channel: &Channel, bit: u64) -> bool {
        let Some(me) = &self.current_user else { return false };
        if self.is_guild_owner(guild_id) {
            return true;
        }
        let my_role_ids = self.my_guild_roles.get(guild_id).map(Vec::as_slice).unwrap_or(&[]);
        let permissions = rustcord_client::permissions::compute_channel_permissions(
            self.my_base_permissions(guild_id),
            &me.id,
            my_role_ids,
            guild_id,
            &channel.permission_overwrites,
        );
        rustcord_client::permissions::has_permission(permissions, bit)
    }

    pub fn assignable_roles(&self, guild_id: &str) -> Vec<&rustcord_client::models::Role> {
        let Some(roles) = self.guild_roles.get(guild_id) else { return Vec::new() };
        let owner = self.is_guild_owner(guild_id);
        let mine = self.my_guild_roles.get(guild_id).map(Vec::as_slice).unwrap_or(&[]);
        let my_top = roles.iter().filter(|r| mine.contains(&r.id)).map(|r| r.position).max().unwrap_or(0);
        let mut assignable: Vec<_> = roles
            .iter()
            .filter(|r| r.id != guild_id && !r.managed && (owner || r.position < my_top))
            .collect();
        assignable.sort_by(|a, b| b.position.cmp(&a.position));
        assignable
    }

    pub fn set_member_role(&mut self, guild_id: &str, user_id: &str, role_id: &str, add: bool) {
        let Some(member) = self.guild_members.get_mut(guild_id).and_then(|m| m.iter_mut().find(|m| m.user.id == user_id)) else {
            return;
        };
        member.roles.retain(|r| r != role_id);
        if add {
            member.roles.push(role_id.to_string());
        }
        if self.current_user.as_ref().is_some_and(|me| me.id == user_id) {
            self.my_guild_roles.insert(guild_id.to_string(), member.roles.clone());
        }
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
            primary_guild: None,
        }
    }

    fn test_member(id: &str, username: &str, nick: Option<&str>) -> rustcord_client::models::GuildMember {
        rustcord_client::models::GuildMember {
            user: test_user(id, username, None),
            nick: nick.map(str::to_string),
            roles: Vec::new(),
            joined_at: "2026-01-01T00:00:00+00:00".into(),
            communication_disabled_until: None,
        }
    }

    fn test_role(id: &str, permissions: u64) -> rustcord_client::models::Role {
        rustcord_client::models::Role { id: id.into(), name: id.into(), permissions, position: 1, color: 0, managed: false }
    }

    #[test]
    fn a_role_update_changes_channel_visibility_live() {
        let view = rustcord_client::permissions::VIEW_CHANNEL;
        let mut state = AppState { current_user: Some(test_user("me", "me", None)), ..Default::default() };
        state.guild_roles.insert("g".into(), vec![test_role("g", view)]);
        let channel = test_channel("c", "g");
        assert!(state.can_view_channel("g", &channel));
        state.upsert_role("g", test_role("g", 0));
        assert!(!state.can_view_channel("g", &channel));
        state.upsert_role("g", test_role("g", view));
        assert!(state.can_view_channel("g", &channel));
    }

    #[test]
    fn my_own_member_update_replaces_my_roles() {
        let mut state = AppState { current_user: Some(test_user("me", "me", None)), ..Default::default() };
        state.my_guild_roles.insert("g".into(), vec!["old".into()]);
        let mut me = test_member("me", "me", None);
        me.roles = vec!["new".into()];
        state.upsert_guild_member("g", me);
        assert_eq!(state.my_guild_roles["g"], ["new"]);
    }

    #[test]
    fn member_update_keeps_joined_at_when_the_event_omits_it() {
        let mut state = AppState::default();
        state.guild_members.insert("g".into(), vec![test_member("1", "ada", None)]);
        let mut update = test_member("1", "ada", Some("Countess"));
        update.joined_at = String::new();
        state.upsert_guild_member("g", update);
        let members = &state.guild_members["g"];
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].nick.as_deref(), Some("Countess"));
        assert_eq!(members[0].joined_at, "2026-01-01T00:00:00+00:00");
    }

    #[test]
    fn member_events_do_not_create_an_unloaded_member_list() {
        let mut state = AppState::default();
        state.upsert_guild_member("g", test_member("1", "ada", None));
        assert!(!state.guild_members.contains_key("g"));
    }

    #[test]
    fn deleting_a_role_strips_it_from_everyone() {
        let mut state = AppState::default();
        state.guild_roles.insert("g".into(), vec![test_role("r", 0), test_role("s", 0)]);
        state.my_guild_roles.insert("g".into(), vec!["r".into(), "s".into()]);
        let mut member = test_member("1", "ada", None);
        member.roles = vec!["r".into()];
        state.guild_members.insert("g".into(), vec![member]);
        state.remove_role("g", "r");
        assert_eq!(state.guild_roles["g"].len(), 1);
        assert_eq!(state.my_guild_roles["g"], ["s"]);
        assert!(state.guild_members["g"][0].roles.is_empty());
    }

    #[test]
    fn relationships_upsert_by_id_and_remove() {
        use rustcord_client::models::{Relationship, RelationshipType};
        let mut state = AppState::default();
        let request = Relationship { id: "1".into(), kind: RelationshipType::IncomingRequest, user: test_user("1", "ada", None), since: None };
        state.upsert_relationship(request.clone());
        state.upsert_relationship(Relationship { kind: RelationshipType::Friend, ..request });
        assert_eq!(state.relationships.len(), 1);
        assert_eq!(state.relationships[0].kind, RelationshipType::Friend);
        state.remove_relationship("1");
        assert!(state.relationships.is_empty());
    }

    fn settings(json: serde_json::Value) -> rustcord_client::models::GuildSettings {
        serde_json::from_value(json).unwrap()
    }

    fn incoming(guild_id: Option<&str>, channel_id: &str, mentions_me: bool) -> Message {
        let mentions = if mentions_me { serde_json::json!([{ "id": "me", "username": "me", "discriminator": "0" }]) } else { serde_json::json!([]) };
        serde_json::from_value(serde_json::json!({
            "id": "1", "channel_id": channel_id, "guild_id": guild_id,
            "author": { "id": "them", "username": "them", "discriminator": "0" },
            "content": "hi", "timestamp": "2026-11-08T12:00:00+00:00", "edited_timestamp": null,
            "mentions": mentions
        }))
        .unwrap()
    }

    fn noon() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-11-08T12:00:00+00:00").unwrap().with_timezone(&chrono::Utc)
    }

    fn notifying_state() -> AppState {
        let mut state = AppState { current_user: Some(test_user("me", "me", None)), ..Default::default() };
        let mut child = test_channel("c", "general");
        child.parent_id = Some("cat".into());
        state.guild_channels.insert("g".into(), vec![child]);
        state
    }

    #[test]
    fn without_settings_guilds_notify_on_mentions_and_dms_always() {
        let state = notifying_state();
        assert!(state.should_notify(&incoming(None, "dm", false), noon()));
        assert!(!state.should_notify(&incoming(Some("g"), "c", false), noon()));
        assert!(state.should_notify(&incoming(Some("g"), "c", true), noon()));
    }

    #[test]
    fn a_muted_guild_is_silent_until_the_mute_expires() {
        let mut state = notifying_state();
        state.apply_guild_settings(vec![settings(serde_json::json!({
            "guild_id": "g", "muted": true, "mute_config": { "end_time": "2026-11-08T13:00:00+00:00", "selected_time_window": 3600 }
        }))]);
        assert!(!state.should_notify(&incoming(Some("g"), "c", true), noon()));
        let later = noon() + chrono::Duration::hours(2);
        assert!(state.should_notify(&incoming(Some("g"), "c", true), later));
    }

    #[test]
    fn a_category_mute_or_level_applies_to_its_channels() {
        let mut state = notifying_state();
        state.apply_guild_settings(vec![settings(serde_json::json!({
            "guild_id": "g", "message_notifications": 2,
            "channel_overrides": [{ "channel_id": "cat", "message_notifications": 0 }]
        }))]);
        assert!(state.should_notify(&incoming(Some("g"), "c", false), noon()));
        state.apply_guild_settings(vec![settings(serde_json::json!({
            "guild_id": "g", "channel_overrides": [{ "channel_id": "cat", "muted": true }]
        }))]);
        assert!(state.is_channel_muted(Some("g"), "c", noon()));
        assert!(!state.channel_mute_is_direct(Some("g"), "c", noon()));
        assert!(!state.should_notify(&incoming(Some("g"), "c", true), noon()));
    }

    #[test]
    fn a_muted_dm_is_silent_and_shows_no_unread() {
        let mut state = notifying_state();
        state.apply_guild_settings(vec![settings(serde_json::json!({
            "guild_id": null, "channel_overrides": [{ "channel_id": "dm", "muted": true }]
        }))]);
        assert!(!state.should_notify(&incoming(None, "dm", false), noon()));
        assert!(!state.channel_shows_unread(None, "dm", Some("5")));
        assert!(state.channel_shows_unread(None, "other", Some("5")));
    }

    #[test]
    fn a_muted_guild_shows_no_unread_dot() {
        let mut state = notifying_state();
        state.guild_channels.get_mut("g").unwrap()[0].last_message_id = Some("5".into());
        assert!(state.guild_has_unread("g"));
        state.apply_guild_settings(vec![settings(serde_json::json!({ "guild_id": "g", "muted": true }))]);
        assert!(!state.guild_has_unread("g"));
    }

    #[test]
    fn a_timed_mute_patch_ends_after_the_window_and_a_permanent_one_has_no_end() {
        let timed = mute_patch(true, 3600, noon());
        assert_eq!(timed["mute_config"]["end_time"], "2026-11-08T13:00:00+00:00");
        let forever = mute_patch(true, -1, noon());
        assert!(forever["mute_config"]["end_time"].is_null());
        assert!(mute_patch(false, -1, noon())["mute_config"].is_null());
    }

    fn ranked_role(id: &str, position: i32, managed: bool) -> rustcord_client::models::Role {
        rustcord_client::models::Role { id: id.into(), name: id.into(), permissions: 0, position, color: 0, managed }
    }

    fn moderator_state(owner: bool) -> AppState {
        let mut state = AppState { current_user: Some(test_user("me", "me", None)), ..Default::default() };
        let mut guild = test_guild("g", "Home");
        guild.owner_id = owner.then(|| "me".to_string());
        state.guilds.push(guild);
        state.guild_roles.insert(
            "g".into(),
            vec![ranked_role("g", 0, false), ranked_role("low", 1, false), ranked_role("bot", 2, true), ranked_role("mod", 3, false), ranked_role("admin", 4, false)],
        );
        state.my_guild_roles.insert("g".into(), vec!["mod".into()]);
        state
    }

    #[test]
    fn only_roles_below_my_top_role_are_assignable() {
        let state = moderator_state(false);
        let ids: Vec<_> = state.assignable_roles("g").iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["low"]);
    }

    #[test]
    fn the_owner_can_assign_every_unmanaged_role() {
        let state = moderator_state(true);
        let ids: Vec<_> = state.assignable_roles("g").iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["admin", "mod", "low"]);
        assert!(state.has_guild_permission("g", rustcord_client::permissions::MANAGE_ROLES));
    }

    #[test]
    fn toggling_a_role_updates_the_member_and_my_roles() {
        let mut state = moderator_state(false);
        state.guild_members.insert("g".into(), vec![test_member("me", "me", None)]);
        state.set_member_role("g", "me", "low", true);
        state.set_member_role("g", "me", "low", true);
        assert_eq!(state.guild_members["g"][0].roles, ["low"]);
        assert_eq!(state.my_guild_roles["g"], ["low"]);
        state.set_member_role("g", "me", "low", false);
        assert!(state.guild_members["g"][0].roles.is_empty());
    }

    #[test]
    fn a_timeout_is_active_only_until_it_expires() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-08T12:00:00+00:00").unwrap().with_timezone(&chrono::Utc);
        assert!(active_timeout(Some("2026-11-08T12:05:00+00:00"), now).is_some());
        assert!(active_timeout(Some("2026-11-08T11:55:00+00:00"), now).is_none());
        assert!(active_timeout(Some("garbage"), now).is_none());
        assert!(active_timeout(None, now).is_none());
    }

    fn test_gif(url: &str) -> rustcord_client::models::Gif {
        serde_json::from_value(serde_json::json!({ "url": url, "src": url })).unwrap()
    }

    #[test]
    fn gif_search_waits_for_typing_to_settle() {
        let start = std::time::Instant::now();
        let mut picker = GifPicker::default();
        assert_eq!(picker.due_query(start).as_deref(), Some(""));
        picker.requested = Some(String::new());
        assert_eq!(picker.due_query(start), None);
        picker.query = "cat ".into();
        picker.edited_at = Some(start);
        assert_eq!(picker.due_query(start + GIF_SEARCH_DEBOUNCE / 2), None);
        assert_eq!(picker.due_query(start + GIF_SEARCH_DEBOUNCE).as_deref(), Some("cat"));
    }

    #[test]
    fn a_stale_gif_result_is_dropped() {
        let mut picker = GifPicker { requested: Some("dog".into()), ..Default::default() };
        assert!(picker.loading());
        assert!(!picker.apply("do".into(), Ok(vec![test_gif("a")])));
        assert!(picker.results.is_empty());
        assert!(picker.apply("dog".into(), Ok(vec![test_gif("b")])));
        assert_eq!(picker.results[0].url, "b");
        assert!(!picker.loading());
    }

    #[test]
    fn a_failed_gif_search_is_not_retried_every_frame() {
        let mut picker = GifPicker { requested: Some("dog".into()), query: "dog".into(), ..Default::default() };
        picker.apply("dog".into(), Err("boom".into()));
        assert_eq!(picker.error.as_deref(), Some("boom"));
        assert_eq!(picker.due_query(std::time::Instant::now()), None);
    }

    fn test_sticker(id: &str, name: &str, available: bool) -> rustcord_client::models::Sticker {
        serde_json::from_value(serde_json::json!({ "id": id, "name": name, "format_type": 1, "available": available })).unwrap()
    }

    fn section_names<'a>(sections: &'a [StickerSection<'a>]) -> Vec<(&'a str, usize)> {
        sections.iter().map(|s| (s.name, s.stickers.len())).collect()
    }

    #[test]
    fn sticker_sections_put_the_open_server_first_then_other_servers_then_packs() {
        let mut state = AppState { selected_guild: Some("g".into()), ..Default::default() };
        state.guilds.push(test_guild("o", "Other"));
        state.guilds.push(test_guild("g", "Home"));
        state.guilds.push(test_guild("e", "Empty"));
        state.guild_stickers.insert("g".into(), vec![test_sticker("1", "cat", true), test_sticker("2", "cat gone", false)]);
        state.guild_stickers.insert("o".into(), vec![test_sticker("4", "dog", true)]);
        state.guild_stickers.insert("e".into(), Vec::new());
        state.sticker_packs = serde_json::from_value(serde_json::json!([
            { "id": "p", "name": "Wumpus", "stickers": [{ "id": "3", "name": "wave", "format_type": 3 }] }
        ]))
        .unwrap();
        let all = state.sticker_sections("");
        assert_eq!(section_names(&all), [("Home", 1), ("Other", 1), ("Wumpus", 1)]);
        assert_eq!(all.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["g", "o", "pack:p"]);
        let cats = state.sticker_sections(" CAT ");
        assert_eq!(section_names(&cats), [("Home", 1)]);
    }

    #[test]
    fn frequently_used_stickers_lead_the_picker_but_not_a_search() {
        let mut state = AppState::default();
        state.guilds.push(test_guild("g", "Home"));
        state.guild_stickers.insert("g".into(), vec![test_sticker("1", "cat", true), test_sticker("2", "dog", true)]);
        state.record_sticker_use("2");
        state.record_sticker_use("2");
        state.record_sticker_use("1");
        state.record_sticker_use("gone");
        let all = state.sticker_sections("");
        assert_eq!(all[0].key, FREQUENT_SECTION);
        assert_eq!(all[0].stickers.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["2", "1"]);
        assert!(state.sticker_sections("cat").iter().all(|s| s.key != FREQUENT_SECTION));
    }

    #[test]
    fn usage_ranks_by_count_then_recency_and_stays_bounded() {
        let mut counts = HashMap::new();
        FrequentlyUsed::record(&mut counts, "a", 1);
        FrequentlyUsed::record(&mut counts, "b", 2);
        FrequentlyUsed::record(&mut counts, "a", 3);
        FrequentlyUsed::record(&mut counts, "c", 4);
        assert_eq!(FrequentlyUsed::ranked(&counts), ["a", "c", "b"]);
        for i in 0..FREQUENT_KEEP + 10 {
            FrequentlyUsed::record(&mut counts, &i.to_string(), 10 + i as i64);
        }
        assert_eq!(counts.len(), FREQUENT_KEEP);
        assert!(counts.contains_key("a"));
    }

    #[test]
    fn usage_survives_a_toml_round_trip_with_custom_emoji_keys() {
        let mut used = FrequentlyUsed::default();
        FrequentlyUsed::record(&mut used.emojis, "<a:blob:123>", 5);
        FrequentlyUsed::record(&mut used.emojis, "\u{1F600}", 6);
        let back: FrequentlyUsed = toml::from_str(&toml::to_string(&used).unwrap()).unwrap();
        assert_eq!(back.emojis, used.emojis);
    }

    #[test]
    fn frequent_emojis_drop_custom_ones_you_can_no_longer_use() {
        let mut state = AppState::default();
        state.guilds.push(test_guild("g", "Home"));
        state.guild_emojis.insert("g".into(), vec![serde_json::from_value(serde_json::json!({ "id": "7", "name": "blob" })).unwrap()]);
        state.record_emoji_use("<:blob:7>");
        state.record_emoji_use("<:blob:7>");
        state.record_emoji_use("<:gone:8>");
        state.record_emoji_use("\u{1F600}");
        let names: Vec<String> = state
            .frequent_emojis()
            .iter()
            .map(|e| match e {
                FrequentEmoji::Custom(e) => e.name.clone(),
                FrequentEmoji::Unicode(e) => e.as_str().to_string(),
            })
            .collect();
        assert_eq!(names, ["blob", "\u{1F600}"]);
    }

    #[test]
    fn stickers_are_requested_once_for_every_server_missing_them() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = AppState { selected_guild: Some("g".into()), ..Default::default() };
        state.guilds.push(test_guild("a", "A"));
        state.guilds.push(test_guild("g", "G"));
        state.guilds.push(test_guild("b", "B"));
        state.guild_stickers.insert("b".into(), Vec::new());
        state.request_stickers(&tx);
        state.request_stickers(&tx);
        let mut sent = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            sent.push(match cmd {
                crate::backend::UiCommand::LoadGuildStickers { guild_ids } => guild_ids.join(","),
                crate::backend::UiCommand::LoadStickerPacks => "packs".into(),
                _ => "other".into(),
            });
        }
        assert_eq!(sent, ["packs", "g,a"]);
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
    fn switching_channels_keeps_each_draft() {
        let mut state = AppState::default();
        state.selected_channel = Some("a".into());
        state.sync_draft_channel();
        state.compose_buf = "half written".into();
        state.selected_channel = Some("b".into());
        state.sync_draft_channel();
        assert_eq!(state.compose_buf, "");
        state.compose_buf = "other".into();
        state.selected_channel = Some("a".into());
        state.sync_draft_channel();
        assert_eq!(state.compose_buf, "half written");
        state.selected_channel = Some("b".into());
        state.sync_draft_channel();
        assert_eq!(state.compose_buf, "other");
    }

    #[test]
    fn switching_away_mid_edit_keeps_the_draft_not_the_edit() {
        let mut state = AppState::default();
        state.selected_channel = Some("a".into());
        state.sync_draft_channel();
        state.compose_buf = "editing text".into();
        state.editing = Some(("m1".into(), "my draft".into()));
        state.selected_channel = Some("b".into());
        state.sync_draft_channel();
        assert!(state.editing.is_none());
        state.selected_channel = Some("a".into());
        state.sync_draft_channel();
        assert_eq!(state.compose_buf, "my draft");
    }

    #[test]
    fn dm_placeholders_use_the_display_name() {
        let mut state = AppState::default();
        let mut dm = test_channel("dm", "dm");
        dm.kind = ChannelType::Dm;
        dm.recipients = vec![test_user("2", "olbiaphleet", Some("lunatest"))];
        state.dm_channels.push(dm);
        state.selected_channel = Some("dm".into());
        state.sync_draft_channel();
        assert_eq!(state.compose_placeholder, "Message @lunatest");
    }

    #[test]
    fn deafening_mutes_and_undeafening_restores_the_previous_mute() {
        let mut state = AppState::default();
        state.toggle_deafen();
        assert!(state.self_deafened && state.self_muted);
        state.toggle_deafen();
        assert!(!state.self_deafened && !state.self_muted);

        state.toggle_mute();
        state.toggle_deafen();
        state.toggle_deafen();
        assert!(state.self_muted && !state.self_deafened);
    }

    #[test]
    fn unmuting_while_deafened_undeafens_too() {
        let mut state = AppState::default();
        state.toggle_deafen();
        state.toggle_mute();
        assert!(!state.self_muted && !state.self_deafened);
    }

    #[test]
    fn mute_state_round_trips_through_settings() {
        let mut state = AppState::default();
        state.toggle_deafen();
        let saved = state.audio_settings();
        assert!(saved.self_mute && saved.self_deaf);
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
            icon: None,
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
            icon: None,
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
        state.guilds.push(Guild { id: "g1".into(), name: "Guild".into(), icon: None, unavailable: false, owner_id: None });
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
        state.guilds.push(Guild { id: "g1".into(), name: "Old".into(), icon: None, unavailable: false, owner_id: None });
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
        rustcord_client::models::Role { id: id.into(), name: name.into(), permissions: 0, position, color, managed: false }
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
        rustcord_client::models::Guild { id: id.into(), name: name.into(), icon: None, unavailable: false, owner_id: None }
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
                primary_guild: None,
            }],
            last_message_id: None,
            icon: None,
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
