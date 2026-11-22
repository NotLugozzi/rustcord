use crate::app_state::AppState;
use rustcord_client::models::{Message, User};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BUNDLED: &str = include_str!("../demo.toml");
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
const ENTITY_BASE_MS: i64 = 1_577_836_800_000;
const ME: &str = "me";

#[cfg_attr(not(feature = "screenshare"), allow(dead_code))]
pub struct Feed {
    pub key: String,
    pub frame: image::RgbaImage,
}

#[cfg(feature = "screenshare")]
fn call_feeds(call: &Call, b: &Builder, state: &mut AppState) -> Result<Vec<Feed>, String> {
    let mut feeds = Vec::new();
    for (index, key) in call.cameras.iter().enumerate() {
        feeds.push(Feed { key: crate::app_state::camera_video_key(&b.user_id(key)?), frame: camera_frame(index) });
    }
    if let Some(key) = &call.watching {
        let user_id = b.user_id(key)?;
        let stream_key = state.active_streams.get(&user_id).cloned().ok_or_else(|| format!("call.watching \"{key}\" is not streaming"))?;
        state.watching_stream = Some(stream_key);
        feeds.push(Feed { key: user_id, frame: stream_frame() });
    }
    Ok(feeds)
}

#[cfg(not(feature = "screenshare"))]
fn call_feeds(_: &Call, _: &Builder, _: &mut AppState) -> Result<Vec<Feed>, String> {
    Ok(Vec::new())
}

#[cfg(feature = "screenshare")]
fn camera_frame(index: usize) -> image::RgbaImage {
    let palette = [[88u8, 101, 242], [235, 69, 158], [87, 242, 135], [254, 231, 92], [237, 66, 69]];
    let tint = palette[index % palette.len()];
    image::RgbaImage::from_fn(640, 360, |x, y| {
        let (dx, dy) = (x as f32 - 320.0, y as f32 - 170.0);
        let head = dx * dx + dy * dy < 70.0 * 70.0;
        let body = y > 250 && (x as f32 - 320.0).abs() < 150.0 - (360.0 - y as f32) * 0.4;
        let shade = 0.35 + 0.4 * (y as f32 / 360.0);
        let [r, g, b] = if head || body { [230, 200, 180] } else { tint.map(|c| (c as f32 * shade) as u8) };
        image::Rgba([r, g, b, 255])
    })
}

#[cfg(feature = "screenshare")]
fn stream_frame() -> image::RgbaImage {
    image::RgbaImage::from_fn(1280, 720, |x, y| {
        let window = (120..760).contains(&x) && (90..560).contains(&y);
        let sidebar = (800..1180).contains(&x) && (90..640).contains(&y);
        let title = window && y < 120 || sidebar && y < 120;
        let line = window && y > 150 && (y - 150) % 36 < 14 && x < 200 + (y * 7 % 480);
        let [r, g, b] = match () {
            _ if title => [60, 63, 70],
            _ if line => [120, 170, 255],
            _ if window => [30, 31, 34],
            _ if sidebar => [43, 45, 49],
            _ => [18 + (y / 24) as u8, 22, 40 + (x / 32) as u8],
        };
        image::Rgba([r, g, b, 255])
    })
}

pub enum Source {
    Bundled,
    File(PathBuf),
}

pub fn requested() -> Option<Source> {
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--demo-template" {
            print!("{BUNDLED}");
            std::process::exit(0);
        }
        if let Some(path) = arg.strip_prefix("--demo=") {
            return Some(Source::File(path.into()));
        }
        if arg == "--demo" {
            return Some(match args.next_if(|next| !next.starts_with("--")) {
                Some(path) => Source::File(path.into()),
                None => Source::Bundled,
            });
        }
    }
    None
}

pub fn enter_sandbox() {
    let real_config = crate::paths::config_dir();
    let root = std::env::temp_dir().join("rustcord-demo");
    let _ = std::fs::remove_dir_all(&root);
    crate::paths::sandbox(root.clone());
    tracing::info!("demo mode: settings and caches live in {}", root.display());
    let (Some(real), Some(config)) = (real_config, crate::paths::config_dir()) else { return };
    copy_dir(&real.join("themes"), &config.join("themes"));
    if let Ok(settings) = std::fs::read(real.join("settings.toml")) {
        let _ = std::fs::create_dir_all(&config);
        let _ = std::fs::write(config.join("settings.toml"), settings);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else { return };
    let _ = std::fs::create_dir_all(to);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            let _ = std::fs::copy(&path, to.join(entry.file_name()));
        }
    }
}

