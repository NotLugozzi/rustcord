use crate::dave::{self, DaveSession};
use crate::error::ClientError;
use crate::voice_audio::{AudioCodec, MicCapture, SpeakerPlayback};
use crate::voice_rtc::{RtcEvent, VoiceRtc};
use crate::voice_sdp;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Clone, Default)]
pub struct VoiceMediaState {
    pub muted: Arc<AtomicBool>,
    pub deafened: Arc<AtomicBool>,
    pub input_device: Arc<std::sync::Mutex<Option<String>>>,
    pub output_device: Arc<std::sync::Mutex<Option<String>>>,
}

const VOICE_VERSION: &str = "v=8";

#[derive(Debug, Clone)]
pub enum VoiceGatewayEvent {
    Connecting,
    Ready,
    MediaConnected,
    Speaking { ssrc: u32, user_id: Option<String>, speaking: bool },
    DaveReady,
    AudioReceived { payload: Vec<u8> },
    Stats { bitrate_up_bps: u32, bitrate_down_bps: u32 },
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
    media_state: VoiceMediaState,
}

impl VoiceGateway {
    pub fn new(
        endpoint: String,
        guild_id: String,
        channel_id: String,
        user_id: String,
        session_id: String,
        token: String,
        media_state: VoiceMediaState,
    ) -> Self {
        Self { endpoint, guild_id, channel_id, user_id, session_id, token, media_state }
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
        let local_user_id = self.user_id.parse::<u64>().map_err(|e| ClientError::GatewayClosed(format!("voice: invalid local user id: {e}")))?;
        let mut frame_crypto = crate::dave_frame_crypto::FrameCrypto::new(local_user_id);
        let mut remote_speakers: HashMap<u32, String> = HashMap::new();
        let mut recognized_users = vec![self.user_id.clone()];
        let mut dave_enabled = false;
        let mut target_dave_version = 0u32;
        let mut pending_frame_epoch_update = false;
        let mut pending_dave_group_id: Option<u64> = None;
        let mut rtc: Option<VoiceRtc> = None;
        let mut media_connected = false;
        let mut rtc_buf = vec![0u8; 2000];
        let mut seq_ack: u64 = 0;
        let mut mic_frame_count: u64 = 0;
        let mut received_audio_frame_count: u64 = 0;
        let mut decrypt_success_count: u64 = 0;
        let mut decrypt_failure_count: u64 = 0;
        let mut decode_success_count: u64 = 0;
        let mut decode_failure_count: u64 = 0;
        let mut speaking_activity: HashMap<String, Instant> = HashMap::new();
        let mut speaking_check = tokio::time::interval(Duration::from_millis(200));
        let mut bytes_sent: u64 = 0;
        let mut bytes_received: u64 = 0;
        let mut stats_check = tokio::time::interval(Duration::from_secs(1));
        let mut codec = AudioCodec::new()?;
        let mut mic: Option<MicCapture> = None;
        let mut speaker: Option<SpeakerPlayback> = None;
        let mut attempted_input_device: Option<Option<String>> = None;
        let mut attempted_output_device: Option<Option<String>> = None;
        let mut device_check = tokio::time::interval(Duration::from_millis(500));
        let (mic_frame_tx, mut mic_frame_rx) = mpsc::unbounded_channel::<Vec<i16>>();

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
                _ = speaking_check.tick() => {
                    const SPEAKING_TIMEOUT: Duration = Duration::from_millis(500);
                    let now = Instant::now();
                    speaking_activity.retain(|user_id, last_active| {
                        if now.duration_since(*last_active) < SPEAKING_TIMEOUT {
                            return true;
                        }
                        let ssrc = remote_speakers.iter().find(|(_, id)| *id == user_id).map(|(ssrc, _)| *ssrc).unwrap_or_default();
                        let _ = tx.send(VoiceGatewayEvent::Speaking { ssrc, user_id: Some(user_id.clone()), speaking: false });
                        false
                    });
                }
                _ = stats_check.tick() => {
                    let up = (bytes_sent * 8) as u32;
                    let down = (bytes_received * 8) as u32;
                    bytes_sent = 0;
                    bytes_received = 0;
                    let _ = tx.send(VoiceGatewayEvent::Stats { bitrate_up_bps: up, bitrate_down_bps: down });
                }
                _ = device_check.tick(), if media_connected => {
                    let input_device = self.media_state.input_device.lock().unwrap().clone();
                    if attempted_input_device.as_ref() != Some(&input_device) {
                        attempted_input_device = Some(input_device.clone());
                        match MicCapture::start(input_device.as_deref(), self.media_state.muted.clone(), {
                            let mic_frame_tx = mic_frame_tx.clone();
                            move |frame| { let _ = mic_frame_tx.send(frame); }
                        }) {
                            Ok(new_mic) => {
                                mic = Some(new_mic);
                                tracing::info!("voice: microphone switched to {}", input_device.as_deref().unwrap_or("Default"));
                            }
                            Err(error) => tracing::warn!("voice: failed to switch microphone to {}: {error}", input_device.as_deref().unwrap_or("Default")),
                        }
                    }
                    let output_device = self.media_state.output_device.lock().unwrap().clone();
                    if attempted_output_device.as_ref() != Some(&output_device) {
                        attempted_output_device = Some(output_device.clone());
                        match SpeakerPlayback::start(output_device.as_deref(), self.media_state.deafened.clone()) {
                            Ok(new_speaker) => {
                                speaker = Some(new_speaker);
                                tracing::info!("voice: speaker switched to {}", output_device.as_deref().unwrap_or("Default"));
                            }
                            Err(error) => tracing::warn!("voice: failed to switch speaker to {}: {error}", output_device.as_deref().unwrap_or("Default")),
                        }
                    }
                }
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
                            if matches!(payload.op, dave::OP_PREPARE_TRANSITION | dave::OP_EXECUTE_TRANSITION | dave::OP_PREPARE_EPOCH) {
                                tracing::info!("voice: received DAVE control opcode {}, data {:?}", payload.op, payload.d);
                            }
                            if payload.op == 2 {
                                tracing::info!("voice: op 2 Ready payload: {:?}", payload.d);
                                if let Err(e) = write.send(WsMessage::Text(json!({ "op": 16, "d": {} }).to_string())).await {
                                    break Err(e.into());
                                }
                            } else if payload.op == 16 {
                                let (mut new_rtc, params) = match VoiceRtc::new().await {
                                    Ok(v) => v,
                                    Err(e) => break Err(e),
                                };
                                for &remote_ssrc in remote_speakers.keys() {
                                    new_rtc.expect_remote_audio(remote_ssrc);
                                }
                                let offer = voice_sdp::build_offer(&params);
                                rtc = Some(new_rtc);
                                let select_protocol = select_protocol_frame(&offer);
                                if let Err(e) = write.send(WsMessage::Text(select_protocol)).await {
                                    break Err(e.into());
                                }
                                let _ = tx.send(VoiceGatewayEvent::Ready);
                            } else if payload.op == 4 {
                                if let Some(data) = payload.d.as_ref() {
                                    target_dave_version = data["dave_protocol_version"].as_u64().unwrap_or(0) as u32;
                                    if target_dave_version > 0 {
                                        tracing::info!("voice: gateway selected DAVE protocol version {target_dave_version}");
                                        pending_dave_group_id = data["group_id"].as_u64()
                                            .or_else(|| self.channel_id.parse::<u64>().ok());
                                        if dave_session.has_external_sender() {
                                            if let Some(group_id) = pending_dave_group_id.take() {
                                                match dave_session.create_new_group(group_id) {
                                                    Ok(package) => {
                                                        tracing::info!("voice: sending initial DAVE MLS key package");
                                                        if let Err(e) = write.send(WsMessage::Binary(dave::encode_client_frame(dave::OP_MLS_KEY_PACKAGE, &package))).await {
                                                            break Err(e.into());
                                                        }
                                                    }
                                                    Err(e) => break Err(e),
                                                }
                                            }
                                        }
                                    }
                                }
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
                            } else if payload.op == 5 {
                                if let Some(data) = &payload.d {
                                    let ssrc = data["ssrc"].as_u64().unwrap_or_default() as u32;
                                    let speaker_id = data["user_id"].as_str();
                                    if ssrc != 0 && speaker_id != Some(self.user_id.as_str()) {
                                        if let Some(user_id) = speaker_id {
                                            tracing::info!("voice: mapped remote audio SSRC {ssrc} to user {user_id}");
                                            remote_speakers.insert(ssrc, user_id.to_string());
                                            if dave_enabled {
                                                if let Ok(secret) = dave_session.sender_base_secret(user_id) {
                                                    if let Ok(id) = user_id.parse::<u64>() {
                                                        if pending_frame_epoch_update {
                                                            frame_crypto.prepare_sender_base_secret(id, secret);
                                                        } else {
                                                            frame_crypto.set_sender_base_secret(id, secret);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if let Some(active_rtc) = rtc.as_mut() {
                                            active_rtc.expect_remote_audio(ssrc);
                                        }
                                    }
                                }
                            } else if payload.op == 11 {
                                if let Some(ids) = payload.d.as_ref().and_then(|d| d["user_ids"].as_array()) {
                                    for id in ids.iter().filter_map(|value| value.as_str()) {
                                        if !recognized_users.iter().any(|known| known == id) {
                                            recognized_users.push(id.to_string());
                                        }
                                    }
                                    tracing::info!("voice: gateway recognized {} DAVE users", recognized_users.len());
                                }
                            } else if payload.op == 13 {
                                if let Some(id) = payload.d.as_ref().and_then(|d| d["user_id"].as_str()) {
                                    recognized_users.retain(|known| known != id || known == &self.user_id);
                                }
                            } else if payload.op == 24 {
                                if let Some(data) = &payload.d {
                                    target_dave_version = data["protocol_version"].as_u64().unwrap_or(0) as u32;
                                    let epoch = data["epoch"].as_u64().unwrap_or_default();
                                    if epoch == 1 && target_dave_version > 0 {
                                        frame_crypto.begin_epoch();
                                        pending_frame_epoch_update = true;
                                        let group_id = data["group_id"].as_u64()
                                            .or_else(|| self.channel_id.parse::<u64>().ok())
                                            .ok_or_else(|| ClientError::GatewayClosed("voice: DAVE prepare_epoch missing group ID".into()));
                                        let group_id = match group_id {
                                            Ok(id) => id,
                                            Err(e) => break Err(e),
                                        };
                                        pending_dave_group_id = Some(group_id);
                                        if dave_session.has_external_sender() {
                                            let package = match dave_session.create_new_group(group_id) {
                                                Ok(package) => package,
                                                Err(e) => break Err(e),
                                            };
                                            pending_dave_group_id = None;
                                            if let Err(e) = write.send(WsMessage::Binary(dave::encode_client_frame(dave::OP_MLS_KEY_PACKAGE, &package))).await {
                                                break Err(e.into());
                                            }
                                        }
                                    }
                                }
                            } else if payload.op == 21 {
                                if let Some(data) = &payload.d {
                                    target_dave_version = data["protocol_version"].as_u64().unwrap_or(0) as u32;
                                    let transition_id = data["transition_id"].as_u64().unwrap_or_default();
                                    if transition_id == 0 && target_dave_version > 0 {
                                        if let Err(e) = prepare_dave_frame_keys(&dave_session, &mut frame_crypto, local_user_id, remote_speakers.values()) {
                                            break Err(e);
                                        }
                                        frame_crypto.execute_epoch();
                                        pending_frame_epoch_update = false;
                                        dave_enabled = true;
                                    }
                                    if let Err(e) = write.send(WsMessage::Text(json!({"op": 23, "d": {"transition_id": transition_id}}).to_string())).await {
                                        break Err(e.into());
                                    }
                                }
                            } else if payload.op == 22 {
                                if pending_frame_epoch_update {
                                    frame_crypto.execute_epoch();
                                    pending_frame_epoch_update = false;
                                }
                                dave_enabled = target_dave_version > 0;
                                tracing::info!("voice: executing DAVE transition; protocol version {}, enabled={dave_enabled}", target_dave_version);
                                let _ = tx.send(VoiceGatewayEvent::DaveReady);
                            } else if payload.op == 12 {
                                if let Some(data) = &payload.d {
                                    let user_id = data["user_id"].as_str();
                                    let audio_ssrc = data["audio_ssrc"].as_u64().unwrap_or_default() as u32;
                                    tracing::info!("voice: op 12 client connect, user {:?}, audio_ssrc {audio_ssrc}", user_id);
                                    if let (Some(user_id), true) = (user_id, audio_ssrc != 0) {
                                        remote_speakers.insert(audio_ssrc, user_id.to_string());
                                        if dave_enabled {
                                            if let Ok(secret) = dave_session.sender_base_secret(user_id) {
                                                if let Ok(id) = user_id.parse::<u64>() {
                                                    if pending_frame_epoch_update {
                                                        frame_crypto.prepare_sender_base_secret(id, secret);
                                                    } else {
                                                        frame_crypto.set_sender_base_secret(id, secret);
                                                    }
                                                }
                                            }
                                        }
                                        if let Some(active_rtc) = rtc.as_mut() {
                                            active_rtc.expect_remote_audio(audio_ssrc);
                                        }
                                    }
                                }
                            } else if !matches!(payload.op, 3 | 8) {
                                tracing::info!("voice: unhandled opcode {}, data {:?}", payload.op, payload.d);
                            }
                            if let Err(e) = handle_payload(payload, &tx) {
                                break Err(e);
                            }
                        }
                        WsMessage::Binary(bytes) => {
                            let server_frame = match dave::decode_server_frame(&bytes) {
                                Ok(frame) => {
                                    seq_ack = u64::from(frame.seq);
                                    Some(frame)
                                }
                                Err(_) => None,
                            };
                            let opcode = server_frame.as_ref().map(|frame| frame.opcode);
                            let is_initial_transition = server_frame.as_ref().is_some_and(|frame| {
                                matches!(frame.opcode, dave::OP_MLS_ANNOUNCE_COMMIT_TRANSITION | dave::OP_MLS_WELCOME)
                                    && frame.payload.get(..2).is_some_and(|id| id == [0, 0])
                            });
                            tracing::info!("voice: received DAVE binary opcode {:?}", opcode);
                            if matches!(opcode, Some(dave::OP_MLS_ANNOUNCE_COMMIT_TRANSITION | dave::OP_MLS_WELCOME)) {
                                target_dave_version = dave::DAVE_PROTOCOL_VERSION;
                            }
                            match handle_dave_frame(&bytes, &mut dave_session, &mut pending_dave_group_id, &recognized_users, &tx) {
                                Ok(Some(reply)) => {
                                    if matches!(opcode, Some(dave::OP_MLS_ANNOUNCE_COMMIT_TRANSITION | dave::OP_MLS_WELCOME)) {
                                        if let Err(e) = prepare_dave_frame_keys(&dave_session, &mut frame_crypto, local_user_id, remote_speakers.values()) {
                                            break Err(e);
                                        }
                                        pending_frame_epoch_update = true;
                                    }
                                    if is_initial_transition {
                                        frame_crypto.execute_epoch();
                                        pending_frame_epoch_update = false;
                                        dave_enabled = target_dave_version > 0;
                                        tracing::info!("voice: executed initial DAVE transition 0 locally; protocol version {}, enabled={dave_enabled}", target_dave_version);
                                    }
                                    if let Err(e) = write.send(reply).await {
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
                            media_connected = true;
                            let audio_ssrc = rtc.as_ref().map(|r| r.audio_ssrc()).unwrap_or_default();
                            let client_connect = json!({
                                "op": 12,
                                "d": { "audio_ssrc": audio_ssrc, "video_ssrc": 0, "rtx_ssrc": 0 }
                            });
                            if let Err(e) = write.send(WsMessage::Text(client_connect.to_string())).await {
                                break Err(e.into());
                            }
                            if let Err(e) = write.send(WsMessage::Text(speaking_frame(audio_ssrc, true))).await {
                                break Err(e.into());
                            }

                            let input_device = self.media_state.input_device.lock().unwrap().clone();
                            attempted_input_device = Some(input_device.clone());
                            match MicCapture::start(input_device.as_deref(), self.media_state.muted.clone(), {
                                let mic_frame_tx = mic_frame_tx.clone();
                                move |frame| { let _ = mic_frame_tx.send(frame); }
                            }) {
                                Ok(m) => {
                                    tracing::info!("voice: mic capture started");
                                    mic = Some(m);
                                }
                                Err(e) => tracing::warn!("voice: failed to start mic capture: {e}"),
                            }
                            let output_device = self.media_state.output_device.lock().unwrap().clone();
                            attempted_output_device = Some(output_device.clone());
                            match SpeakerPlayback::start(output_device.as_deref(), self.media_state.deafened.clone()) {
                                Ok(s) => {
                                    tracing::info!("voice: speaker playback started");
                                    speaker = Some(s);
                                }
                                Err(e) => tracing::warn!("voice: failed to start speaker playback: {e}"),
                            }

                            let _ = tx.send(VoiceGatewayEvent::MediaConnected);
                        }
                        Ok(Some(RtcEvent::Disconnected)) => {
                            break Err(ClientError::GatewayClosed("voice media connection disconnected".into()));
                        }
                        Ok(Some(RtcEvent::AudioData { ssrc, payload })) => {
                            received_audio_frame_count += 1;
                            bytes_received += payload.len() as u64;
                            if received_audio_frame_count == 1 || received_audio_frame_count % 50 == 0 {
                                tracing::info!("voice: received {received_audio_frame_count} decoded RTP audio events; latest SSRC {ssrc}, {} bytes, DAVE enabled={dave_enabled}", payload.len());
                            }
                            if payload.is_empty() {
                                continue;
                            }
                            let is_dtx = payload == [0xf8, 0xff, 0xfe];
                            if !is_dtx {
                                if let Some(user_id) = remote_speakers.get(&ssrc) {
                                    if speaking_activity.insert(user_id.clone(), Instant::now()).is_none() {
                                        let _ = tx.send(VoiceGatewayEvent::Speaking { ssrc, user_id: Some(user_id.clone()), speaking: true });
                                    }
                                }
                            }
                            let decoded_payload = if dave_enabled && is_dtx {
                                payload
                            } else if dave_enabled {
                                let Some(user_id) = remote_speakers.get(&ssrc).and_then(|id| id.parse::<u64>().ok()) else {
                                    tracing::warn!("voice: no speaker identity for encrypted stream SSRC {ssrc}");
                                    continue;
                                };
                                match frame_crypto.decrypt(user_id, &payload) {
                                    Some(plain) => {
                                        decrypt_success_count += 1;
                                        plain
                                    }
                                    None => {
                                        decrypt_failure_count += 1;
                                        if decrypt_failure_count == 1 || decrypt_failure_count % 20 == 0 {
                                            let tail = &payload[payload.len().saturating_sub(16)..];
                                            tracing::warn!("voice: DAVE decryption failed ({decrypt_failure_count} total, {decrypt_success_count} succeeded) for SSRC {ssrc} (user {user_id}); {} byte frame, tail {:02x?}", payload.len(), tail);
                                        }
                                        continue;
                                    }
                                }
                            } else {
                                payload
                            };
                            match codec.decode(ssrc, &decoded_payload) {
                                Ok(pcm) => {
                                    decode_success_count += 1;
                                    if decode_success_count == 1 || decode_success_count % 50 == 0 {
                                        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                                        tracing::info!("voice: decoded {decode_success_count} audio frames so far, latest peak sample magnitude {peak}");
                                    }
                                    if let Some(speaker) = speaker.as_ref() {
                                        speaker.push_frame(ssrc, pcm);
                                    } else {
                                        tracing::warn!("voice: decoded audio but no speaker available");
                                    }
                                }
                                Err(e) => {
                                    decode_failure_count += 1;
                                    if decode_failure_count == 1 || decode_failure_count % 20 == 0 {
                                        tracing::warn!("voice: opus decode failed ({decode_failure_count} total): {e}");
                                    }
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("voice: rtc drive failed: {e}");
                            break Err(e);
                        }
                    }
                }
                Some(pcm_frame) = mic_frame_rx.recv() => {
                    mic_frame_count += 1;
                    if mic_frame_count % 50 == 0 {
                        tracing::info!("voice: sent {mic_frame_count} mic frames so far");
                    }
                    if speaking_activity.insert(self.user_id.clone(), Instant::now()).is_none() {
                        let local_ssrc = rtc.as_ref().map(|r| r.audio_ssrc()).unwrap_or_default();
                        let _ = tx.send(VoiceGatewayEvent::Speaking { ssrc: local_ssrc, user_id: Some(self.user_id.clone()), speaking: true });
                    }
                    match codec.encode(&pcm_frame) {
                        Ok(opus_payload) => {
                            let outgoing_payload = if dave_enabled {
                                match frame_crypto.encrypt(&opus_payload) {
                                    Some(encrypted) => encrypted,
                                    None => {
                                        tracing::warn!("voice: DAVE encryption failed; dropping audio frame");
                                        continue;
                                    }
                                }
                            } else {
                                opus_payload
                            };
                            if let Some(active_rtc) = rtc.as_mut() {
                                if mic_frame_count == 1 || mic_frame_count % 100 == 0 {
                                    tracing::info!("voice: submitting {} byte Opus frame to RTC, DAVE enabled={dave_enabled}", outgoing_payload.len());
                                }
                                bytes_sent += outgoing_payload.len() as u64;
                                if let Err(e) = active_rtc.send_audio_frame(&outgoing_payload) {
                                    tracing::warn!("voice: failed to send audio frame: {e}");
                                }
                            } else {
                                tracing::warn!("voice: have mic frame but no active rtc");
                            }
                        }
                        Err(e) => tracing::warn!("voice: opus encode failed: {e}"),
                    }
                }
            }
        };

        drop(mic);
        drop(speaker);

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

fn prepare_dave_frame_keys<'a>(
    session: &DaveSession,
    crypto: &mut crate::dave_frame_crypto::FrameCrypto,
    local_user_id: u64,
    remote_user_ids: impl Iterator<Item = &'a String>,
) -> Result<(), ClientError> {
    crypto.begin_epoch();
    let local_secret = session.sender_base_secret(&local_user_id.to_string())?;
    crypto.prepare_sender_base_secret(local_user_id, local_secret);
    for user_id in remote_user_ids {
        let Ok(id) = user_id.parse::<u64>() else { continue };
        if let Ok(secret) = session.sender_base_secret(user_id) {
            crypto.prepare_sender_base_secret(id, secret);
        }
    }
    Ok(())
}

fn handle_payload(payload: Payload, tx: &mpsc::UnboundedSender<VoiceGatewayEvent>) -> Result<(), ClientError> {
    let Some(data) = payload.d else { return Ok(()) };
    let event = match payload.op {
        5 => {
            let ssrc = data["ssrc"].as_u64().unwrap_or_default() as u32;
            let user_id = data["user_id"].as_str().map(str::to_string);
            let speaking = data["speaking"].as_u64().unwrap_or_default() != 0;
            VoiceGatewayEvent::Speaking { ssrc, user_id, speaking }
        }
        _ => return Ok(()),
    };
    let _ = tx.send(event);
    Ok(())
}

fn handle_dave_frame(
    bytes: &[u8],
    dave_session: &mut DaveSession,
    pending_group_id: &mut Option<u64>,
    recognized_users: &[String],
    tx: &mpsc::UnboundedSender<VoiceGatewayEvent>,
) -> Result<Option<WsMessage>, ClientError> {
    let frame = dave::decode_server_frame(bytes)?;
    match frame.opcode {
        dave::OP_MLS_EXTERNAL_SENDER => {
            dave_session.set_external_sender(&frame.payload)?;
            if let Some(group_id) = pending_group_id.take() {
                let package = dave_session.create_new_group(group_id)?;
                tracing::info!("voice: sending DAVE MLS key package for group initialization");
                Ok(Some(WsMessage::Binary(dave::encode_client_frame(dave::OP_MLS_KEY_PACKAGE, &package))))
            } else {
                Ok(None)
            }
        }
        dave::OP_MLS_PROPOSALS => {
            tracing::info!("voice: processing DAVE MLS proposals");
            match dave_session.process_proposals(&frame.payload, recognized_users)? {
                Some(commit_welcome) => {
                    tracing::info!("voice: sending DAVE MLS commit and welcome ({} bytes)", commit_welcome.len());
                    Ok(Some(WsMessage::Binary(dave::encode_client_frame(dave::OP_MLS_COMMIT_WELCOME, &commit_welcome))))
                }
                None => Ok(None),
            }
        }
        dave::OP_MLS_ANNOUNCE_COMMIT_TRANSITION => {
            if frame.payload.len() < 2 { return Err(ClientError::GatewayClosed("dave commit frame missing transition ID".into())); }
            let transition_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
            dave_session.process_commit(&frame.payload[2..])?;
            tracing::info!("voice: applied DAVE MLS commit for transition {transition_id}");
            tracing::info!("voice: acknowledging DAVE transition {transition_id}");
            Ok(Some(WsMessage::Text(json!({"op": 23, "d": {"transition_id": transition_id}}).to_string())))
        }
        dave::OP_MLS_WELCOME => {
            if frame.payload.len() < 2 { return Err(ClientError::GatewayClosed("dave welcome frame missing transition ID".into())); }
            let transition_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
            dave_session.process_welcome(&frame.payload[2..])?;
            tracing::info!("voice: applied DAVE MLS welcome for transition {transition_id}");
            tracing::info!("voice: acknowledging DAVE transition {transition_id}");
            Ok(Some(WsMessage::Text(json!({"op": 23, "d": {"transition_id": transition_id}}).to_string())))
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
                { "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120, "rtx_payload_type": null },
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
