use crate::dave::{self, DaveSession};
use crate::error::ClientError;
use crate::voice_audio::{AudioCodec, MicCapture, SpeakerPlayback};
use crate::voice_rtc::{RtcEvent, VoiceRtc};
use crate::voice_sdp;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
#[cfg(feature = "screenshare")]
use std::collections::HashSet;
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
    pub mic: crate::voice_audio::MicProcessing,
    pub user_volumes: Arc<std::sync::Mutex<HashMap<String, f32>>>,
    pub screen_sharing: Arc<AtomicBool>,
}

#[derive(Debug)]
struct EncodedVideoFrame {
    #[cfg(feature = "screenshare")]
    data: Vec<u8>,
    #[cfg(feature = "screenshare")]
    timestamp: u32,
    #[cfg(feature = "screenshare")]
    keyframe: bool,
    #[cfg(feature = "screenshare")]
    width: u32,
    #[cfg(feature = "screenshare")]
    height: u32,
}

struct IncomingVideoPacket {
    #[cfg(feature = "screenshare")]
    ssrc: u32,
    #[cfg(feature = "screenshare")]
    user_id: String,
    #[cfg(feature = "screenshare")]
    payload: Vec<u8>,
    #[cfg(feature = "screenshare")]
    codec: crate::screen_decode::VideoCodec,
}

struct DecodedScreenFrame {
    #[cfg(feature = "screenshare")]
    user_id: String,
    #[cfg(feature = "screenshare")]
    width: u32,
    #[cfg(feature = "screenshare")]
    height: u32,
    #[cfg(feature = "screenshare")]
    rgba: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct VoiceStats {
    pub bitrate_up_bps: u32,
    pub bitrate_down_bps: u32,
    pub ws_rtt_ms: Option<u32>,
    pub rtc_rtt_ms: Option<f32>,
    pub loss_up_pct: Option<f32>,
    pub loss_down_pct: Option<f32>,
    pub jitter_ms: Option<f32>,
    pub endpoint: String,
    pub audio_ssrc: u32,
    pub dave_active: bool,
    pub dave_version: u32,
    pub dave_epoch: Option<u64>,
    pub dave_code: Option<[u8; 4]>,
    pub video_fps: f32,
    pub video_bitrate_down_bps: u32,
    pub video_width: u32,
    pub video_height: u32,
    pub video_codec: String,
    pub video_decrypt_failures: u64,
    pub video_decoder_drops: u64,
    pub video_fps_up: f32,
    pub video_bitrate_up_bps: u32,
    pub audio_stream_bitrate_up_bps: u32,
    pub audio_stream_active: bool,
    pub video_up_width: u32,
    pub video_up_height: u32,
}

const VOICE_VERSION: &str = "v=8";

#[derive(Debug, Clone)]
pub enum VoiceGatewayEvent {
    Connecting,
    Ready,
    MediaConnected,
    ScreenShareStopped,
    #[cfg(feature = "screenshare")]
    VideoFrame { user_id: String, width: u32, height: u32, rgba: Vec<u8> },
    Speaking { ssrc: u32, user_id: Option<String>, speaking: bool },
    DaveReady,
    AudioReceived { payload: Vec<u8> },
    Stats(VoiceStats),
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
    stream_media: bool,
    publish_stream: bool,
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
        stream_media: bool,
        publish_stream: bool,
    ) -> Self {
        Self { endpoint, guild_id, channel_id, user_id, session_id, token, media_state, stream_media, publish_stream }
    }

