use crate::app_state::AppState;
use rustcord_client::models::{Channel, Guild, GuildSettings, Message, Relationship, Role, User};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const VERSION: u32 = 1;
const FILE_NAME: &str = "session.json";
const CACHED_CHANNELS: usize = 24;
const MESSAGES_PER_CHANNEL: usize = 50;
const AUTOSAVE_EVERY: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    version: u32,
    pub user: User,
    pub guilds: Vec<Guild>,
    pub guild_channels: HashMap<String, Vec<Channel>>,
    pub dm_channels: Vec<Channel>,
    pub relationships: Vec<Relationship>,
    pub guild_roles: HashMap<String, Vec<Role>>,
    pub my_guild_roles: HashMap<String, Vec<String>>,
    pub guild_settings: HashMap<String, GuildSettings>,
    pub mention_counts: HashMap<String, u32>,
    pub messages: HashMap<String, Vec<Message>>,
    pub selected_guild: Option<String>,
    pub selected_channel: Option<String>,
}

fn snowflake(id: &str) -> u64 {
    id.parse().unwrap_or(0)
}

impl Snapshot {
    pub fn capture(state: &AppState) -> Option<Self> {
        let user = state.current_user.clone()?;
        let mut recent: Vec<(&String, u64)> = state
            .messages
            .iter()
            .filter_map(|(id, list)| Some((id, snowflake(&list.last()?.id))))
            .collect();
        recent.sort_by(|a, b| b.1.cmp(&a.1));
        let mut keep: Vec<&String> = recent.into_iter().take(CACHED_CHANNELS).map(|(id, _)| id).collect();
        if let Some(selected) = state.selected_channel.as_ref().filter(|id| state.messages.contains_key(*id)) {
            if !keep.contains(&selected) {
                keep.push(selected);
            }
        }
        let messages = keep
            .into_iter()
            .map(|id| {
                let list = &state.messages[id];
                let tail = list[list.len().saturating_sub(MESSAGES_PER_CHANNEL)..].iter().map(|m| (**m).clone()).collect();
                (id.clone(), tail)
            })
            .collect();
        Some(Self {
            version: VERSION,
            user,
            guilds: state.guilds.clone(),
            guild_channels: state.guild_channels.clone(),
            dm_channels: state.dm_channels.clone(),
            relationships: state.relationships.clone(),
            guild_roles: state.guild_roles.clone(),
            my_guild_roles: state.my_guild_roles.clone(),
            guild_settings: state.guild_settings.clone(),
            mention_counts: state.mention_counts.clone(),
            messages,
            selected_guild: state.selected_guild.clone(),
            selected_channel: state.selected_channel.clone(),
        })
    }

    pub fn apply(self, state: &mut AppState) {
        state.current_user = Some(self.user);
        state.guilds = self.guilds;
        state.reconcile_rail_order();
        state.guild_channels = self.guild_channels;
        state.dm_channels = self.dm_channels;
        state.relationships = self.relationships;
        state.guild_roles = self.guild_roles;
        state.my_guild_roles = self.my_guild_roles;
        state.guild_settings = self.guild_settings;
        for (channel_id, count) in self.mention_counts {
            state.mention_counts.entry(channel_id).or_insert(count);
        }
        for (channel_id, messages) in self.messages {
            for message in &messages {
                state.known_users.entry(message.author.id.clone()).or_insert_with(|| message.author.clone());
            }
            state.set_message_history(channel_id, messages);
        }
        state.selected_guild = self.selected_guild;
        state.selected_channel = self.selected_channel;
        state.logged_in = true;
        state.restored_from_cache = true;
        state.login_status = None;
    }
}

pub fn forget_cached_account(state: &mut AppState) {
    state.messages.clear();
    state.message_rows.clear();
    state.guild_channels.clear();
    state.dm_channels.clear();
    state.relationships.clear();
    state.guild_roles.clear();
    state.my_guild_roles.clear();
    state.guild_settings.clear();
    state.mention_counts.clear();
    state.selected_guild = None;
    state.selected_channel = None;
    state.restored_from_cache = false;
}

fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|dirs| dirs.cache_dir().join(FILE_NAME))
}

fn decode(bytes: &[u8]) -> Option<Snapshot> {
    let snapshot: Snapshot = serde_json::from_slice(bytes).map_err(|e| tracing::warn!("startup cache unreadable: {e}")).ok()?;
    (snapshot.version == VERSION).then_some(snapshot)
}

pub fn load() -> Option<Snapshot> {
    let started = Instant::now();
    let bytes = std::fs::read(path()?).ok()?;
    let snapshot = decode(&bytes)?;
    tracing::info!(bytes = bytes.len(), ms = started.elapsed().as_millis() as u64, "loaded startup cache");
    Some(snapshot)
}

fn write(snapshot: &Snapshot) -> std::io::Result<()> {
    let Some(path) = path() else { return Ok(()) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&temp)?, &bytes)?;
    std::fs::rename(&temp, &path)
}