pub fn load(source: &Source) -> Result<DemoFile, String> {
    let text = match source {
        Source::Bundled => BUNDLED.to_string(),
        Source::File(path) => std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?,
    };
    toml::from_str(&text).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoFile {
    me: Person,
    #[serde(default)]
    users: Vec<Person>,
    #[serde(default)]
    servers: Vec<Server>,
    #[serde(default)]
    dms: Vec<Dm>,
    call: Option<Call>,
    #[serde(default)]
    open: Open,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Person {
    #[serde(default)]
    key: Option<String>,
    username: String,
    display_name: Option<String>,
    id: Option<String>,
    avatar: Option<String>,
    #[serde(default)]
    status: Status,
    playing: Option<String>,
    listening: Option<String>,
    custom_status: Option<String>,
    #[serde(default)]
    bot: bool,
    friend: Option<bool>,
    nickname: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Status {
    #[default]
    Online,
    Idle,
    Dnd,
    Offline,
}

impl Status {
    fn presence(self) -> rustcord_client::models::PresenceStatus {
        use rustcord_client::models::PresenceStatus;
        match self {
            Status::Online => PresenceStatus::Online,
            Status::Idle => PresenceStatus::Idle,
            Status::Dnd => PresenceStatus::Dnd,
            Status::Offline => PresenceStatus::Offline,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Server {
    name: String,
    id: Option<String>,
    icon: Option<String>,
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    roles: Vec<RoleDef>,
    #[serde(default)]
    channels: Vec<ChannelDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleDef {
    name: String,
    color: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ChannelKind {
    #[default]
    Text,
    Voice,
    Announcement,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelDef {
    name: String,
    #[serde(default)]
    kind: ChannelKind,
    category: Option<String>,
    topic: Option<String>,
    user_limit: Option<u32>,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    mentions: u32,
    #[serde(default)]
    voice: Vec<String>,
    #[serde(default)]
    muted: Vec<String>,
    #[serde(default)]
    deafened: Vec<String>,
    #[serde(default)]
    streaming: Vec<String>,
    #[serde(default)]
    cameras: Vec<String>,
    started_minutes_ago: Option<u64>,
    status: Option<String>,
    #[serde(default)]
    messages: Vec<MessageDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dm {
    with: Vec<String>,
    name: Option<String>,
    #[serde(default)]
    request: bool,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    mentions: u32,
    #[serde(default)]
    messages: Vec<MessageDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageDef {
    from: String,
    text: String,
    minutes_ago: Option<i64>,
    reply_to: Option<usize>,
    #[serde(default)]
    edited: bool,
    #[serde(default)]
    reactions: Vec<ReactionDef>,
    #[serde(default)]
    images: Vec<ImageDef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReactionDef {
    emoji: String,
    count: u32,
    #[serde(default)]
    me: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageDef {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "screenshare"), allow(dead_code))]
struct Call {
    server: Option<String>,
    channel: Option<String>,
    dm: Option<String>,
    #[serde(default)]
    speaking: Vec<String>,
    #[serde(default)]
    cameras: Vec<String>,
    #[serde(default)]
    streaming: Vec<String>,
    watching: Option<String>,
    #[serde(default)]
    minutes_in: u64,
    #[serde(default = "yes")]
    open: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Open {
    server: Option<String>,
    channel: Option<String>,
    dm: Option<String>,
}

struct Builder {
    next_entity: i64,
    next_message: u64,
    now_ms: i64,
    users: HashMap<String, User>,
}

impl Builder {
    fn id(&mut self) -> String {
        self.next_entity += 1;
        snowflake(ENTITY_BASE_MS + self.next_entity, 0)
    }

    fn user(&self, key: &str) -> Result<&User, String> {
        self.users.get(key).ok_or_else(|| format!("demo data refers to unknown user \"{key}\""))
    }

    fn user_id(&self, key: &str) -> Result<String, String> {
        self.user(key).map(|u| u.id.clone())
    }

    fn messages(&mut self, channel_id: &str, guild_id: Option<&str>, defs: &[MessageDef]) -> Result<Vec<Message>, String> {
        let mut built: Vec<Message> = Vec::with_capacity(defs.len());
        for (index, def) in defs.iter().enumerate() {
            let minutes_ago = def.minutes_ago.unwrap_or(((defs.len() - index) * 2) as i64);
            let ms = self.now_ms - minutes_ago * 60_000;
            self.next_message += 1;
            let id = snowflake(ms, self.next_message);
            let timestamp = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().to_rfc3339();
            let author = self.user(&def.from)?.clone();
            let mut content = def.text.clone();
            let mut mentions = Vec::new();
            for (key, user) in &self.users {
                let tag = format!("<@{key}>");
                if content.contains(&tag) {
                    content = content.replace(&tag, &format!("<@{}>", user.id));
                    mentions.push(user.clone());
                }
            }
            let reply = match def.reply_to {
                Some(target) => Some(built.get(target).cloned().ok_or_else(|| format!("reply_to = {target} in \"{channel_id}\" points past the earlier messages"))?),
                None => None,
            };
            let attachments: Vec<Value> = def
                .images
                .iter()
                .map(|image| {
                    let filename = image.url.rsplit('/').next().unwrap_or("image.png").split('?').next().unwrap_or("image.png").to_string();
                    json!({ "id": self.id(), "filename": filename, "url": image.url, "size": 0, "content_type": "image/png", "width": image.width, "height": image.height })
                })
                .collect();
            let value = json!({
                "id": id,
                "channel_id": channel_id,
                "guild_id": guild_id,
                "author": author,
                "content": content,
                "timestamp": timestamp,
                "edited_timestamp": def.edited.then(|| timestamp.clone()),
                "mentions": mentions,
                "attachments": attachments,
                "reactions": def.reactions.iter().map(|r| json!({ "emoji": { "id": null, "name": r.emoji }, "count": r.count, "me": r.me })).collect::<Vec<_>>(),
                "type": if reply.is_some() { 19 } else { 0 },
                "message_reference": reply.as_ref().map(|r| json!({ "message_id": r.id, "channel_id": channel_id, "guild_id": guild_id })),
                "referenced_message": reply,
            });
            built.push(serde_json::from_value(value).map_err(|e| format!("bad demo message: {e}"))?);
        }
        Ok(built)
    }
}

fn snowflake(ms: i64, sequence: u64) -> String {
    ((((ms - DISCORD_EPOCH_MS).max(0) as u64) << 22) | (sequence & 0xfff)).to_string()
}

fn person_user(person: &Person, id: String) -> Result<User, String> {
    serde_json::from_value(json!({
        "id": person.id.clone().unwrap_or(id),
        "username": person.username,
        "discriminator": "0",
        "global_name": person.display_name,
        "avatar": person.avatar,
        "bot": person.bot,
    }))
    .map_err(|e| format!("bad demo user {}: {e}", person.username))
}

fn activities(person: &Person) -> Vec<Value> {
    let mut list = Vec::new();
    if let Some(text) = &person.custom_status {
        list.push(json!({ "name": "Custom Status", "type": 4, "state": text }));
    }
    if let Some(game) = &person.playing {
        list.push(json!({ "name": game, "type": 0 }));
    }
    if let Some(song) = &person.listening {
        list.push(json!({ "name": "Spotify", "type": 2, "details": song }));
    }
    list
}

fn role_color(hex: &str) -> u32 {
    u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0)
}

fn read_up_to(state: &mut AppState, channel_id: &str, messages: &[Message], unread: bool, mentions: u32) {
    if mentions > 0 {
        state.mention_counts.insert(channel_id.to_string(), mentions);
    }
    let seen = if unread {
        messages.len().checked_sub(2).map_or_else(|| "0".to_string(), |index| messages[index].id.clone())
    } else {
        match messages.last() {
            Some(message) => message.id.clone(),
            None => return,
        }
    };
    state.last_read_message_id.insert(channel_id.to_string(), seen);
}

pub fn apply(data: DemoFile, state: &mut AppState) -> Result<Vec<Feed>, String> {
    let mut b = Builder { next_entity: 0, next_message: 0, now_ms: chrono::Utc::now().timestamp_millis(), users: HashMap::new() };
    let me_id = b.id();
    let me = person_user(&data.me, me_id)?;
    b.users.insert(ME.into(), me.clone());
    state.my_status = data.me.status.presence();
    state.custom_status_input = data.me.custom_status.clone().unwrap_or_default();

    let mut presences = Vec::new();
    let mut relationships = Vec::new();
    for person in &data.users {
        let key = person.key.clone().unwrap_or_else(|| person.username.clone());
        let id = b.id();
        let user = person_user(person, id)?;
        presences.push(json!({ "user": { "id": user.id }, "status": person.status.presence(), "activities": activities(person) }));
        if person.friend.unwrap_or(!person.bot) {
            relationships.push(json!({ "id": user.id, "type": 1, "user": user, "nickname": person.nickname }));
        }
        b.users.insert(key, user);
    }
    presences.push(json!({ "user": { "id": me.id }, "status": data.me.status.presence(), "activities": activities(&data.me) }));
    for presence in presences {
        let presence: rustcord_client::models::Presence = serde_json::from_value(presence).map_err(|e| format!("bad demo presence: {e}"))?;
        state.presences.insert(presence.user.id.clone(), presence);
    }
    state.relationships = relationships.into_iter().map(serde_json::from_value).collect::<Result<_, _>>().map_err(|e| format!("bad demo friend: {e}"))?;
    state.known_users = b.users.values().map(|u| (u.id.clone(), u.clone())).collect();

    let mut channel_ids: HashMap<(String, String), (String, String)> = HashMap::new();
    let mut first_text_channel: Option<(String, String)> = None;
    let joined = chrono::DateTime::from_timestamp_millis(ENTITY_BASE_MS).unwrap_or_default().to_rfc3339();
    for server in &data.servers {
        let guild_id = server.id.clone().unwrap_or_else(|| b.id());
        state.guilds.push(
            serde_json::from_value(json!({ "id": guild_id, "name": server.name, "icon": server.icon, "owner_id": me.id }))
                .map_err(|e| format!("bad demo server {}: {e}", server.name))?,
        );

        let mut roles = vec![json!({ "id": guild_id, "name": "@everyone", "permissions": "0", "position": 0 })];
        let mut member_roles: HashMap<String, Vec<String>> = HashMap::new();
        for (index, role) in server.roles.iter().enumerate() {
            let role_id = b.id();
            roles.push(json!({ "id": role_id, "name": role.name, "permissions": "0", "position": server.roles.len() - index, "color": role.color.as_deref().map(role_color).unwrap_or(0), "hoist": true }));
            for key in &role.members {
                member_roles.entry(b.user_id(key)?).or_default().push(role_id.clone());
            }
        }
        state.guild_roles.insert(guild_id.clone(), roles.into_iter().map(serde_json::from_value).collect::<Result<_, _>>().map_err(|e| format!("bad demo role: {e}"))?);
        state.my_guild_roles.insert(guild_id.clone(), member_roles.get(&me.id).cloned().unwrap_or_default());

        let mut member_keys: Vec<&str> = vec![ME];
        member_keys.extend(server.members.iter().map(String::as_str));
        let mut members = Vec::new();
        for key in member_keys {
            let user = b.user(key)?.clone();
            let roles = member_roles.get(&user.id).cloned().unwrap_or_default();
            members.push(serde_json::from_value(json!({ "user": user, "nick": null, "roles": roles, "joined_at": joined })).map_err(|e| format!("bad demo member: {e}"))?);
        }
        state.guild_members.insert(guild_id.clone(), members);

        let mut channels = Vec::new();
        let mut categories: Vec<(String, String)> = Vec::new();
        for (position, def) in server.channels.iter().enumerate() {
            let parent_id = match &def.category {
                Some(name) => Some(match categories.iter().find(|(n, _)| n == name) {
                    Some((_, id)) => id.clone(),
                    None => {
                        let id = b.id();
                        channels.push(json!({ "id": id, "type": 4, "guild_id": guild_id, "name": name, "position": categories.len() }));
                        categories.push((name.clone(), id.clone()));
                        id
                    }
                }),
                None => None,
            };
            let channel_id = b.id();
            let kind = match def.kind {
                ChannelKind::Text => 0,
                ChannelKind::Voice => 2,
                ChannelKind::Announcement => 5,
            };
            let messages = b.messages(&channel_id, Some(&guild_id), &def.messages)?;
            channels.push(json!({
                "id": channel_id,
                "type": kind,
                "guild_id": guild_id,
                "name": def.name,
                "topic": def.topic,
                "position": position,
                "parent_id": parent_id,
                "user_limit": def.user_limit,
                "last_message_id": messages.last().map(|m| m.id.clone()),
            }));
            if def.kind != ChannelKind::Voice && first_text_channel.is_none() {
                first_text_channel = Some((guild_id.clone(), channel_id.clone()));
            }
            for key in &def.voice {
                let user = b.user(key)?.clone();
                state.apply_voice_state(
                    serde_json::from_value(json!({
                        "guild_id": guild_id,
                        "channel_id": channel_id,
                        "user_id": user.id,
                        "session_id": "demo",
                        "self_mute": def.muted.contains(key) || def.deafened.contains(key),
                        "self_deaf": def.deafened.contains(key),
                        "self_stream": def.streaming.contains(key),
                        "self_video": def.cameras.contains(key),
                        "member": { "nick": null, "user": user },
                    }))
                    .map_err(|e| format!("bad demo voice state: {e}"))?,
                );
            }
            if let Some(minutes) = def.started_minutes_ago {
                state.voice_started.insert(channel_id.clone(), (b.now_ms / 1000) as u64 - minutes * 60);
            }
            if let Some(status) = &def.status {
                state.voice_status.insert(channel_id.clone(), status.clone());
            }
            read_up_to(state, &channel_id, &messages, def.unread, def.mentions);
            if !messages.is_empty() {
                state.set_message_history(channel_id.clone(), messages);
            }
            channel_ids.insert((server.name.clone(), def.name.clone()), (guild_id.clone(), channel_id));
        }
        state.guild_channels.insert(
            guild_id.clone(),
            channels.into_iter().map(serde_json::from_value).collect::<Result<_, _>>().map_err(|e| format!("bad demo channel in {}: {e}", server.name))?,
        );
    }
    state.reconcile_rail_order();

    let mut dm_ids: HashMap<String, String> = HashMap::new();
    for dm in &data.dms {
        let channel_id = b.id();
        let recipients = dm.with.iter().map(|key| b.user(key).cloned()).collect::<Result<Vec<_>, _>>()?;
        let messages = b.messages(&channel_id, None, &dm.messages)?;
        let kind = if recipients.len() == 1 { 1 } else { 3 };
        state.dm_channels.push(
            serde_json::from_value(json!({ "id": channel_id, "type": kind, "name": dm.name, "recipients": recipients, "last_message_id": messages.last().map(|m| m.id.clone()), "is_message_request": dm.request }))
                .map_err(|e| format!("bad demo dm: {e}"))?,
        );
        read_up_to(state, &channel_id, &messages, dm.unread, dm.mentions);
        if !messages.is_empty() {
            state.set_message_history(channel_id.clone(), messages);
        }
        dm_ids.insert(dm.name.clone().unwrap_or_else(|| dm.with.join(",")), channel_id);
    }

    let mut feeds = Vec::new();
    if let Some(call) = &data.call {
        let (guild_id, channel_id, others, muted, deafened) = match (&call.dm, &call.server, &call.channel) {
            (Some(dm), _, _) => {
                let channel_id = dm_ids.get(dm).cloned().ok_or_else(|| format!("call.dm \"{dm}\" matches no dm (use the user key, or the group name)"))?;
                let others = data.dms.iter().find(|d| d.name.clone().unwrap_or_else(|| d.with.join(",")) == *dm).map(|d| d.with.clone()).unwrap_or_default();
                (None, channel_id, others, false, false)
            }
            (None, Some(server), Some(channel)) => {
                let (guild_id, channel_id) = channel_ids
                    .get(&(server.clone(), channel.clone()))
                    .cloned()
                    .ok_or_else(|| format!("call refers to unknown channel \"{channel}\" in \"{server}\""))?;
                let def = data.servers.iter().find(|s| s.name == *server).and_then(|s| s.channels.iter().find(|c| c.name == *channel));
                let deafened = def.is_some_and(|d| d.deafened.iter().any(|k| k == ME));
                let muted = deafened || def.is_some_and(|d| d.muted.iter().any(|k| k == ME));
                (Some(guild_id), channel_id, Vec::new(), muted, deafened)
            }
            _ => return Err("call needs either dm, or server and channel".into()),
        };
        state.self_deafened = deafened;
        state.self_muted = muted;
        for key in others.iter().map(String::as_str).chain([ME]) {
            let user = b.user(key)?.clone();
            state.apply_voice_state(
                serde_json::from_value(json!({
                    "guild_id": guild_id,
                    "channel_id": channel_id,
                    "user_id": user.id,
                    "session_id": "demo",
                    "self_mute": key == ME && state.self_muted,
                    "self_deaf": key == ME && state.self_deafened,
                    "self_stream": key != ME && call.streaming.iter().any(|k| k == key),
                    "member": { "nick": null, "user": user },
                }))
                .map_err(|e| format!("bad demo voice state: {e}"))?,
            );
        }
        for key in &call.speaking {
            state.voice_speaking_users.insert(b.user_id(key)?);
        }
        feeds = call_feeds(call, &b, state)?;
        state.connected_voice_channel = Some((guild_id, channel_id));
        state.voice_joined_at = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(call.minutes_in * 60)).or_else(|| Some(std::time::Instant::now()));
        state.server_call_minimized = !call.open;
    }

    match (&data.open.dm, &data.open.server, &data.open.channel) {
        (Some(dm), _, _) => {
            let channel_id = dm_ids.get(dm).cloned().ok_or_else(|| format!("open.dm \"{dm}\" matches no dm (use the user key, or the group name)"))?;
            state.selected_guild = None;
            state.selected_channel = Some(channel_id);
        }
        (None, Some(server), Some(channel)) => {
            let (guild_id, channel_id) = channel_ids.get(&(server.clone(), channel.clone())).cloned().ok_or_else(|| format!("open refers to unknown channel \"{channel}\" in \"{server}\""))?;
            state.selected_guild = Some(guild_id);
            state.selected_channel = Some(channel_id);
        }
        _ => {
            state.selected_guild = first_text_channel.as_ref().map(|(g, _)| g.clone());
            state.selected_channel = first_text_channel.map(|(_, c)| c);
        }
    }

    state.current_user = Some(me);
    state.logged_in = true;
    state.gateway_connected = true;
    state.login_status = None;
    Ok(feeds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_demo_builds_a_logged_in_session_in_a_call() {
        let mut state = AppState::default();
        apply(load(&Source::Bundled).unwrap(), &mut state).unwrap();
        assert!(state.logged_in);
        assert_eq!(state.guilds.len(), 4);
        assert_eq!(state.dm_channels.len(), 5);
        assert_eq!(state.message_requests().len(), 1);
        let (Some(guild_id), channel_id) = state.connected_voice_channel.clone().unwrap() else { panic!("not in a server call") };
        assert_eq!(state.members_in_voice_channel(&guild_id, &channel_id).len(), 5);
        let general = state.selected_channel.clone().unwrap();
        assert_eq!(state.messages[&general].len(), 8);
        assert!(state.messages[&general][5].referenced_message.is_some());
    }

    #[test]
    fn unknown_users_are_reported_by_name() {
        let data: DemoFile = toml::from_str("[me]\nusername = \"a\"\n[[dms]]\nwith = [\"ghost\"]\n").unwrap();
        let error = apply(data, &mut AppState::default()).err().unwrap();
        assert!(error.contains("ghost"));
    }

    #[test]
    fn unread_channels_keep_their_newest_message_unseen() {
        let mut state = AppState::default();
        apply(load(&Source::Bundled).unwrap(), &mut state).unwrap();
        let unread = state.dm_channels.iter().filter(|dm| state.channel_is_unread(&dm.id, dm.last_message_id.as_deref())).count();
        assert_eq!(unread, 1);
    }
}
