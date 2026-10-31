use crate::error::ClientError;
use crate::models::{Channel, Guild, GuildMember, MergedPresences, Message, Presence, Role, User, VoiceState};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub enum GatewayEvent {
    Connecting,
    Connected,
    Resumed,
    Disconnected { reason: String },
    Ready { user: User, guilds: Vec<Guild>, friend_presences: Vec<Presence> },
    Presences(Vec<Presence>),
    MessageCreate(Message),
    TypingStart { channel_id: String, user_id: String },
    PresenceUpdate(Presence),
    MessageUpdate { channel_id: String, id: String, content: Option<String>, edited_timestamp: Option<String>, call: Option<crate::models::CallInfo> },
    MessageDelete { channel_id: String, id: String },
    VoiceStateUpdate(VoiceState),
    VoiceServerUpdate { token: String, guild_id: String, endpoint: Option<String> },
    InitialVoiceStates(Vec<VoiceState>),
    GuildCreate { guild_id: String, roles: Vec<Role>, member_roles: Vec<(String, Vec<String>)> },
    GuildMembersChunk { guild_id: String, members: Vec<GuildMember>, presences: Vec<Presence> },
    ReadState(Vec<ChannelReadState>),
    ChannelUpsert(Channel),
    ChannelDelete { channel_id: String, guild_id: Option<String> },
    GuildUpdate { guild_id: String, name: Option<String>, icon: Option<String> },
    GuildDelete { guild_id: String },
    MessageAck { channel_id: String, message_id: String },
    Unknown { op_name: String, raw: Value },
}

#[derive(Debug, Clone)]
pub struct ChannelReadState {
    pub channel_id: String,
    pub last_message_id: String,
}

#[derive(Deserialize)]
struct Payload {
    op: u8,
    d: Option<Value>,
    s: Option<u64>,
    t: Option<String>,
}

#[derive(Serialize)]
struct Identify<'a> {
    token: &'a str,
    properties: IdentifyProperties,
    capabilities: u64,
}

const CAPABILITY_PRIORITIZED_READY_PAYLOAD: u64 = 1 << 5;

#[derive(Serialize)]
struct IdentifyProperties {
    os: &'static str,
    browser: &'static str,
    device: &'static str,
}

#[derive(Serialize)]
struct Resume<'a> {
    token: &'a str,
    session_id: &'a str,
    seq: u64,
}

pub fn presence_frame(status: &str, activities: &[Value]) -> String {
    json!({ "op": 3, "d": { "since": 0, "activities": activities, "status": status, "afk": false } }).to_string()
}

pub fn voice_state_update_frame(guild_id: Option<&str>, channel_id: Option<&str>, self_mute: bool, self_deaf: bool) -> String {
    json!({
        "op": 4,
        "d": {
            "guild_id": guild_id,
            "channel_id": channel_id,
            "self_mute": self_mute,
            "self_deaf": self_deaf,
        }
    })
    .to_string()
}

pub fn request_guild_members_frame(guild_id: &str) -> String {
    json!({
        "op": 8,
        "d": {
            "guild_id": guild_id,
            "query": "",
            "limit": 0,
            "presences": true,
        }
    })
    .to_string()
}

#[derive(Default)]
struct Session {
    session_id: Option<String>,
    seq: Option<u64>,
    resume_gateway_url: Option<String>,
}

pub struct Gateway {
    token: String,
}