static SAVING: AtomicBool = AtomicBool::new(false);

pub fn save_now(state: &AppState) {
    if !state.logged_in || state.restored_from_cache {
        return;
    }
    if let Some(snapshot) = Snapshot::capture(state) {
        if let Err(e) = write(&snapshot) {
            tracing::warn!("could not write startup cache: {e}");
        }
    }
}

pub fn autosave(state: &AppState) {
    thread_local! {
        static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    }
    let due = LAST.with(|last| {
        let now = Instant::now();
        let due = last.get().is_some_and(|at| now.duration_since(at) >= AUTOSAVE_EVERY);
        if last.get().is_none() || due {
            last.set(Some(now));
        }
        due
    });
    if !due || !state.logged_in || state.restored_from_cache || SAVING.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(snapshot) = Snapshot::capture(state) else {
        SAVING.store(false, Ordering::Release);
        return;
    };
    std::thread::spawn(move || {
        if let Err(e) = write(&snapshot) {
            tracing::warn!("could not write startup cache: {e}");
        }
        SAVING.store(false, Ordering::Release);
    });
}

pub fn clear() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> User {
        serde_json::from_value(serde_json::json!({ "id": id, "username": id, "discriminator": "0" })).unwrap()
    }

    fn message(id: u64, channel: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id.to_string(), "channel_id": channel, "content": format!("m{id}"),
            "timestamp": "2026-11-08T12:00:00+00:00", "edited_timestamp": null,
            "author": { "id": "a", "username": "ada", "discriminator": "0" },
            "embeds": [ { "title": "link", "url": "https://example.com" } ],
            "attachments": [ { "id": "1", "filename": "a.png", "url": "https://cdn/a.png", "size": 3 } ]
        }))
        .unwrap()
    }

    fn logged_in_state() -> AppState {
        let mut state = AppState { logged_in: true, current_user: Some(user("me")), ..Default::default() };
        state.guilds = vec![serde_json::from_value(serde_json::json!({ "id": "g", "name": "Server", "icon": null })).unwrap()];
        state.guild_channels.insert("g".into(), vec![serde_json::from_value(serde_json::json!({ "id": "c", "type": 0, "guild_id": "g", "name": "general" })).unwrap()]);
        state.selected_guild = Some("g".into());
        state.selected_channel = Some("c".into());
        state.set_message_history("c".into(), (1..=80).map(|i| message(i, "c")).collect());
        state.mention_counts.insert("dm".into(), 2);
        state
    }

    #[test]
    fn a_snapshot_round_trips_through_json_and_restores_the_ui() {
        let snapshot = Snapshot::capture(&logged_in_state()).unwrap();
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let mut restored = AppState::default();
        decode(&bytes).expect("decodes").apply(&mut restored);
        assert!(restored.logged_in && restored.restored_from_cache);
        assert_eq!(restored.current_user.as_ref().map(|u| u.id.as_str()), Some("me"));
        assert_eq!(restored.selected_channel.as_deref(), Some("c"));
        assert_eq!(restored.guild_channels["g"][0].name.as_deref(), Some("general"));
        let messages = &restored.messages["c"];
        assert_eq!(messages.len(), MESSAGES_PER_CHANNEL);
        assert_eq!(messages.last().unwrap().id, "80");
        assert_eq!(messages[0].embeds.len(), 1);
        assert_eq!(restored.mention_count("dm"), 2);
        assert!(restored.known_users.contains_key("a"));
    }

    #[test]
    fn only_the_most_recent_channels_are_kept() {
        let mut state = logged_in_state();
        for i in 0..40u64 {
            let channel = format!("x{i}");
            state.set_message_history(channel.clone(), vec![message(1_000 + i, &channel)]);
        }
        state.selected_channel = Some("c".into());
        let snapshot = Snapshot::capture(&state).unwrap();
        assert_eq!(snapshot.messages.len(), CACHED_CHANNELS + 1);
        assert!(snapshot.messages.contains_key("x39") && snapshot.messages.contains_key("c"));
        assert!(!snapshot.messages.contains_key("x0"));
    }

    #[test]
    fn an_older_format_is_ignored() {
        let mut value = serde_json::to_value(Snapshot::capture(&logged_in_state()).unwrap()).unwrap();
        value["version"] = serde_json::json!(VERSION + 1);
        assert!(decode(&serde_json::to_vec(&value).unwrap()).is_none());
        assert!(decode(b"not json").is_none());
    }

    #[test]
    fn nothing_is_captured_before_login() {
        assert!(Snapshot::capture(&AppState::default()).is_none());
    }

    #[test]
    fn forgetting_the_cached_account_clears_private_data() {
        let mut state = AppState::default();
        Snapshot::capture(&logged_in_state()).unwrap().apply(&mut state);
        forget_cached_account(&mut state);
        assert!(state.messages.is_empty() && state.guild_channels.is_empty() && state.selected_channel.is_none());
        assert!(!state.restored_from_cache);
    }
}
