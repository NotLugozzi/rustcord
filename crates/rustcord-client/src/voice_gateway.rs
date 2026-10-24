use crate::error::ClientError;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const VOICE_VERSION: &str = "v=8";

#[derive(Debug, Clone)]
pub enum VoiceGatewayEvent {
    Connecting,
    Ready { ssrc: u32, ip: String, port: u16, modes: Vec<String> },
    SessionDescription { mode: String, secret_key: [u8; 32] },
    Speaking { ssrc: u32, user_id: Option<String> },
    Disconnected { reason: String },
}

#[derive(Deserialize)]
struct Payload {
    op: u8,
    d: Option<Value>,
}

pub struct VoiceGateway {
    endpoint: String,
    guild_id: String,
    channel_id: String,
    user_id: String,
    session_id: String,
    token: String,
}

impl VoiceGateway {
    pub fn new(endpoint: String, guild_id: String, channel_id: String, user_id: String, session_id: String, token: String) -> Self {
        Self { endpoint, guild_id, channel_id, user_id, session_id, token }
    }

    pub async fn run(
        &self,
        tx: mpsc::UnboundedSender<VoiceGatewayEvent>,
        mut outgoing: mpsc::UnboundedReceiver<String>,
    ) -> Result<(), ClientError> {
        let _ = tx.send(VoiceGatewayEvent::Connecting);

        let url = format!("wss://{}/?{VOICE_VERSION}", self.endpoint);
        let (ws_stream, _) = tokio_tungstenite::connect_async(url).await?;
        let (mut write, mut read) = ws_stream.split();

        let hello = read
            .next()
            .await
            .ok_or_else(|| ClientError::GatewayClosed("no voice hello frame".into()))??;
        let hello_payload: Payload = serde_json::from_str(&hello.into_text()?)?;
        let heartbeat_interval = hello_payload
            .d
            .and_then(|d| d.get("heartbeat_interval").and_then(Value::as_f64))
            .unwrap_or(41250.0);

        let identify = json!({
            "op": 0,
            "d": {
                "server_id": self.guild_id,
                "channel_id": self.channel_id,
                "user_id": self.user_id,
                "session_id": self.session_id,
                "token": self.token,
                "video": false,
                "streams": [],
                "max_dave_protocol_version": 0,
            }
        });
        write.send(WsMessage::Text(identify.to_string())).await?;

        let (heartbeat_tx, mut heartbeat_rx) = mpsc::unbounded_channel::<()>();
        let heartbeat_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_interval as u64));
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
                        break Err(ClientError::GatewayClosed("voice stream ended".into()));
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
                            break Err(ClientError::GatewayClosed(reason));
                        }
                        WsMessage::Text(text) => {
                            let payload: Payload = match serde_json::from_str(&text) {
                                Ok(p) => p,
                                Err(e) => break Err(e.into()),
                            };
                            if let Err(e) = handle_payload(payload, &tx) {
                                break Err(e);
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
                    let nonce = json!({ "op": 3, "d": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 });
                    if let Err(e) = write.send(WsMessage::Text(nonce.to_string())).await {
                        break Err(e.into());
                    }
                }
            }
        };

        heartbeat_task.abort();
        let _ = tx.send(VoiceGatewayEvent::Disconnected { reason: result.as_ref().err().map(|e| e.to_string()).unwrap_or_default() });
        result
    }
}

fn handle_payload(payload: Payload, tx: &mpsc::UnboundedSender<VoiceGatewayEvent>) -> Result<(), ClientError> {
    let Some(data) = payload.d else { return Ok(()) };
    let event = match payload.op {
        2 => {
            let ssrc = data["ssrc"].as_u64().unwrap_or_default() as u32;
            let ip = data["ip"].as_str().unwrap_or_default().to_string();
            let port = data["port"].as_u64().unwrap_or_default() as u16;
            let modes = data["modes"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            VoiceGatewayEvent::Ready { ssrc, ip, port, modes }
        }
        4 => {
            let mode = data["mode"].as_str().unwrap_or_default().to_string();
            let key_values = data["secret_key"].as_array().cloned().unwrap_or_default();
            let mut secret_key = [0u8; 32];
            for (i, v) in key_values.iter().take(32).enumerate() {
                secret_key[i] = v.as_u64().unwrap_or_default() as u8;
            }
            VoiceGatewayEvent::SessionDescription { mode, secret_key }
        }
        5 => {
            let ssrc = data["ssrc"].as_u64().unwrap_or_default() as u32;
            let user_id = data["user_id"].as_str().map(str::to_string);
            VoiceGatewayEvent::Speaking { ssrc, user_id }
        }
        _ => return Ok(()),
    };
    let _ = tx.send(event);
    Ok(())
}

pub fn select_protocol_frame(address: &str, port: u16, mode: &str) -> String {
    json!({
        "op": 1,
        "d": {
            "protocol": "udp",
            "data": {
                "address": address,
                "port": port,
                "mode": mode,
            }
        }
    })
    .to_string()
}

pub fn speaking_frame(ssrc: u32, speaking: bool) -> String {
    json!({
        "op": 5,
        "d": {
            "speaking": if speaking { 1 } else { 0 },
            "delay": 0,
            "ssrc": ssrc,
        }
    })
    .to_string()
}