impl Gateway {
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into() }
    }

    pub async fn run(
        &self,
        tx: mpsc::UnboundedSender<GatewayEvent>,
        mut outgoing: mpsc::UnboundedReceiver<String>,
    ) -> Result<(), ClientError> {
        let mut session = Session::default();
        let mut backoff = Duration::from_secs(1);

        loop {
            let _ = tx.send(GatewayEvent::Connecting);
            match self.connect_and_run(&tx, &mut outgoing, &mut session).await {
                Ok(()) => {
                    backoff = Duration::from_secs(1);
                }
                Err(e) => {
                    tracing::warn!("gateway connection ended: {e}");
                    let _ = tx.send(GatewayEvent::Disconnected { reason: e.to_string() });
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    async fn connect_and_run(
        &self,
        tx: &mpsc::UnboundedSender<GatewayEvent>,
        outgoing: &mut mpsc::UnboundedReceiver<String>,
        session: &mut Session,
    ) -> Result<(), ClientError> {
        let url = session.resume_gateway_url.as_deref().unwrap_or(GATEWAY_URL);
        let (ws_stream, _) = tokio_tungstenite::connect_async(url).await?;
        let (mut write, mut read) = ws_stream.split();

        let hello = read
            .next()
            .await
            .ok_or_else(|| ClientError::GatewayClosed("no hello frame".into()))??;
        let hello_payload: Payload = serde_json::from_str(&hello.into_text()?)?;
        let heartbeat_interval = hello_payload
            .d
            .and_then(|d| d.get("heartbeat_interval").and_then(Value::as_u64))
            .unwrap_or(41250);

        match (&session.session_id, session.seq) {
            (Some(session_id), Some(seq)) => {
                let resume = Resume { token: &self.token, session_id, seq };
                let resume_payload = json!({ "op": 6, "d": resume });
                write.send(WsMessage::Text(resume_payload.to_string())).await?;
            }
            _ => {
                let identify = Identify {
                    token: &self.token,
                    properties: IdentifyProperties {
                        os: "Windows",
                        browser: "Discord Client",
                        device: "Discord Client",
                    },
                    capabilities: CAPABILITY_PRIORITIZED_READY_PAYLOAD,
                };
                let identify_payload = json!({ "op": 2, "d": identify });
                write.send(WsMessage::Text(identify_payload.to_string())).await?;
            }
        }

        let _ = tx.send(GatewayEvent::Connected);

        let (heartbeat_tx, mut heartbeat_rx) = mpsc::unbounded_channel::<()>();
        let heartbeat_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_interval));
            loop {
                interval.tick().await;
                if heartbeat_tx.send(()).is_err() {
                    break;
                }
            }
        });

        let result = loop {
            tokio::select! {
                frame = read.next() => {
                    let Some(frame) = frame else {
                        break Err(ClientError::GatewayClosed("stream ended".into()));
                    };
                    let frame = match frame {
                        Ok(f) => f,
                        Err(e) => break Err(e.into()),
                    };
                    match frame {
                        WsMessage::Ping(payload) => {
                            if let Err(e) = write.send(WsMessage::Pong(payload)).await {
                                break Err(e.into());
                            }
                        }
                        WsMessage::Close(frame) => {
                            let reason = frame.map(|f| format!("{}: {}", f.code, f.reason)).unwrap_or_else(|| "closed".into());
                            let resumable = frame_is_resumable(&reason);
                            if !resumable {
                                session.session_id = None;
                                session.seq = None;
                                session.resume_gateway_url = None;
                            }
                            break Err(ClientError::GatewayClosed(reason));
                        }
                        WsMessage::Text(text) => {
                            let payload: Payload = match serde_json::from_str(&text) {
                                Ok(p) => p,
                                Err(e) => break Err(e.into()),
                            };
                            if let Some(seq) = payload.s {
                                session.seq = Some(seq);
                            }
                            match payload.op {
                                1 => {
                                    if let Err(e) = write.send(WsMessage::Text(json!({ "op": 1, "d": null }).to_string())).await {
                                        break Err(e.into());
                                    }
                                }
                                7 => {
                                    break Err(ClientError::GatewayClosed("reconnect requested".into()));
                                }
                                9 => {
                                    let resumable = payload.d.and_then(|d| d.as_bool()).unwrap_or(false);
                                    if !resumable {
                                        session.session_id = None;
                                        session.seq = None;
                                        session.resume_gateway_url = None;
                                    }
                                    break Err(ClientError::GatewayClosed("invalid session".into()));
                                }
                                0 => {
                                    if payload.t.as_deref() == Some("READY") {
                                        if let Some(data) = &payload.d {
                                            session.session_id = data.get("session_id").and_then(Value::as_str).map(str::to_string);
                                            session.resume_gateway_url = data
                                                .get("resume_gateway_url")
                                                .and_then(Value::as_str)
                                                .map(|u| format!("{u}/?v=10&encoding=json"));
                                        }
                                    }
                                    if let Err(e) = self.handle_frame(payload, tx) {
                                        break Err(e);
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                Some(frame) = outgoing.recv() => {
                    if let Err(e) = write.send(WsMessage::Text(frame)).await {
                        break Err(e.into());
                    }
                }
                _ = heartbeat_rx.recv() => {
                    if let Err(e) = write.send(WsMessage::Text(json!({ "op": 1, "d": null }).to_string())).await {
                        break Err(e.into());
                    }
                }
            }
        };

        heartbeat_task.abort();
        result
    }

    fn handle_frame(&self, payload: Payload, tx: &mpsc::UnboundedSender<GatewayEvent>) -> Result<(), ClientError> {
        let Some(data) = payload.d else { return Ok(()) };
        let event = match payload.t.as_deref() {
            Some("READY") => {
                let user: User = serde_json::from_value(data["user"].clone())?;
                let guilds: Vec<Guild> = serde_json::from_value(data["guilds"].clone())?;
                let voice_states = parse_initial_voice_states(data.get("guilds"));
                tracing::info!("READY supplied {} initial voice states", voice_states.len());
                if !voice_states.is_empty() {
                    let _ = tx.send(GatewayEvent::InitialVoiceStates(voice_states));
                }
                let mut friend_presences = parse_presence_list(data.get("presences"));
                match data.get("merged_presences") {
                    Some(v) => {
                        friend_presences.extend(parse_merged_presences(v));
                    }
                    None => {
                        tracing::debug!("READY has no merged_presences field");
                    }
                }
                deduplicate_presences(&mut friend_presences);
                tracing::info!("READY supplied {} friend presences", friend_presences.len());
                let read_state = parse_read_state(data.get("read_state"));
                tracing::info!("READY supplied {} channel read states", read_state.len());
                if !read_state.is_empty() {
                    let _ = tx.send(GatewayEvent::ReadState(read_state));
                }
                GatewayEvent::Ready { user, guilds, friend_presences }
            }
            Some("RESUMED") => GatewayEvent::Resumed,
            Some("READY_SUPPLEMENTAL") => {
                let mut presences = parse_presence_list(data.get("presences"));
                if let Some(merged) = data.get("merged_presences") {
                    presences.extend(parse_merged_presences(merged));
                }
                deduplicate_presences(&mut presences);

                let voice_states = parse_initial_voice_states(data.get("guilds"));
                tracing::info!("READY_SUPPLEMENTAL supplied {} initial voice states", voice_states.len());
                if !voice_states.is_empty() {
                    let _ = tx.send(GatewayEvent::InitialVoiceStates(voice_states));
                }

                GatewayEvent::Presences(presences)
            }
            Some("MESSAGE_ACK") => {
                let channel_id = data["channel_id"].as_str().unwrap_or_default().to_string();
                let message_id = data["message_id"].as_str().unwrap_or_default().to_string();
                GatewayEvent::MessageAck { channel_id, message_id }
            }
            Some("MESSAGE_CREATE") => {
                let message: Message = serde_json::from_value(data)?;
                GatewayEvent::MessageCreate(message)
            }
            Some("TYPING_START") => {
                let channel_id = data["channel_id"].as_str().unwrap_or_default().to_string();
                let user_id = data["user_id"].as_str().unwrap_or_default().to_string();
                GatewayEvent::TypingStart { channel_id, user_id }
            }
            Some("MESSAGE_UPDATE") => {
                let text = |k: &str| data[k].as_str().map(str::to_string);
                GatewayEvent::MessageUpdate {
                    channel_id: text("channel_id").unwrap_or_default(),
                    id: text("id").unwrap_or_default(),
                    content: text("content"),
                    edited_timestamp: text("edited_timestamp"),
                    call: serde_json::from_value(data["call"].clone()).ok(),
                }
            }
            Some("MESSAGE_DELETE") => GatewayEvent::MessageDelete {
                channel_id: data["channel_id"].as_str().unwrap_or_default().to_string(),
                id: data["id"].as_str().unwrap_or_default().to_string(),
            },
            Some("PRESENCE_UPDATE") => {
                let presence: Presence = serde_json::from_value(data)?;
                GatewayEvent::PresenceUpdate(presence)
            }
            Some("VOICE_STATE_UPDATE") => {
                let voice_state: VoiceState = serde_json::from_value(data)?;
                GatewayEvent::VoiceStateUpdate(voice_state)
            }
            Some("GUILD_CREATE") => {
                let guild_id = data["id"].as_str().unwrap_or_default().to_string();
                let roles: Vec<Role> = serde_json::from_value(data["roles"].clone()).unwrap_or_default();
                let member_roles: Vec<(String, Vec<String>)> = data["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|member| {
                        let user_id = member["user"]["id"].as_str()?.to_string();
                        let roles: Vec<String> = serde_json::from_value(member["roles"].clone()).unwrap_or_default();
                        Some((user_id, roles))
                    })
                    .collect();
                GatewayEvent::GuildCreate { guild_id, roles, member_roles }
            }
            Some("CHANNEL_CREATE") | Some("CHANNEL_UPDATE") => {
                let channel: Channel = serde_json::from_value(data)?;
                GatewayEvent::ChannelUpsert(channel)
            }
            Some("CHANNEL_DELETE") => {
                let channel_id = data["id"].as_str().unwrap_or_default().to_string();
                let guild_id = data["guild_id"].as_str().map(str::to_string);
                GatewayEvent::ChannelDelete { channel_id, guild_id }
            }
            Some("GUILD_UPDATE") => {
                let guild_id = data["id"].as_str().unwrap_or_default().to_string();
                let name = data["name"].as_str().map(str::to_string);
                let icon = data["icon"].as_str().map(str::to_string);
                GatewayEvent::GuildUpdate { guild_id, name, icon }
            }
            Some("GUILD_DELETE") => {
                let guild_id = data["id"].as_str().unwrap_or_default().to_string();
                if data["unavailable"].as_bool().unwrap_or(false) {
                    tracing::debug!("guild {guild_id} is temporarily unavailable, not removing");
                    return Ok(());
                }
                GatewayEvent::GuildDelete { guild_id }
            }
            Some("GUILD_MEMBERS_CHUNK") => {
                let guild_id = data["guild_id"].as_str().unwrap_or_default().to_string();
                let members: Vec<GuildMember> = serde_json::from_value(data["members"].clone()).unwrap_or_default();
                let presences: Vec<Presence> = serde_json::from_value(data["presences"].clone()).unwrap_or_default();
                GatewayEvent::GuildMembersChunk { guild_id, members, presences }
            }
            Some("VOICE_SERVER_UPDATE") => GatewayEvent::VoiceServerUpdate {
                token: data["token"].as_str().unwrap_or_default().to_string(),
                guild_id: data["guild_id"].as_str().or(data["channel_id"].as_str()).unwrap_or_default().to_string(),
                endpoint: data["endpoint"].as_str().map(str::to_string),
            },
            Some(other) => GatewayEvent::Unknown { op_name: other.to_string(), raw: data },
            None => return Ok(()),
        };
        let _ = tx.send(event);
        Ok(())
    }
}

fn parse_initial_voice_states(guilds: Option<&Value>) -> Vec<VoiceState> {
    let mut voice_states = Vec::new();
    let Some(guilds) = guilds.and_then(Value::as_array) else { return voice_states };
    for guild in guilds {
        let guild_id = guild.get("id").and_then(Value::as_str);
        let Some(states) = guild.get("voice_states").and_then(Value::as_array) else { continue };
        for state in states {
            let mut state = state.clone();
            if state.get("guild_id").and_then(Value::as_str).is_none() {
                if let (Some(guild_id), Some(object)) = (guild_id, state.as_object_mut()) {
                    object.insert("guild_id".to_string(), Value::String(guild_id.to_string()));
                }
            }
            match serde_json::from_value::<VoiceState>(state) {
                Ok(state) => voice_states.push(state),
                Err(error) => tracing::debug!("skipping malformed initial voice state: {error}"),
            }
        }
    }
    voice_states
}

fn frame_is_resumable(reason: &str) -> bool {
    !reason.starts_with("1000") && !reason.starts_with("1001") && !reason.starts_with("4004")
}

fn parse_read_state(value: Option<&Value>) -> Vec<ChannelReadState> {
    let items = match value {
        Some(Value::Array(items)) => items,
        Some(Value::Object(obj)) => match obj.get("entries") {
            Some(Value::Array(items)) => items,
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|item| {
            let channel_id = item.get("id").and_then(Value::as_str)?;
            let last_message_id = match item.get("last_message_id") {
                Some(Value::String(id)) => id.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return None,
            };
            if last_message_id.is_empty() || last_message_id == "0" {
                return None;
            }
            Some(ChannelReadState { channel_id: channel_id.to_string(), last_message_id })
        })
        .collect()
}

fn parse_presence_list(value: Option<&Value>) -> Vec<Presence> {
    let Some(Value::Array(items)) = value else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| match serde_json::from_value::<Presence>(item.clone()) {
            Ok(presence) => Some(presence),
            Err(error) => {
                tracing::debug!("ignoring malformed presence entry: {error}");
                None
            }
        })
        .collect()
}

fn parse_merged_presences(value: &Value) -> Vec<Presence> {
    if let Ok(merged) = serde_json::from_value::<MergedPresences>(value.clone()) {
        return merged.friends;
    }
    tracing::debug!("could not parse merged presences as a whole; parsing entries individually");
    parse_presence_list(value.get("friends"))
}

fn deduplicate_presences(presences: &mut Vec<Presence>) {
    let mut seen = std::collections::HashSet::new();
    presences.retain(|presence| seen.insert(presence.user.id.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(states: &[ChannelReadState]) -> Vec<(&str, &str)> {
        states.iter().map(|s| (s.channel_id.as_str(), s.last_message_id.as_str())).collect()
    }

    #[test]
    fn parses_a_bare_read_state_array() {
        let value = json!([{ "id": "1", "last_message_id": "100" }]);
        assert_eq!(ids(&parse_read_state(Some(&value))), [("1", "100")]);
    }

    #[test]
    fn parses_the_entries_wrapped_form() {
        let value = json!({ "entries": [{ "id": "2", "last_message_id": "200" }] });
        assert_eq!(ids(&parse_read_state(Some(&value))), [("2", "200")]);
    }

    #[test]
    fn accepts_a_numeric_last_message_id() {
        let value = json!([{ "id": "3", "last_message_id": 300 }]);
        assert_eq!(ids(&parse_read_state(Some(&value))), [("3", "300")]);
    }

    #[test]
    fn skips_entries_with_no_last_message() {
        let value = json!([
            { "id": "4" },
            { "id": "5", "last_message_id": null },
            { "id": "6", "last_message_id": "0" },
            { "id": "7", "last_message_id": "" },
            { "id": "8", "last_message_id": "800" },
        ]);
        assert_eq!(ids(&parse_read_state(Some(&value))), [("8", "800")]);
    }

    #[test]
    fn a_missing_or_unexpected_field_yields_nothing() {
        assert!(parse_read_state(None).is_empty());
        assert!(parse_read_state(Some(&json!("nonsense"))).is_empty());
        assert!(parse_read_state(Some(&json!({ "entries": 5 }))).is_empty());
    }
}
