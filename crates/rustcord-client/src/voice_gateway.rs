use crate::dave::{self, DaveSession};
use crate::error::ClientError;
use crate::voice_rtc::{RtcEvent, VoiceRtc};
use crate::voice_sdp;
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
    Ready,
    MediaConnected,
    Speaking { ssrc: u32, user_id: Option<String> },
    DaveReady,
    Disconnected { reason: String },
}

#[derive(Deserialize)]
struct Payload {
    op: u8,
    d: Option<Value>,
    #[serde(default)]
    seq: Option<u64>,
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
                "max_dave_protocol_version": dave::DAVE_PROTOCOL_VERSION,
            }
        });
        write.send(WsMessage::Text(identify.to_string())).await?;

        let mut dave_session = DaveSession::new(&self.user_id)?;
        let mut rtc: Option<VoiceRtc> = None;
        let mut rtc_buf = vec![0u8; 2000];
        let mut seq_ack: u64 = 0;

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
                            tracing::info!("voice: closed: {reason}");
                            break Err(ClientError::GatewayClosed(reason));
                        }
                        WsMessage::Text(text) => {
                            let payload: Payload = match serde_json::from_str(&text) {
                                Ok(p) => p,
                                Err(e) => break Err(e.into()),
                            };
                            if let Some(seq) = payload.seq {
                                seq_ack = seq;
                            }
                            if payload.op == 2 {
                                if let Err(e) = write.send(WsMessage::Text(json!({ "op": 16, "d": {} }).to_string())).await {
                                    break Err(e.into());
                                }
                            } else if payload.op == 16 {
                                let (new_rtc, params) = match VoiceRtc::new().await {
                                    Ok(v) => v,
                                    Err(e) => break Err(e),
                                };
                                let offer = voice_sdp::build_offer(&params);
                                rtc = Some(new_rtc);
                                let select_protocol = select_protocol_frame(&offer);
                                if let Err(e) = write.send(WsMessage::Text(select_protocol)).await {
                                    break Err(e.into());
                                }
                                let _ = tx.send(VoiceGatewayEvent::Ready);
                            } else if payload.op == 4 {
                                let Some(active_rtc) = rtc.as_mut() else {
                                    break Err(ClientError::GatewayClosed("voice: session description with no pending rtc".into()));
                                };
                                let remote_sdp = payload
                                    .d
                                    .as_ref()
                                    .and_then(|d| d.get("sdp"))
                                    .and_then(Value::as_str)
                                    .and_then(voice_sdp::parse_remote_sdp);
                                let Some(remote) = remote_sdp else {
                                    break Err(ClientError::GatewayClosed("voice: session description missing parsable sdp".into()));
                                };
                                if let Err(e) = active_rtc.set_remote(&remote) {
                                    tracing::warn!("voice: set_remote failed: {e}");
                                    break Err(e);
                                }
                            }
                            if let Err(e) = handle_payload(payload, &tx) {
                                break Err(e);
                            }
                        }
                        WsMessage::Binary(bytes) => {
                            match handle_dave_frame(&bytes, &mut dave_session, &tx) {
                                Ok(Some(reply)) => {
                                    if let Err(e) = write.send(WsMessage::Binary(reply)).await {
                                        break Err(e.into());
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    tracing::warn!("voice: dave frame handling failed: {e}");
                                    break Err(e);
                                }
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
                    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                    let heartbeat = json!({ "op": 3, "d": { "t": t, "seq_ack": seq_ack } });
                    if let Err(e) = write.send(WsMessage::Text(heartbeat.to_string())).await {
                        break Err(e.into());
                    }
                }
                rtc_event = drive_rtc(rtc.as_mut(), &mut rtc_buf) => {
                    match rtc_event {
                        Ok(Some(RtcEvent::Connected)) => {
                            let _ = tx.send(VoiceGatewayEvent::MediaConnected);
                        }
                        Ok(Some(RtcEvent::Disconnected)) => {
                            break Err(ClientError::GatewayClosed("voice media connection disconnected".into()));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("voice: rtc drive failed: {e}");
                            break Err(e);
                        }
                    }
                }
            }
        };

        heartbeat_task.abort();
        let _ = tx.send(VoiceGatewayEvent::Disconnected { reason: result.as_ref().err().map(|e| e.to_string()).unwrap_or_default() });
        result
    }
}

async fn drive_rtc(rtc: Option<&mut VoiceRtc>, buf: &mut [u8]) -> Result<Option<RtcEvent>, ClientError> {
    match rtc {
        Some(rtc) => rtc.drive(buf).await,
        None => std::future::pending().await,
    }
}

fn handle_payload(payload: Payload, tx: &mpsc::UnboundedSender<VoiceGatewayEvent>) -> Result<(), ClientError> {
    let Some(data) = payload.d else { return Ok(()) };
    let event = match payload.op {
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

fn handle_dave_frame(
    bytes: &[u8],
    dave_session: &mut DaveSession,
    tx: &mpsc::UnboundedSender<VoiceGatewayEvent>,
) -> Result<Option<Vec<u8>>, ClientError> {
    let frame = dave::decode_server_frame(bytes)?;
    match frame.opcode {
        dave::OP_MLS_EXTERNAL_SENDER => Ok(None),
        dave::OP_PREPARE_EPOCH => {
            dave_session.create_new_group()?;
            let key_package = dave_session.generate_key_package()?;
            Ok(Some(dave::encode_client_frame(dave::OP_MLS_KEY_PACKAGE, &key_package)))
        }
        dave::OP_MLS_PROPOSALS => Ok(None),
        dave::OP_MLS_ANNOUNCE_COMMIT_TRANSITION => {
            dave_session.process_commit(&frame.payload)?;
            Ok(Some(dave::encode_client_frame(dave::OP_TRANSITION_READY, &[])))
        }
        dave::OP_MLS_WELCOME => {
            dave_session.process_welcome(&frame.payload)?;
            Ok(Some(dave::encode_client_frame(dave::OP_TRANSITION_READY, &[])))
        }
        dave::OP_EXECUTE_TRANSITION => {
            let _ = tx.send(VoiceGatewayEvent::DaveReady);
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn random_uuid_v4() -> String {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mixed = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(i as u64 * 0x2545F4914F6CDD1D);
        chunk.copy_from_slice(&mixed.to_le_bytes()[..chunk.len()]);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12],
        bytes[13], bytes[14], bytes[15]
    )
}

pub fn select_protocol_frame(offer: &str) -> String {
    json!({
        "op": 1,
        "d": {
            "protocol": "webrtc",
            "data": offer,
            "sdp": offer,
            "codecs": [
                { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 111, "rtx_payload_type": null },
                { "name": "H264", "type": "video", "priority": 1000, "payload_type": 102, "rtx_payload_type": 103 },
                { "name": "VP8", "type": "video", "priority": 2000, "payload_type": 96, "rtx_payload_type": 97 },
                { "name": "VP9", "type": "video", "priority": 3000, "payload_type": 98, "rtx_payload_type": 99 }
            ],
            "rtc_connection_id": random_uuid_v4(),
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
