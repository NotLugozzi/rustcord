use crate::error::ClientError;
use crate::models::{Guild, MergedPresences, Message, Presence, User};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

#[derive(Debug, Clone)]
pub enum GatewayEvent {
    Ready { user: User, guilds: Vec<Guild>, friend_presences: Vec<Presence> },
    Presences(Vec<Presence>),
    MessageCreate(Message),
    TypingStart { channel_id: String, user_id: String },
    PresenceUpdate(Presence),
    MessageUpdate { channel_id: String, id: String, content: Option<String>, edited_timestamp: Option<String> },
    MessageDelete { channel_id: String, id: String },
    Unknown { op_name: String, raw: Value },
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
    intents: Option<u64>,
}

#[derive(Serialize)]
struct IdentifyProperties {
    os: &'static str,
    browser: &'static str,
    device: &'static str,
}

pub fn presence_frame(status: &str, activities: &[Value]) -> String {
    json!({ "op": 3, "d": { "since": 0, "activities": activities, "status": status, "afk": false } }).to_string()
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
        let (ws_stream, _) = tokio_tungstenite::connect_async(GATEWAY_URL).await?;
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

        let identify = Identify {
            token: &self.token,
            properties: IdentifyProperties {
                os: "Windows",
                browser: "Chrome",
                device: "",
            },
            intents: None,
        };
        let identify_payload = json!({ "op": 2, "d": identify });
        write.send(WsMessage::Text(identify_payload.to_string())).await?;

        let (heartbeat_tx, mut heartbeat_rx) = mpsc::unbounded_channel::<()>();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_interval));
            loop {
                interval.tick().await;
                if heartbeat_tx.send(()).is_err() {
                    break;
                }
            }
        });

        loop {
            tokio::select! {
                frame = read.next() => {
                    let Some(frame) = frame else {
                        return Err(ClientError::GatewayClosed("stream ended".into()));
                    };
                    let frame = frame?;
                    match frame {
                        WsMessage::Ping(payload) => write.send(WsMessage::Pong(payload)).await?,
                        WsMessage::Text(text) => {
                            let payload: Payload = serde_json::from_str(&text)?;
                            if payload.op == 1 {
                                write.send(WsMessage::Text(json!({ "op": 1, "d": null }).to_string())).await?;
                                continue;
                            }
                            self.handle_frame(&text, &tx)?;
                        }
                        _ => {}
                    }
                }
                Some(frame) = outgoing.recv() => {
                    write.send(WsMessage::Text(frame)).await?;
                }
                _ = heartbeat_rx.recv() => {
                    write.send(WsMessage::Text(json!({ "op": 1, "d": null }).to_string())).await?;
                }
            }
        }
    }

    fn handle_frame(&self, text: &str, tx: &mpsc::UnboundedSender<GatewayEvent>) -> Result<(), ClientError> {
        let payload: Payload = serde_json::from_str(text)?;
        if payload.op != 0 {
            return Ok(());
        }
        let Some(data) = payload.d else { return Ok(()) };
        let event = match payload.t.as_deref() {
            Some("READY") => {
                let user: User = serde_json::from_value(data["user"].clone())?;
                let guilds: Vec<Guild> = serde_json::from_value(data["guilds"].clone())?;
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
                GatewayEvent::Ready { user, guilds, friend_presences }
            }
            Some("READY_SUPPLEMENTAL") => {
                let mut presences = parse_presence_list(data.get("presences"));
                if let Some(merged) = data.get("merged_presences") {
                    presences.extend(parse_merged_presences(merged));
                }
                deduplicate_presences(&mut presences);
                GatewayEvent::Presences(presences)
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
            Some(other) => GatewayEvent::Unknown { op_name: other.to_string(), raw: data },
            None => return Ok(()),
        };
        let _ = tx.send(event);
        Ok(())
    }
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