    pub async fn run(
        &self,
        tx: mpsc::UnboundedSender<VoiceGatewayEvent>,
        mut outgoing: mpsc::UnboundedReceiver<String>,
    ) -> Result<(), ClientError> {
        let _ = tx.send(VoiceGatewayEvent::Connecting);

        if self.stream_media {
            tracing::info!(publish_stream = self.publish_stream, "connecting screen share voice gateway");
        }
        let url = format!("wss://{}/?{VOICE_VERSION}", self.endpoint);
        let (ws_stream, _) = tokio_tungstenite::connect_async(url).await?;
        if self.stream_media {
            tracing::info!("screen share voice websocket connected");
        }
        let (mut write, mut read) = ws_stream.split();

        let hello = read
            .next()
            .await
            .ok_or_else(|| ClientError::GatewayClosed("no voice hello frame".into()))??;
        let hello_payload: Payload = serde_json::from_str(&hello.into_text()?)?;
        if self.stream_media {
            tracing::info!("screen share voice websocket received hello");
        }
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
                "video": self.stream_media,
                "streams": if self.publish_stream {
                    json!([{ "type": "screen", "rid": "100", "quality": 100 }])
                } else {
                    json!([])
                },
                "max_dave_protocol_version": dave::DAVE_PROTOCOL_VERSION,
            }
        });
        write.send(WsMessage::Text(identify.to_string())).await?;

        let mut dave_session = DaveSession::new(&self.user_id)?;
        let local_user_id = self.user_id.parse::<u64>().map_err(|e| ClientError::GatewayClosed(format!("voice: invalid local user id: {e}")))?;
        let mut frame_crypto = crate::dave_frame_crypto::FrameCrypto::new(local_user_id);
        let mut remote_speakers: HashMap<u32, String> = HashMap::new();
        #[cfg(feature = "screenshare")]
        let mut remote_video_speakers: HashMap<u32, String> = HashMap::new();
        let mut recognized_users = vec![self.user_id.clone()];
        let mut dave_enabled = false;
        let mut target_dave_version = 0u32;
        let mut pending_frame_epoch_update = false;
        let mut pending_dave_group_id: Option<u64> = None;
        let mut rtc: Option<VoiceRtc> = None;
        let mut ready_audio_ssrc: Option<u32> = None;
        let mut ready_video_ssrcs: Option<(u32, u32)> = None;
        #[cfg(feature = "screenshare")]
        let mut negotiated_video_codec = crate::screen_decode::VideoCodec::Vp8;
        #[cfg(feature = "screenshare")]
        let mut video_announced = false;
        let mut media_connected = false;
        let mut rtc_buf = vec![0u8; 2000];
        let mut next_rtc_deadline: Option<Instant> = None;
        let mut rtc_socket: Option<std::sync::Arc<tokio::net::UdpSocket>> = None;
        let mut seq_ack: u64 = 0;
        let mut ws_rtt_ms: Option<u32> = None;
        let mut mic_frame_count: u64 = 0;
        let mut last_select_entry: Option<Instant> = None;
        let mut received_audio_frame_count: u64 = 0;
        #[cfg(feature = "screenshare")]
        let mut received_video_packet_count: u64 = 0;
        #[cfg(feature = "screenshare")]
        let mut logged_video_decrypt_result = false;
        #[cfg(feature = "screenshare")]
        let mut video_stats_started = Instant::now();
        #[cfg(feature = "screenshare")]
        let mut video_stats_frames = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_stats_bytes = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_decrypt_failures = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_queue_drops = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_interval_started = Instant::now();
        #[cfg(feature = "screenshare")]
        let mut video_interval_packets = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_interval_bytes = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_up_interval_frames = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_up_interval_bytes = 0u64;
        #[cfg(feature = "screenshare")]
        let mut audio_up_interval_bytes = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_up_width = 0u32;
        #[cfg(feature = "screenshare")]
        let mut video_up_height = 0u32;
        #[cfg(feature = "screenshare")]
        let mut video_width = 0u32;
        #[cfg(feature = "screenshare")]
        let mut video_height = 0u32;
        #[cfg(feature = "screenshare")]
        let mut video_decrypt_failures_total = 0u64;
        #[cfg(feature = "screenshare")]
        let mut video_decoder_drops_total = 0u64;
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
        let mut screen_share_task: Option<tokio::task::JoinHandle<()>> = None;
        let (system_audio_tx, mut system_audio_rx) = mpsc::channel::<Vec<i16>>(6);
        let mut system_audio_pending = std::collections::VecDeque::<i16>::new();
        let mut system_audio_announced = false;
        #[cfg(not(feature = "screenshare"))]
        let _ = &system_audio_tx;
        let (video_frame_tx, mut video_frame_rx) = mpsc::channel(2);
        let mut screen_share_check = tokio::time::interval(Duration::from_millis(250));
        let (video_packet_tx, video_packet_rx) = mpsc::channel(4);
        let (decoded_video_tx, mut decoded_video_rx) = mpsc::channel(2);
        tokio::task::spawn_blocking(move || decode_screen_packets(video_packet_rx, decoded_video_tx));
        #[cfg(not(feature = "screenshare"))]
        let _ = &video_packet_tx;
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
                    #[cfg(feature = "screenshare")]
                    let video_sample_elapsed = video_interval_started.elapsed().as_secs_f32().max(0.001);
                    #[cfg(feature = "screenshare")]
                    let video_fps_sample = video_interval_packets as f32 / video_sample_elapsed;
                    #[cfg(feature = "screenshare")]
                    let video_bitrate_sample = (video_interval_bytes as f32 * 8.0 / video_sample_elapsed).min(u32::MAX as f32) as u32;
                    #[cfg(feature = "screenshare")]
                    let video_fps_up_sample = video_up_interval_frames as f32 / video_sample_elapsed;
                    #[cfg(feature = "screenshare")]
                    let video_bitrate_up_sample = (video_up_interval_bytes as f32 * 8.0 / video_sample_elapsed).min(u32::MAX as f32) as u32;
                    #[cfg(feature = "screenshare")]
                    let audio_stream_bitrate_up_sample = (audio_up_interval_bytes as f32 * 8.0 / video_sample_elapsed).min(u32::MAX as f32) as u32;
                    #[cfg(feature = "screenshare")]
                    let audio_stream_active = audio_up_interval_bytes > 0;
                    #[cfg(feature = "screenshare")]
                    {
                        video_interval_started = Instant::now();
                        video_interval_packets = 0;
                        video_interval_bytes = 0;
                        video_up_interval_frames = 0;
                        video_up_interval_bytes = 0;
                        audio_up_interval_bytes = 0;
                    }
                    const SSRC_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
                    for ssrc in codec.idle_ssrcs(SSRC_IDLE_TIMEOUT) {
                        if remote_speakers.contains_key(&ssrc) {
                            continue;
                        }
                        tracing::debug!("voice: pruning orphaned idle SSRC {ssrc}");
                        codec.drop_ssrc(ssrc);
                        if let Some(active_rtc) = rtc.as_mut() {
                            active_rtc.forget_remote_audio(ssrc);
                        }
                    }
                    let up = (bytes_sent * 8) as u32;
                    let down = (bytes_received * 8) as u32;
                    bytes_sent = 0;
                    bytes_received = 0;
                    let rtc_stats = rtc.as_ref().map(|r| r.stats.clone()).unwrap_or_default();
                    let dave = dave_session.epoch_info();
                    #[cfg(feature = "screenshare")]
                    let (video_fps, video_bitrate_down_bps, video_width, video_height, video_codec, video_decrypt_failures_total, video_decoder_drops_total,
                        video_fps_up, video_bitrate_up_bps, audio_stream_bitrate_up_bps, audio_stream_active, video_up_width, video_up_height) = (
                        video_fps_sample, video_bitrate_sample, video_width, video_height,
                        format!("{:?}", negotiated_video_codec), video_decrypt_failures_total, video_decoder_drops_total,
                        video_fps_up_sample, video_bitrate_up_sample, audio_stream_bitrate_up_sample, audio_stream_active, video_up_width, video_up_height,
                    );
                    #[cfg(not(feature = "screenshare"))]
                    let (video_fps, video_bitrate_down_bps, video_width, video_height, video_codec, video_decrypt_failures_total, video_decoder_drops_total,
                        video_fps_up, video_bitrate_up_bps, audio_stream_bitrate_up_bps, audio_stream_active, video_up_width, video_up_height) = (0.0, 0, 0, 0, String::new(), 0, 0, 0.0, 0, 0, false, 0, 0);
                    let _ = tx.send(VoiceGatewayEvent::Stats(VoiceStats {
                        bitrate_up_bps: up,
                        bitrate_down_bps: down,
                        ws_rtt_ms,
                        rtc_rtt_ms: rtc_stats.rtt_ms,
                        loss_up_pct: rtc_stats.loss_up,
                        loss_down_pct: rtc_stats.loss_down,
                        jitter_ms: rtc_stats.jitter_ms,
                        endpoint: self.endpoint.clone(),
                        audio_ssrc: rtc.as_ref().map(|r| r.audio_ssrc()).unwrap_or_default(),
                        dave_active: dave_enabled,
                        dave_version: target_dave_version,
                        dave_epoch: dave.map(|d| d.0),
                        dave_code: dave.map(|d| d.1),
                        video_fps,
                        video_bitrate_down_bps,
                        video_width,
                        video_height,
                        video_codec,
                        video_decrypt_failures: video_decrypt_failures_total,
                        video_decoder_drops: video_decoder_drops_total,
                        video_fps_up,
                        video_bitrate_up_bps,
                        audio_stream_bitrate_up_bps,
                        audio_stream_active,
                        video_up_width,
                        video_up_height,
                    }));
                }
                _ = device_check.tick(), if media_connected => {
                    let input_device = self.media_state.input_device.lock().unwrap().clone();
                    if attempted_input_device.as_ref() != Some(&input_device) {
                        attempted_input_device = Some(input_device.clone());
                        match MicCapture::start(input_device.as_deref(), self.media_state.muted.clone(), self.media_state.mic.clone(), {
                            let mic_frame_tx = mic_frame_tx.clone();
                            move |frame| { let _ = mic_frame_tx.send(frame); }
                        }) {
                            Ok(new_mic) => {
                                mic = Some(new_mic);
                                tracing::debug!("voice: microphone switched to {}", input_device.as_deref().unwrap_or("Default"));
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
                                tracing::debug!("voice: speaker switched to {}", output_device.as_deref().unwrap_or("Default"));
                            }
                            Err(error) => tracing::warn!("voice: failed to switch speaker to {}: {error}", output_device.as_deref().unwrap_or("Default")),
                        }
                    }
                }
                _ = screen_share_check.tick(), if media_connected && self.publish_stream => {
                    let mut requested = self.media_state.screen_sharing.load(std::sync::atomic::Ordering::Relaxed);
                    if screen_share_task.as_ref().is_some_and(tokio::task::JoinHandle::is_finished) {
                        screen_share_task = None;
                        self.media_state.screen_sharing.store(false, std::sync::atomic::Ordering::Relaxed);
                        requested = false;
                        let _ = tx.send(VoiceGatewayEvent::ScreenShareStopped);
                    }
                    #[cfg(feature = "screenshare")]
                    if !requested && video_announced {
                        let inactive = rtc.as_ref().map(|active| {
                            video_state_frame(active.audio_ssrc(), active.video_ssrc(), active.video_rtx_ssrc(), false)
                        }).unwrap_or_default();
                        if let Err(error) = write.send(WsMessage::Text(inactive)).await {
                            break Err(error.into());
                        }
                        video_announced = false;
                    }
                    #[cfg(feature = "screenshare")]
                    if !requested {
                        system_audio_pending.clear();
                        if system_audio_announced {
                            let audio_ssrc = rtc.as_ref().map(|active| active.audio_ssrc()).unwrap_or_default();
                            if let Err(error) = write.send(WsMessage::Text(speaking_flags_frame(audio_ssrc, 0))).await {
                                break Err(error.into());
                            }
                            system_audio_announced = false;
                        }
                    }
                    match (screen_share_task.is_some(), requested) {
                        (false, true) => {
                            let frames = video_frame_tx.clone();
                            let sharing = self.media_state.screen_sharing.clone();
                            screen_share_task = Some(tokio::spawn(async move {
                                if let Err(error) = capture_and_encode_screen(frames, sharing).await {
                                    tracing::warn!("screen share stopped: {error}");
                                }
                            }));
                            #[cfg(feature = "screenshare")]
                            {
                                let audio = system_audio_tx.clone();
                                let sharing = self.media_state.screen_sharing.clone();
                                tokio::task::spawn_blocking(move || {
                                    if let Err(error) = crate::screen_audio::capture_system_audio(audio, sharing) {
                                        tracing::warn!("system audio capture stopped: {error}");
                                    }
                                });
                            }
                        }
                        (true, false) => {
                            if let Some(task) = screen_share_task.take() {
                                task.abort();
                            }
                        }
                        _ => {}
                    }
                }
                Some(samples) = system_audio_rx.recv(), if self.stream_media && self.publish_stream && media_connected => {
                    system_audio_pending.extend(samples);
                    let mut send_error = None;
                    while system_audio_pending.len() >= crate::voice_audio::FRAME_SAMPLES {
                        let pcm: Vec<i16> = system_audio_pending.drain(..crate::voice_audio::FRAME_SAMPLES).collect();
                        let opus = match codec.encode(&pcm) {
                            Ok(opus) => opus,
                            Err(error) => {
                                tracing::debug!("screen audio: Opus encode failed: {error}");
                                continue;
                            }
                        };
                        let payload = if dave_enabled {
                            let Some(encrypted) = frame_crypto.encrypt(&opus) else {
                                tracing::debug!("screen audio: DAVE encryption failed; dropping audio frame");
                                continue;
                            };
                            encrypted
                        } else { opus };
                        let Some(active_rtc) = rtc.as_mut() else { continue };
                        let audio_ssrc = active_rtc.audio_ssrc();
                        if !system_audio_announced {
                            if let Err(error) = write.send(WsMessage::Text(speaking_flags_frame(audio_ssrc, 4))).await {
                                send_error = Some(error.into());
                                break;
                            }
                            system_audio_announced = true;
                        }
                        bytes_sent = bytes_sent.saturating_add(payload.len() as u64);
                        if let Err(error) = active_rtc.send_audio_frame(&payload) {
                            tracing::debug!("screen audio: failed to send Opus frame: {error}");
                        } else {
                            audio_up_interval_bytes = audio_up_interval_bytes.saturating_add(payload.len() as u64);
                        }
                    }
                    if let Some(error) = send_error { break Err(error); }
                }
                Some(frame) = video_frame_rx.recv() => {
                    #[cfg(feature = "screenshare")]
                    if let Some(active_rtc) = rtc.as_mut() {
                        if !video_announced {
                            let active = video_state_frame(active_rtc.audio_ssrc(), active_rtc.video_ssrc(), active_rtc.video_rtx_ssrc(), true);
                            if let Err(error) = write.send(WsMessage::Text(active)).await {
                                break Err(error.into());
                            }
                            video_announced = true;
                        }
                        let data = if dave_enabled {
                            let clear_header = if frame.keyframe { 10 } else { 1 };
                            let Some(encrypted) = frame_crypto.encrypt_with_unencrypted_ranges(&frame.data, &[(0, clear_header)]) else {
                                tracing::debug!("voice: DAVE encryption failed; dropping VP8 frame");
                                continue;
                            };
                            encrypted
                        } else {
                            frame.data
                        };
                        let frame_bytes = data.len() as u64;
                        bytes_sent = bytes_sent.saturating_add(frame_bytes);
                        let rtc_frame = crate::screen_encode::EncodedFrame { data, timestamp: frame.timestamp, keyframe: frame.keyframe };
                        if let Err(error) = active_rtc.send_video_frame(&rtc_frame) {
                            tracing::debug!("voice: failed to send VP8 frame: {error}");
                        } else {
                            video_up_interval_frames += 1;
                            video_up_interval_bytes = video_up_interval_bytes.saturating_add(frame_bytes);
                            video_up_width = frame.width;
                            video_up_height = frame.height;
                        }
                    }
                    #[cfg(not(feature = "screenshare"))]
                    let _ = frame;
                }
                Some(frame) = decoded_video_rx.recv() => {
                    #[cfg(feature = "screenshare")]
                    {
                        video_width = frame.width;
                        video_height = frame.height;
                    }
                    #[cfg(feature = "screenshare")]
                    let _ = tx.send(VoiceGatewayEvent::VideoFrame {
                        user_id: frame.user_id,
                        width: frame.width,
                        height: frame.height,
                        rgba: frame.rgba,
                    });
                    #[cfg(not(feature = "screenshare"))]
                    let _ = frame;
                }
                frame = read.next() => {
                    {
                        let now = Instant::now();
                        if let Some(prev) = last_select_entry {
                            let gap = now.duration_since(prev);
                            if gap > Duration::from_millis(60) {
                                tracing::trace!("voice: [ws read] {gap:?} since previous select! iteration");
                            }
                        }
                        last_select_entry = Some(now);
                    }
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
                                tracing::trace!("voice: received DAVE control opcode {}, data {:?}", payload.op, payload.d);
                            }
                            if payload.op == 2 {
                                tracing::trace!("voice: op 2 Ready payload: {:?}", payload.d);
                                if let Some(data) = payload.d.as_ref() {
                                    ready_audio_ssrc = data["ssrc"].as_u64().map(|ssrc| ssrc as u32);
                                    ready_video_ssrcs = data["streams"].as_array()
                                        .and_then(|streams| streams.iter().find(|stream| stream["type"].as_str() == Some("video")))
                                        .and_then(|stream| {
                                            let ssrc = stream["ssrc"].as_u64()? as u32;
                                            let rtx = stream["rtx_ssrc"].as_u64().map(|value| value as u32).unwrap_or(ssrc.wrapping_add(1));
                                            Some((ssrc, rtx))
                                        });
                                }
                                if self.stream_media {
                                    tracing::info!(has_video_ssrc = ready_video_ssrcs.is_some(), "screen share voice gateway received READY");
                                }
                                if let Err(e) = write.send(WsMessage::Text(json!({ "op": 16, "d": {} }).to_string())).await {
                                    break Err(e.into());
                                }
                            } else if payload.op == 16 {
                                let (mut new_rtc, params) = match VoiceRtc::new(ready_audio_ssrc, ready_video_ssrcs).await {
                                    Ok(v) => v,
                                    Err(e) => break Err(e),
                                };
                                for &remote_ssrc in remote_speakers.keys() {
                                    new_rtc.expect_remote_audio(remote_ssrc);
                                }
                                #[cfg(feature = "screenshare")]
                                for &remote_ssrc in remote_video_speakers.keys() {
                                    new_rtc.expect_remote_video(remote_ssrc);
                                }
                                let offer = voice_sdp::build_offer(&params);
                                rtc_socket = Some(new_rtc.socket_handle());
                                rtc = Some(new_rtc);
                                let select_protocol = select_protocol_frame(&offer, self.publish_stream);
                                if let Err(e) = write.send(WsMessage::Text(select_protocol)).await {
                                    break Err(e.into());
                                }
                                let _ = tx.send(VoiceGatewayEvent::Ready);
                            } else if payload.op == 4 {
                                if let Some(data) = payload.d.as_ref() {
                                    #[cfg(feature = "screenshare")]
                                    if let Some(codec) = data["video_codec"].as_str() {
                                        negotiated_video_codec = crate::screen_decode::VideoCodec::from_gateway(codec);
                                        tracing::debug!("voice: negotiated remote video codec {codec}");
                                    }
                                    target_dave_version = data["dave_protocol_version"].as_u64().unwrap_or(0) as u32;
                                    if target_dave_version > 0 {
                                        tracing::debug!("voice: gateway selected DAVE protocol version {target_dave_version}");
                                        pending_dave_group_id = data["group_id"].as_u64()
                                            .or_else(|| {
                                                if self.stream_media {
                                                    self.guild_id.parse::<u64>().ok()?.checked_sub(1)
                                                } else {
                                                    self.channel_id.parse::<u64>().ok()
                                                }
                                            });
                                        if dave_session.has_external_sender() {
                                            if let Some(group_id) = pending_dave_group_id.take() {
                                                match dave_session.create_new_group(group_id) {
                                                    Ok(package) => {
                                                        tracing::debug!("voice: sending initial DAVE MLS key package");
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
                                if self.stream_media {
                                    tracing::info!(remote_ice_candidates = remote.candidates.len(), "applying screen share RTC session description");
                                }
                                if let Err(e) = active_rtc.set_remote(&remote) {
                                    tracing::warn!("voice: set_remote failed: {e}");
                                    break Err(e);
                                }
                            } else if payload.op == 5 {
                                if let Some(data) = &payload.d {
                                    let ssrc = data["ssrc"].as_u64().unwrap_or_default() as u32;
                                    let speaker_id = data["user_id"].as_str();
                                    #[cfg(feature = "screenshare")]
                                    let video_ssrc = data["video_ssrc"].as_u64().unwrap_or_default() as u32;
                                    if ssrc != 0 && speaker_id != Some(self.user_id.as_str()) {
                                        if let Some(user_id) = speaker_id {
                                            tracing::debug!("voice: mapped remote audio SSRC {ssrc} to user {user_id}");
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
                                        codec.track_ssrc(ssrc);
                                        if let Some(active_rtc) = rtc.as_mut() {
                                            active_rtc.expect_remote_audio(ssrc);
                                        }
                                        #[cfg(feature = "screenshare")]
                                        if let (Some(user_id), true) = (speaker_id, video_ssrc != 0) {
                                            if self.stream_media {
                                                tracing::info!("screen share RTC mapped remote video SSRC");
                                            }
                                            remote_video_speakers.insert(video_ssrc, user_id.to_string());
                                            if let Some(active_rtc) = rtc.as_mut() {
                                                active_rtc.expect_remote_video(video_ssrc);
                                            }
                                            let wants = media_sink_wants(video_ssrc);
                                            if let Err(error) = write.send(WsMessage::Text(wants)).await {
                                                break Err(error.into());
                                            }
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
                                    tracing::debug!("voice: gateway recognized {} DAVE users", recognized_users.len());
                                }
                            } else if payload.op == 13 {
                                if let Some(id) = payload.d.as_ref().and_then(|d| d["user_id"].as_str()) {
                                    recognized_users.retain(|known| known != id || known == &self.user_id);
                                    let departed: Vec<u32> = remote_speakers
                                        .iter()
                                        .filter(|(_, user)| user.as_str() == id)
                                        .map(|(ssrc, _)| *ssrc)
                                        .collect();
                                    for ssrc in departed {
                                        remote_speakers.remove(&ssrc);
                                        codec.drop_ssrc(ssrc);
                                        if let Some(active_rtc) = rtc.as_mut() {
                                            active_rtc.forget_remote_audio(ssrc);
                                        }
                                    }
                                    #[cfg(feature = "screenshare")]
                                    {
                                        let departed_video: Vec<u32> = remote_video_speakers
                                            .iter()
                                            .filter(|(_, user)| user.as_str() == id)
                                            .map(|(ssrc, _)| *ssrc)
                                            .collect();
                                        for ssrc in departed_video {
                                            remote_video_speakers.remove(&ssrc);
                                            if let Some(active_rtc) = rtc.as_mut() {
                                                active_rtc.forget_remote_video(ssrc);
                                            }
                                        }
                                    }
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
                                tracing::debug!("voice: executing DAVE transition; protocol version {}, enabled={dave_enabled}", target_dave_version);
                                let _ = tx.send(VoiceGatewayEvent::DaveReady);
                            } else if payload.op == 12 {
                                if let Some(data) = &payload.d {
                                    let user_id = data["user_id"].as_str();
                                    let audio_ssrc = data["audio_ssrc"].as_u64().unwrap_or_default() as u32;
                                    #[cfg(feature = "screenshare")]
                                    let video_ssrc = data["video_ssrc"].as_u64().unwrap_or_default() as u32;
                                    tracing::debug!("voice: op 12 client connect, user {:?}, audio_ssrc {audio_ssrc}", user_id);
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
                                        codec.track_ssrc(audio_ssrc);
                                        if let Some(active_rtc) = rtc.as_mut() {
                                            active_rtc.expect_remote_audio(audio_ssrc);
                                        }
                                    }
                                    #[cfg(feature = "screenshare")]
                                    if let (Some(user_id), true) = (user_id, video_ssrc != 0) {
                                        if self.stream_media {
                                            tracing::info!("screen share RTC announced remote video SSRC");
                                        }
                                        remote_video_speakers.insert(video_ssrc, user_id.to_string());
                                        if let Some(active_rtc) = rtc.as_mut() {
                                            active_rtc.expect_remote_video(video_ssrc);
                                        }
                                        let wants = media_sink_wants(video_ssrc);
                                        if let Err(error) = write.send(WsMessage::Text(wants)).await {
                                            break Err(error.into());
                                        }
                                    }
                                }
                            } else if payload.op == 6 {
                                let sent = payload.d.as_ref().and_then(|d| d.get("t").and_then(Value::as_u64).or_else(|| d.as_u64()));
                                if let Some(sent) = sent {
                                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                                    ws_rtt_ms = Some(now.saturating_sub(sent) as u32);
                                }
                            } else if !matches!(payload.op, 3 | 8) {
                                tracing::trace!("voice: unhandled opcode {}, data {:?}", payload.op, payload.d);
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
                            tracing::trace!("voice: received DAVE binary opcode {:?}", opcode);
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
                                        tracing::debug!("voice: executed initial DAVE transition 0 locally; protocol version {}, enabled={dave_enabled}", target_dave_version);
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
                drive_result = drive_rtc(rtc.as_mut()) => {
                    {
                        let now = Instant::now();
                        if let Some(prev) = last_select_entry {
                            let gap = now.duration_since(prev);
                            if gap > Duration::from_millis(60) {
                                tracing::trace!("voice: [drive_rtc] {gap:?} since previous select! iteration");
                            }
                        }
                        last_select_entry = Some(now);
                    }
                    let rtc_event = match drive_result {
                        Ok(crate::voice_rtc::DriveResult::WaitUntil(deadline)) => {
                            next_rtc_deadline = Some(deadline);
                            Ok(None)
                        }
                        Ok(crate::voice_rtc::DriveResult::Event(event)) => Ok(Some(event)),
                        Err(e) => Err(e),
                    };
                    match rtc_event {
                        Ok(Some(RtcEvent::Connected)) => {
                            media_connected = true;
                            if self.stream_media {
                                tracing::info!("screen share RTC media transport connected");
                            }
                            let audio_ssrc = rtc.as_ref().map(|r| r.audio_ssrc()).unwrap_or_default();
                            #[cfg(feature = "screenshare")]
                            let client_connect = rtc.as_ref().map(|r| {
                                if self.publish_stream || (!self.stream_media && ready_video_ssrcs.is_some()) {
                                    video_state_frame(audio_ssrc, r.video_ssrc(), r.video_rtx_ssrc(), false)
                                } else {
                                    json!({ "op": 12, "d": { "audio_ssrc": audio_ssrc, "video_ssrc": 0, "rtx_ssrc": 0, "streams": [] } }).to_string()
                                }
                            }).unwrap_or_default();
                            #[cfg(not(feature = "screenshare"))]
                            let client_connect = json!({ "op": 12, "d": { "audio_ssrc": audio_ssrc, "video_ssrc": 0, "rtx_ssrc": 0, "streams": [] } }).to_string();
                            if let Err(e) = write.send(WsMessage::Text(client_connect.to_string())).await {
                                break Err(e.into());
                            }
                            if !self.stream_media {
                                if let Err(e) = write.send(WsMessage::Text(speaking_frame(audio_ssrc, true))).await {
                                    break Err(e.into());
                                }
                            }

                            if !self.stream_media {
                                let input_device = self.media_state.input_device.lock().unwrap().clone();
                                attempted_input_device = Some(input_device.clone());
                                match MicCapture::start(input_device.as_deref(), self.media_state.muted.clone(), self.media_state.mic.clone(), {
                                    let mic_frame_tx = mic_frame_tx.clone();
                                    move |frame| { let _ = mic_frame_tx.send(frame); }
                                }) {
                                    Ok(m) => {
                                        tracing::debug!("voice: mic capture started");
                                        mic = Some(m);
                                    }
                                    Err(e) => tracing::warn!("voice: failed to start mic capture: {e}"),
                                }
                            }
                            let output_device = self.media_state.output_device.lock().unwrap().clone();
                            attempted_output_device = Some(output_device.clone());
                            match SpeakerPlayback::start(output_device.as_deref(), self.media_state.deafened.clone()) {
                                Ok(s) => {
                                    tracing::debug!("voice: speaker playback started");
                                    speaker = Some(s);
                                }
                                Err(e) => tracing::warn!("voice: failed to start speaker playback: {e}"),
                            }

                            let _ = tx.send(VoiceGatewayEvent::MediaConnected);
                        }
                        Ok(Some(RtcEvent::Disconnected)) => {
                            break Err(ClientError::GatewayClosed("voice media connection disconnected".into()));
                        }
                        #[cfg(feature = "screenshare")]
                        Ok(Some(RtcEvent::VideoData { ssrc, payload })) => {
                            received_video_packet_count += 1;
                            video_stats_frames += 1;
                            video_stats_bytes = video_stats_bytes.saturating_add(payload.len() as u64);
                            video_interval_packets += 1;
                            video_interval_bytes = video_interval_bytes.saturating_add(payload.len() as u64);
                            if self.stream_media && received_video_packet_count == 1 {
                                tracing::info!(ssrc, bytes = payload.len(), dave_active = dave_enabled, "received first screen share RTP video packet");
                            }
                            let stats_elapsed = video_stats_started.elapsed();
                            if stats_elapsed >= Duration::from_secs(2) {
                                if self.stream_media || self.publish_stream {
                                    tracing::info!(
                                        frames = video_stats_frames,
                                        fps = video_stats_frames as f32 / stats_elapsed.as_secs_f32(),
                                        receive_kbps = video_stats_bytes as f32 * 8.0 / stats_elapsed.as_secs_f32() / 1000.0,
                                        average_frame_bytes = video_stats_bytes / video_stats_frames.max(1),
                                        codec = ?negotiated_video_codec,
                                        dave_active = dave_enabled,
                                        decrypt_failures = video_decrypt_failures,
                                        decoder_queue_drops = video_queue_drops,
                                        "screen share incoming video stats"
                                    );
                                }
                                video_stats_started = Instant::now();
                                video_stats_frames = 0;
                                video_stats_bytes = 0;
                                video_decrypt_failures = 0;
                                video_queue_drops = 0;
                            }
                            let Some(user_id) = remote_video_speakers.get(&ssrc).cloned() else {
                                tracing::trace!("voice: video frame for unknown SSRC {ssrc}");
                                continue;
                            };
                            let payload = if dave_enabled {
                                let Ok(numeric_id) = user_id.parse::<u64>() else { continue };
                                let Some(clear) = frame_crypto.decrypt(numeric_id, &payload) else {
                                    video_decrypt_failures += 1;
                                    video_decrypt_failures_total += 1;
                                    if self.stream_media && !logged_video_decrypt_result {
                                        tracing::warn!("DAVE could not decrypt the first screen share video packet");
                                        logged_video_decrypt_result = true;
                                    }
                                    tracing::debug!("voice: failed to decrypt video frame for user {user_id}");
                                    continue;
                                };
                                if self.stream_media && !logged_video_decrypt_result {
                                    tracing::info!("DAVE decrypted the first screen share video packet");
                                    logged_video_decrypt_result = true;
                                }
                                clear
                            } else {
                                payload
                            };
                            let packet = IncomingVideoPacket { ssrc, user_id, payload, codec: negotiated_video_codec };
                            if video_packet_tx.try_send(packet).is_err() {
                                video_queue_drops += 1;
                                video_decoder_drops_total += 1;
                                tracing::trace!("voice: dropping incoming video frame because decoder queue is full");
                            }
                        }
                        Ok(Some(RtcEvent::AudioData { ssrc, payload })) => {
                            received_audio_frame_count += 1;
                            bytes_received += payload.len() as u64;
                            if received_audio_frame_count == 1 || received_audio_frame_count % 50 == 0 {
                                tracing::trace!("voice: received {received_audio_frame_count} decoded RTP audio events; latest SSRC {ssrc}, {} bytes, DAVE enabled={dave_enabled}", payload.len());
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
                                    tracing::debug!("voice: no speaker identity for encrypted stream SSRC {ssrc}");
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
                                Ok(mut pcm) => {
                                    decode_success_count += 1;
                                    let user_gain = remote_speakers
                                        .get(&ssrc)
                                        .and_then(|user| self.media_state.user_volumes.lock().unwrap().get(user).copied())
                                        .unwrap_or(1.0);
                                    if user_gain != 1.0 {
                                        for s in pcm.iter_mut() {
                                            *s = (*s as f32 * user_gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                                        }
                                    }
                                    if decode_success_count == 1 || decode_success_count % 50 == 0 {
                                        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                                        tracing::trace!("voice: decoded {decode_success_count} audio frames so far, latest peak sample magnitude {peak}");
                                    }
                                    if let Some(speaker) = speaker.as_ref() {
                                        speaker.push_frame(ssrc, pcm);
                                    } else {
                                        tracing::debug!("voice: decoded audio but no speaker available");
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
                recv = recv_rtc_datagram(rtc_socket.as_ref(), &mut rtc_buf) => {
                    match recv {
                        Ok((len, source)) => {
                            if let Some(active_rtc) = rtc.as_mut() {
                                if let Err(e) = active_rtc.handle_datagram(source, &rtc_buf[..len]) {
                                    break Err(e);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("voice: rtc datagram recv failed: {e}");
                            break Err(e);
                        }
                    }
                }
                _ = sleep_until_deadline(next_rtc_deadline) => {
                    next_rtc_deadline = None;
                    if let Some(active_rtc) = rtc.as_mut() {
                        if let Err(e) = active_rtc.handle_timeout() {
                            break Err(e);
                        }
                    }
                }
                Some(pcm_frame) = mic_frame_rx.recv() => {
                    let now = Instant::now();
                    if let Some(prev) = last_select_entry {
                        let gap = now.duration_since(prev);
                        if gap > Duration::from_millis(60) {
                            tracing::trace!("voice: [mic_frame_rx] {gap:?} since previous select! iteration (queue depth now {}, DAVE enabled={dave_enabled})", mic_frame_rx.len());
                        }
                    }
                    last_select_entry = Some(now);
                    mic_frame_count += 1;
                    if mic_frame_count % 50 == 0 {
                        tracing::trace!("voice: sent {mic_frame_count} mic frames so far");
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
                                        tracing::debug!("voice: DAVE encryption failed; dropping audio frame");
                                        continue;
                                    }
                                }
                            } else {
                                opus_payload
                            };
                            if let Some(active_rtc) = rtc.as_mut() {
                                if mic_frame_count == 1 || mic_frame_count % 100 == 0 {
                                    tracing::trace!("voice: submitting {} byte Opus frame to RTC, DAVE enabled={dave_enabled}", outgoing_payload.len());
                                }
                                bytes_sent += outgoing_payload.len() as u64;
                                if let Err(e) = active_rtc.send_audio_frame(&outgoing_payload) {
                                    tracing::debug!("voice: failed to send audio frame: {e}");
                                }
                            } else {
                                tracing::debug!("voice: have mic frame but no active rtc");
                            }
                        }
                        Err(e) => tracing::debug!("voice: opus encode failed: {e}"),
                    }
                }
            }
        };

        drop(mic);
        drop(speaker);
        self.media_state.screen_sharing.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(task) = screen_share_task.take() {
            task.abort();
        }

        heartbeat_task.abort();
        let _ = tx.send(VoiceGatewayEvent::Disconnected { reason: result.as_ref().err().map(|e| e.to_string()).unwrap_or_default() });
        result
    }
}

async fn capture_and_encode_screen(
    frames: mpsc::Sender<EncodedVideoFrame>,
    sharing: Arc<AtomicBool>,
) -> Result<(), String> {
    #[cfg(feature = "screenshare")]
    {
        let capture = crate::screen_capture::ScreenCapture::start()
            .await
            .map_err(|error| error.to_string())?;
        tokio::task::spawn_blocking(move || encode_screen_frames(capture, frames, sharing))
            .await
            .map_err(|error| format!("screen encoder worker failed: {error}"))?
    }
    #[cfg(not(feature = "screenshare"))]
    {
        let _ = frames;
        let _ = sharing;
        Err("screen sharing was not enabled in this build".into())
    }
}

#[cfg(feature = "screenshare")]
fn decode_screen_packets(
    packets: mpsc::Receiver<IncomingVideoPacket>,
    frames: mpsc::Sender<DecodedScreenFrame>,
) {
    let mut packets = packets;
    let mut decoders: HashMap<u32, (crate::screen_decode::VideoCodec, Option<crate::screen_decode::VideoDecoder>)> = HashMap::new();
    let mut logged_first_packet: HashSet<u32> = HashSet::new();
    let mut logged_first_frame: HashSet<u32> = HashSet::new();
    let mut decode_stats_started = Instant::now();
    let mut decode_stats_frames = 0u64;
    while let Some(packet) = packets.blocking_recv() {
        let entry = decoders.entry(packet.ssrc).or_insert_with(|| (packet.codec, None));
        if entry.0 != packet.codec {
            *entry = (packet.codec, None);
        }
        if entry.1.is_none() {
            entry.1 = match crate::screen_decode::VideoDecoder::new(packet.codec) {
                Ok(decoder) => Some(decoder),
                Err(error) => {
                    tracing::warn!("could not initialize {:?} receive decoder: {error}", packet.codec);
                    None
                }
            };
        }
        let Some(decoder) = entry.1.as_mut() else { continue };
        match decoder.decode(&packet.payload) {
            Ok(Some(frame)) => {
                decode_stats_frames += 1;
                if logged_first_frame.insert(packet.ssrc) {
                    tracing::info!(codec = ?packet.codec, width = frame.width, height = frame.height, "decoded first remote screen share frame");
                }
                let elapsed = decode_stats_started.elapsed();
                if elapsed >= Duration::from_secs(2) {
                    tracing::info!(
                        ssrc = packet.ssrc,
                        codec = ?packet.codec,
                        decoded_fps = decode_stats_frames as f32 / elapsed.as_secs_f32(),
                        width = frame.width,
                        height = frame.height,
                        "screen share decoder stats"
                    );
                    decode_stats_started = Instant::now();
                    decode_stats_frames = 0;
                }
                let decoded = DecodedScreenFrame {
                    user_id: packet.user_id,
                    width: frame.width,
                    height: frame.height,
                    rgba: frame.rgba,
                };
                if frames.blocking_send(decoded).is_err() {
                    break;
                }
            }
            Ok(None) => {
                if logged_first_packet.insert(packet.ssrc) {
                    tracing::info!(codec = ?packet.codec, "decoder accepted the first screen share packet but has not produced a frame");
                }
            }
            Err(error) => {
                if logged_first_packet.insert(packet.ssrc) {
                    tracing::warn!("first remote {:?} screen share packet failed to decode: {error}", packet.codec);
                } else {
                    tracing::debug!("could not decode remote {:?} frame: {error}", packet.codec);
                }
            }
        }
    }
}

#[cfg(not(feature = "screenshare"))]
fn decode_screen_packets(
    packets: mpsc::Receiver<IncomingVideoPacket>,
    frames: mpsc::Sender<DecodedScreenFrame>,
) {
    drop(packets);
    drop(frames);
}

#[cfg(feature = "screenshare")]
fn encode_screen_frames(
    mut capture: crate::screen_capture::ScreenCapture,
    frames: mpsc::Sender<EncodedVideoFrame>,
    sharing: Arc<AtomicBool>,
) -> Result<(), String> {
    use crate::screen_encode::{EncoderSettings, Vp8Encoder};

    let mut encoder: Option<Vp8Encoder> = None;
    while sharing.load(std::sync::atomic::Ordering::Relaxed) {
        let Some(frame) = capture.blocking_next_frame() else { break };
        if encoder.as_ref().is_none_or(|current| !current.matches_source(frame.width, frame.height, frame.layout)) {
            let settings = EncoderSettings::default();
            let new_encoder = Vp8Encoder::new(frame.width, frame.height, frame.layout, settings)
                .map_err(|error| error.to_string())?;
            let (target_width, target_height) = new_encoder.target_size();
            tracing::info!(
                source_width = frame.width,
                source_height = frame.height,
                encoded_width = target_width,
                encoded_height = target_height,
                bitrate_bps = settings.bitrate_bps,
                "screen share VP8 encoder configured"
            );
            encoder = Some(new_encoder);
        }
        let encoded = encoder
            .as_mut()
            .expect("encoder initialized")
            .encode(&frame)
            .map_err(|error| error.to_string())?;
        let (encoded_width, encoded_height) = encoder.as_ref().expect("encoder initialized").target_size();
        for frame in encoded {
            if frames.try_send(EncodedVideoFrame { data: frame.data, timestamp: frame.timestamp, keyframe: frame.keyframe, width: encoded_width, height: encoded_height }).is_err() {
                tracing::trace!("screen share: dropping encoded frame because the RTP queue is full");
            }
        }
    }
    Ok(())
}

async fn drive_rtc(rtc: Option<&mut VoiceRtc>) -> Result<crate::voice_rtc::DriveResult, ClientError> {
    match rtc {
        Some(rtc) => rtc.drive().await,
        None => std::future::pending().await,
    }
}

async fn recv_rtc_datagram(
    socket: Option<&std::sync::Arc<tokio::net::UdpSocket>>,
    buf: &mut [u8],
) -> Result<(usize, std::net::SocketAddr), ClientError> {
    match socket {
        Some(socket) => socket
            .recv_from(buf)
            .await
            .map_err(|e| ClientError::GatewayClosed(format!("voice rtc: udp recv failed: {e}"))),
        None => std::future::pending().await,
    }
}

async fn sleep_until_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
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
                tracing::debug!("voice: sending DAVE MLS key package for group initialization");
                Ok(Some(WsMessage::Binary(dave::encode_client_frame(dave::OP_MLS_KEY_PACKAGE, &package))))
            } else {
                Ok(None)
            }
        }
        dave::OP_MLS_PROPOSALS => {
            tracing::debug!("voice: processing DAVE MLS proposals");
            match dave_session.process_proposals(&frame.payload, recognized_users)? {
                Some(commit_welcome) => {
                    tracing::debug!("voice: sending DAVE MLS commit and welcome ({} bytes)", commit_welcome.len());
                    Ok(Some(WsMessage::Binary(dave::encode_client_frame(dave::OP_MLS_COMMIT_WELCOME, &commit_welcome))))
                }
                None => Ok(None),
            }
        }
        dave::OP_MLS_ANNOUNCE_COMMIT_TRANSITION => {
            if frame.payload.len() < 2 { return Err(ClientError::GatewayClosed("dave commit frame missing transition ID".into())); }
            let transition_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
            dave_session.process_commit(&frame.payload[2..])?;
            tracing::debug!("voice: applied DAVE MLS commit for transition {transition_id}");
            tracing::debug!("voice: acknowledging DAVE transition {transition_id}");
            Ok(Some(WsMessage::Text(json!({"op": 23, "d": {"transition_id": transition_id}}).to_string())))
        }
        dave::OP_MLS_WELCOME => {
            if frame.payload.len() < 2 { return Err(ClientError::GatewayClosed("dave welcome frame missing transition ID".into())); }
            let transition_id = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
            dave_session.process_welcome(&frame.payload[2..])?;
            tracing::debug!("voice: applied DAVE MLS welcome for transition {transition_id}");
            tracing::debug!("voice: acknowledging DAVE transition {transition_id}");
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

pub fn select_protocol_frame(offer: &str, publish_stream: bool) -> String {
    let mut codecs = vec![json!({ "name": "opus", "type": "audio", "priority": 1000, "payload_type": 120, "rtx_payload_type": null })];
    if publish_stream {
        codecs.push(json!({ "name": "VP8", "type": "video", "priority": 1000, "payload_type": 107, "rtx_payload_type": 108 }));
    } else {
        codecs.push(json!({ "name": "H264", "type": "video", "priority": 3000, "payload_type": 105, "rtx_payload_type": 106 }));
        codecs.push(json!({ "name": "VP8", "type": "video", "priority": 4000, "payload_type": 107, "rtx_payload_type": 108 }));
    }
    json!({
        "op": 1,
        "d": {
            "protocol": "webrtc",
            "data": offer,
            "sdp": offer,
            "codecs": codecs,
            "rtc_connection_id": random_uuid_v4(),
        }
    })
    .to_string()
}

pub fn speaking_frame(ssrc: u32, speaking: bool) -> String {
    speaking_flags_frame(ssrc, if speaking { 1 } else { 0 })
}

fn speaking_flags_frame(ssrc: u32, speaking: u8) -> String {
    json!({
        "op": 5,
        "d": {
            "speaking": speaking,
            "delay": 0,
            "ssrc": ssrc,
        }
    })
    .to_string()
}

#[cfg(feature = "screenshare")]
fn video_state_frame(audio_ssrc: u32, video_ssrc: u32, rtx_ssrc: u32, active: bool) -> String {
    let selected_video_ssrc = if active { video_ssrc } else { 0 };
    let selected_rtx_ssrc = if active { rtx_ssrc } else { 0 };
    json!({
        "op": 12,
        "d": {
            "audio_ssrc": audio_ssrc,
            "video_ssrc": selected_video_ssrc,
            "rtx_ssrc": selected_rtx_ssrc,
            "streams": [{
                "type": "video",
                "rid": "100",
                "ssrc": video_ssrc,
                "active": active,
                "quality": 100,
                "rtx_ssrc": rtx_ssrc,
                "max_bitrate": 9000000,
                "max_framerate": 60,
                "max_resolution": { "type": "source", "width": 0, "height": 0 }
            }]
        }
    })
    .to_string()
}

#[cfg(feature = "screenshare")]
fn media_sink_wants(video_ssrc: u32) -> String {
    json!({
        "op": 15,
        "d": {
            video_ssrc.to_string(): 100,
            "any": 100,
            "pixelCounts": { video_ssrc.to_string(): 0 }
        }
    }).to_string()
}
