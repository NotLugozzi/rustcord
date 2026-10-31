use crate::presence;
use crate::session_store;
use rustcord_client::captcha;
use rustcord_client::error::ClientError;
use rustcord_client::gateway::{Gateway, GatewayEvent};
use rustcord_client::rest::{CaptchaChallenge, LoginResponse, LoginSession, RestClient};
use tokio::sync::mpsc;

fn mime_guess_from_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

pub(crate) static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn cache_file(url: &str) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.cache_dir().join("images").join(format!("{:016x}", h.finish())))
}

const MAX_DISK_CACHED_BYTES: usize = 1024 * 1024;

async fn fetch_image_bytes(url: &str) -> Option<Vec<u8>> {
    let path = cache_file(url);
    if let Some(bytes) = path.as_ref().and_then(|p| std::fs::read(p).ok()) {
        return Some(bytes);
    }
    let client = HTTP.get_or_init(reqwest::Client::new);
    let resp = client.get(url).send().await.and_then(|r| r.error_for_status());
    let bytes = match resp {
        Ok(r) => r.bytes().await.map_err(|e| tracing::warn!("failed to read image bytes for {url}: {e}")).ok()?.to_vec(),
        Err(e) => {
            tracing::warn!("failed to download image {url}: {e}");
            return None;
        }
    };
    if let Some(p) = path.filter(|_| bytes.len() <= MAX_DISK_CACHED_BYTES) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, &bytes);
    }
    Some(bytes)
}

async fn save_attachment(url: &str, filename: &str) -> Result<std::path::PathBuf, String> {
    use tokio::io::AsyncWriteExt;
    let dir = directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .ok_or("no Downloads folder")?;
    let name = std::path::Path::new(filename).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("attachment");
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    let mut path = dir.join(name);
    for n in 1.. {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("{stem} ({n}){ext}"));
    }
    let client = HTTP.get_or_init(reqwest::Client::new);
    let mut resp = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let mut file = tokio::fs::File::create(&path).await.map_err(|e| e.to_string())?;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(path)
}

pub(crate) static GATEWAY_TX: std::sync::Mutex<Option<mpsc::UnboundedSender<String>>> = std::sync::Mutex::new(None);
pub(crate) static VOICE_MEDIA_STATE: std::sync::Mutex<Option<rustcord_client::voice_gateway::VoiceMediaState>> = std::sync::Mutex::new(None);

pub enum UiCommand {
    LoginWithPassword { email: String, password: String },
    LoginWithToken { token: String },
    SubmitMfa { ticket: String, code: String },
    SelectGuild { guild_id: String },
    LoadAllGuildEmojis { guild_ids: Vec<String> },
    SelectChannel { channel_id: String },
    LoadOlderMessages { channel_id: String, before: String },
    SendMessage { channel_id: String, content: String, reply_to: Option<String> },
    SendAttachment { channel_id: String, content: String, path: std::path::PathBuf, reply_to: Option<String> },
    SendAttachmentBytes {
        channel_id: String,
        content: String,
        filename: String,
        bytes: Vec<u8>,
        content_type: String,
        reply_to: Option<String>,
    },
    DownloadImage { url: String },
    #[cfg(feature = "video")]
    OpenVideo { url: String },
    #[cfg(feature = "video")]
    LoadVideoPoster { url: String },
    #[cfg(feature = "video")]
    DecodeVideoBatch { url: String },
    #[cfg(feature = "video")]
    CloseVideo { url: String },
    #[cfg(feature = "video")]
    SeekVideo { url: String, target_secs: f64 },
    SaveAttachment { url: String, filename: String },
    OpenDmWithUser { user_id: String },
    RefreshDmChannels,
    StartTyping { channel_id: String },
    LoadProfile { user_id: String },
    EditMessage { channel_id: String, message_id: String, content: String },
    DeleteMessage { channel_id: String, message_id: String },
    AckMessage { channel_id: String, message_id: String },
    JoinGuildByInvite { invite_code: String },
    CreateGuild { name: String },
    RequestGuildMembers { guild_id: String },
    KickMember { guild_id: String, user_id: String },
    BanMember { guild_id: String, user_id: String },
    SetPresence { status: String, custom_status: Option<String> },
    SetManualActivity { name: String, details: String, state: String },
    SetRpcEnabled { conn_id: u64, enabled: bool },
    JoinVoiceChannel { guild_id: Option<String>, channel_id: String, ring: bool },
    DeclineCall { channel_id: String },
    LeaveVoiceChannel { guild_id: Option<String> },
    SetVoiceMuteDeaf { guild_id: Option<String>, channel_id: String, self_mute: bool, self_deaf: bool },
    SetVoiceInputDevice { device: Option<String> },
    SetMicProcessing { gain: f32, denoise: bool },
    SetUserVolume { user_id: String, volume: f32 },
    SetVoiceOutputDevice { device: Option<String> },
    LoadVoiceDevices,
    RunMicTest { input_device: Option<String>, output_device: Option<String> },
    Logout,
}

pub enum BackendEvent {
    MfaRequired { ticket: String },
    CaptchaInProgress,
    LoginFailed(String),
    LoggedIn,
    Error(String),
    RpcApps(Vec<crate::presence::RpcAppInfo>),
    ProfileLoaded { user_id: String, profile: rustcord_client::models::UserProfile, user: Option<rustcord_client::models::User> },
    Gateway(GatewayEvent),
    GuildChannelsLoaded { guild_id: String, channels: Vec<rustcord_client::models::Channel> },
    GuildRolesLoaded { guild_id: String, roles: Vec<rustcord_client::models::Role> },
    MyGuildMemberLoaded { guild_id: String, roles: Vec<String> },
    GuildEmojisLoaded { guild_id: String, emojis: Vec<rustcord_client::models::Emoji> },
    DmChannelsLoaded(Vec<rustcord_client::models::Channel>),
    MessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    OlderMessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    RelationshipsLoaded(Vec<rustcord_client::models::Relationship>),
    DmOpened(rustcord_client::models::Channel),
    ImageDownloaded { url: String, image: crate::images::Decoded },
    #[cfg(feature = "video")]
    VideoBatchReady(crate::images::VideoBatch),
    VoiceConnecting,
    VoiceConnected,
    VoiceDisconnected,
    VoiceSpeaking { user_id: String, speaking: bool },
    VoiceStats(rustcord_client::voice_gateway::VoiceStats),
    Notice(String),
    VoiceDevicesLoaded { inputs: Vec<String>, outputs: Vec<String> },
    MemberRemoved { guild_id: String, user_id: String },
    MicTestStarted,
    MicTestFinished,
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
) {
    rt.spawn(crate::ipc::serve(event_tx.clone()));
    rt.spawn(async move {
        let mut rest: Option<RestClient> = None;
        let mut login_session: Option<LoginSession> = None;

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                UiCommand::LoginWithPassword { email, password } => {
                    let session = match LoginSession::new() {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                            continue;
                        }
                    };
                    attempt_login(email, password, None, &session, &mut rest, &event_tx).await;
                    login_session = Some(session);
                }
                UiCommand::SubmitMfa { ticket, code } => {
                    let Some(session) = &login_session else {
                        let _ = event_tx.send(BackendEvent::LoginFailed("no active login session".into()));
                        continue;
                    };
                    match session.submit_mfa(&ticket, &code).await {
                        Ok(token) => start_session(token, &mut rest, &event_tx).await,
                        Err(e) => {
                            tracing::error!("mfa submit failed: {e}");
                            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                        }
                    }
                }
                UiCommand::SelectGuild { guild_id } => {
                    if let Some(client) = &rest {
                        match client.guild_channels(&guild_id).await {
                            Ok(channels) => {
                                let _ = event_tx.send(BackendEvent::GuildChannelsLoaded { guild_id: guild_id.clone(), channels });
                            }
                            Err(e) => {
                                tracing::error!("failed to load channels for guild {guild_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load channels: {e}")));
                            }
                        }
                        match client.guild_roles(&guild_id).await {
                            Ok(roles) => {
                                let _ = event_tx.send(BackendEvent::GuildRolesLoaded { guild_id: guild_id.clone(), roles });
                            }
                            Err(e) => {
                                tracing::error!("failed to load roles for guild {guild_id}: {e}");
                            }
                        }
                        match client.my_guild_member(&guild_id).await {
                            Ok(member) => {
                                let _ = event_tx.send(BackendEvent::MyGuildMemberLoaded { guild_id: guild_id.clone(), roles: member.roles });
                            }
                            Err(e) => {
                                tracing::error!("failed to load own member for guild {guild_id}: {e}");
                            }
                        }
                        spawn_guild_emoji_loads(client, vec![guild_id], &event_tx);
                    }
                }
                UiCommand::LoadAllGuildEmojis { guild_ids } => {
                    if let Some(client) = &rest {
                        spawn_guild_emoji_loads(client, guild_ids, &event_tx);
                    }
                }
                UiCommand::SelectChannel { channel_id } => {
                    if let Some(client) = &rest {
                        match client.messages(&channel_id, 50).await {
                            Ok(messages) => {
                                let _ = event_tx.send(BackendEvent::MessagesLoaded { channel_id, messages });
                            }
                            Err(e) => {
                                tracing::error!("failed to load messages for channel {channel_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load messages: {e}")));
                            }
                        }
                    }
                }
                UiCommand::LoadOlderMessages { channel_id, before } => {
                    if let Some(client) = &rest {
                        match client.messages_before(&channel_id, &before, 50).await {
                            Ok(messages) => {
                                let _ = event_tx.send(BackendEvent::OlderMessagesLoaded { channel_id, messages });
                            }
                            Err(e) => {
                                tracing::error!("failed to load older messages for {channel_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load older messages: {e}")));
                            }
                        }
                    }
                }
                UiCommand::SendMessage { channel_id, content, reply_to } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.send_message_reply(&channel_id, &content, reply_to.as_deref()).await {
                            tracing::warn!("send failed: {e}");
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to send message: {e}")));
                        }
                    }
                }
                UiCommand::SendAttachment { channel_id, content, path, reply_to } => {
                    if let Some(client) = &rest {
                        let filename = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "file".into());
                        let content_type = mime_guess_from_path(&path);
                        match tokio::fs::read(&path).await {
                            Ok(bytes) => {
                                if let Err(e) = client
                                    .send_message_with_attachment_reply(
                                        &channel_id,
                                        &content,
                                        &filename,
                                        bytes,
                                        content_type,
                                        reply_to.as_deref(),
                                    )
                                    .await
                                {
                                    tracing::warn!("attachment send failed: {e}");
                                    let _ = event_tx.send(BackendEvent::Error(format!("failed to send attachment: {e}")));
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to read file: {e}")));
                            }
                        }
                    }
                }
                UiCommand::SendAttachmentBytes { channel_id, content, filename, bytes, content_type, reply_to } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client
                            .send_message_with_attachment_reply(
                                &channel_id,
                                &content,
                                &filename,
                                bytes,
                                &content_type,
                                reply_to.as_deref(),
                            )
                            .await
                        {
                            tracing::warn!("pasted attachment send failed: {e}");
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to send attachment: {e}")));
                        }
                    }
                }
                UiCommand::DownloadImage { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let image = match fetch_image_bytes(&url).await {
                            Some(bytes) => tokio::task::spawn_blocking(move || crate::images::decode(&bytes))
                                .await
                                .unwrap_or(crate::images::Decoded::Failed),
                            None => crate::images::Decoded::Failed,
                        };
                        let _ = event_tx.send(BackendEvent::ImageDownloaded { url, image });
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::LoadVideoPoster { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let decoded = match poster_source(&url).await {
                            Some(path) => tokio::task::spawn_blocking(move || match crate::video::first_frame(&path) {
                                Some(image) => crate::images::Decoded::Still(image),
                                None => crate::images::Decoded::Failed,
                            })
                            .await
                            .unwrap_or(crate::images::Decoded::Failed),
                            None => crate::images::Decoded::Failed,
                        };
                        let _ = event_tx.send(BackendEvent::ImageDownloaded { url: crate::images::poster_key(&url), image: decoded });
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::OpenVideo { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let batch = match start_video_download(&url).await {
                            Ok((path, handle)) => {
                                let open_url = url.clone();
                                tokio::task::spawn_blocking(move || open_video(&open_url, &path, &handle)).await.ok().flatten()
                            }
                            Err(e) => {
                                tracing::warn!("failed to fetch video: {e}");
                                forget_download(&url);
                                None
                            }
                        };
                        if let Some(batch) = batch {
                            let _ = event_tx.send(BackendEvent::VideoBatchReady(batch));
                        }
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::DecodeVideoBatch { url } => {
                    let event_tx = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(batch) = next_video_batch(&url) {
                            let _ = event_tx.send(BackendEvent::VideoBatchReady(batch));
                        }
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::CloseVideo { url } => {
                    close_video(&url);
                }
                #[cfg(feature = "video")]
                UiCommand::SeekVideo { url, target_secs } => {
                    let event_tx = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(batch) = seek_video(&url, target_secs) {
                            let _ = event_tx.send(BackendEvent::VideoBatchReady(batch));
                        }
                    });
                }
                UiCommand::SaveAttachment { url, filename } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let event = match save_attachment(&url, &filename).await {
                            Ok(path) => BackendEvent::Notice(format!("Saved to {}", path.display())),
                            Err(e) => BackendEvent::Error(format!("failed to save {filename}: {e}")),
                        };
                        let _ = event_tx.send(event);
                    });
                }
                UiCommand::StartTyping { channel_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.trigger_typing(&channel_id).await {
                            tracing::warn!("failed to send typing indicator: {e}");
                        }
                    }
                }
                UiCommand::LoginWithToken { token } => {
                    start_session(token, &mut rest, &event_tx).await;
                }
                UiCommand::OpenDmWithUser { user_id } => {
                    if let Some(client) = &rest {
                        match client.create_dm(&user_id).await {
                            Ok(channel) => {
                                let _ = event_tx.send(BackendEvent::DmOpened(channel));
                            }
                            Err(e) => {
                                tracing::error!("failed to open dm with {user_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to open DM: {e}")));
                            }
                        }
                    }
                }
                UiCommand::RefreshDmChannels => {
                    if let Some(client) = &rest {
                        if let Ok(dms) = client.dm_channels().await {
                            let _ = event_tx.send(BackendEvent::DmChannelsLoaded(dms));
                        }
                    }
                }
                UiCommand::EditMessage { channel_id, message_id, content } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.edit_message(&channel_id, &message_id, &content).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to edit message: {e}")));
                        }
                    }
                }
                UiCommand::DeleteMessage { channel_id, message_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.delete_message(&channel_id, &message_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to delete message: {e}")));
                        }
                    }
                }
                UiCommand::AckMessage { channel_id, message_id } => {
                    if let Some(client) = &rest {
                        let _ = client.ack_message(&channel_id, &message_id).await;
                    }
                }
                UiCommand::JoinGuildByInvite { invite_code } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.join_guild_by_invite(&invite_code).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to join server: {e}")));
                        }
                    }
                }
                UiCommand::CreateGuild { name } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.create_guild(&name).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to create server: {e}")));
                        }
                    }
                }
                UiCommand::RequestGuildMembers { guild_id } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::request_guild_members_frame(&guild_id);
                        let _ = tx.send(frame);
                    }
                }
                UiCommand::KickMember { guild_id, user_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.kick_member(&guild_id, &user_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to kick member: {e}")));
                        } else {
                            let _ = event_tx.send(BackendEvent::MemberRemoved { guild_id, user_id });
                        }
                    }
                }
                UiCommand::BanMember { guild_id, user_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.ban_member(&guild_id, &user_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to ban member: {e}")));
                        } else {
                            let _ = event_tx.send(BackendEvent::MemberRemoved { guild_id, user_id });
                        }
                    }
                }
                UiCommand::LoadProfile { user_id } => {
                    if let Some(client) = &rest {
                        match client.profile(&user_id).await {
                            Ok(v) => {
                                let profile = rustcord_client::models::UserProfile::from_value(&v);
                                let user = serde_json::from_value(v["user"].clone()).ok();
                                let _ = event_tx.send(BackendEvent::ProfileLoaded { user_id, profile, user });
                            }
                            Err(e) => tracing::warn!("failed to load profile {user_id}: {e}"),
                        }
                    }
                }
                UiCommand::SetPresence { status, custom_status } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.set_status(&status, custom_status.as_deref()).await {
                            tracing::warn!("failed to persist status: {e}");
                        }
                    }
                    presence::set_status(&status, custom_status.as_deref());
                    presence::publish(&event_tx);
                }
                UiCommand::SetManualActivity { name, details, state } => {
                    let field = |s: String| (!s.is_empty()).then_some(s);
                    let activity = field(name.trim().to_string()).map(|name| {
                        serde_json::json!({ "name": name, "type": 0, "details": field(details), "state": field(state) })
                    });
                    presence::set_manual(activity);
                    presence::publish(&event_tx);
                }
                UiCommand::SetRpcEnabled { conn_id, enabled } => {
                    presence::set_enabled(conn_id, enabled);
                    presence::publish(&event_tx);
                }
                UiCommand::JoinVoiceChannel { guild_id, channel_id, ring } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::voice_state_update_frame(guild_id.as_deref(), Some(&channel_id), false, false);
                        let _ = tx.send(frame);
                    }
                    if guild_id.is_none() && ring {
                        if let Some(client) = rest.clone() {
                            let event_tx = event_tx.clone();
                            let channel_id = channel_id.clone();
                            tokio::spawn(async move {
                                let mut last_err = None;
                                for _ in 0..4 {
                                    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                                    match client.ring_call(&channel_id).await {
                                        Ok(()) => return,
                                        Err(e) => last_err = Some(e),
                                    }
                                }
                                if let Some(e) = last_err {
                                    tracing::warn!("ring call failed: {e}");
                                    let _ = event_tx.send(BackendEvent::Error(format!("failed to ring: {e}")));
                                }
                            });
                        }
                    }
                    let _ = event_tx.send(BackendEvent::VoiceConnecting);
                }
                UiCommand::DeclineCall { channel_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.stop_ringing(&channel_id).await {
                            tracing::warn!("decline call failed: {e}");
                        }
                    }
                }
                UiCommand::LeaveVoiceChannel { guild_id } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::voice_state_update_frame(guild_id.as_deref(), None, false, false);
                        let _ = tx.send(frame);
                    }
                    *VOICE_MEDIA_STATE.lock().unwrap() = None;
                    let _ = event_tx.send(BackendEvent::VoiceDisconnected);
                }
                UiCommand::SetVoiceMuteDeaf { guild_id, channel_id, self_mute, self_deaf } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame =
                            rustcord_client::gateway::voice_state_update_frame(guild_id.as_deref(), Some(&channel_id), self_mute, self_deaf);
                        let _ = tx.send(frame);
                    }
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.muted.store(self_mute, std::sync::atomic::Ordering::Relaxed);
                        media_state.deafened.store(self_deaf, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                UiCommand::SetMicProcessing { gain, denoise } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.mic.set_gain(gain);
                        media_state.mic.denoise.store(denoise, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                UiCommand::SetUserVolume { user_id, volume } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.user_volumes.lock().unwrap().insert(user_id, volume);
                    }
                }
                UiCommand::SetVoiceInputDevice { device } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        *media_state.input_device.lock().unwrap() = device;
                    }
                }
                UiCommand::SetVoiceOutputDevice { device } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        *media_state.output_device.lock().unwrap() = device;
                    }
                }
                UiCommand::LoadVoiceDevices => {
                    let event_tx = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let inputs = rustcord_client::voice_audio::list_input_devices();
                        let outputs = rustcord_client::voice_audio::list_output_devices();
                        let _ = event_tx.send(BackendEvent::VoiceDevicesLoaded { inputs, outputs });
                    });
                }
                UiCommand::RunMicTest { input_device, output_device } => {
                    let event_tx = event_tx.clone();
                    let _ = event_tx.send(BackendEvent::MicTestStarted);
                    tokio::task::spawn_blocking(move || {
                        rustcord_client::voice_audio::run_mic_test(input_device, output_device, std::time::Duration::from_secs(10));
                        let _ = event_tx.send(BackendEvent::MicTestFinished);
                    });
                }
                UiCommand::Logout => {
                    session_store::clear_token();
                    rest = None;
                    login_session = None;
                }
            }
        }
    });
}

#[derive(Default)]
struct VoicePending {
    session_id: Option<String>,
    guild_id: Option<String>,
    channel_id: Option<String>,
    token: Option<String>,
    endpoint: Option<String>,
}

fn try_spawn_voice_gateway(
    pending_session: &mut Option<VoicePending>,
    my_user_id: &Option<String>,
    forward_tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    let Some(pending) = pending_session.as_ref() else { return };
    let (Some(session_id), Some(guild_id), Some(channel_id), Some(token), Some(endpoint), Some(user_id)) = (
        pending.session_id.clone(),
        pending.guild_id.clone(),
        pending.channel_id.clone(),
        pending.token.clone(),
        pending.endpoint.clone(),
        my_user_id.clone(),
    ) else {
        return;
    };
    *pending_session = None;

    let saved_settings = crate::settings::load();
    let media_state = rustcord_client::voice_gateway::VoiceMediaState {
        input_device: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.input_device)),
        output_device: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.output_device)),
        user_volumes: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.user_volumes)),
        ..Default::default()
    };
    media_state.mic.set_gain(saved_settings.mic_gain.unwrap_or(1.0));
    media_state.mic.denoise.store(saved_settings.denoise, std::sync::atomic::Ordering::Relaxed);
    *VOICE_MEDIA_STATE.lock().unwrap() = Some(media_state.clone());

    let voice_gateway =
        rustcord_client::voice_gateway::VoiceGateway::new(endpoint, guild_id, channel_id, user_id, session_id, token, media_state);
    let (vg_tx, mut vg_rx) = mpsc::unbounded_channel();
    let (_vg_out_tx, vg_out_rx) = mpsc::unbounded_channel();
    let forward_tx = forward_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = vg_rx.recv().await {
            use rustcord_client::voice_gateway::VoiceGatewayEvent;
            match ev {
                VoiceGatewayEvent::Connecting => {}
                VoiceGatewayEvent::Ready => {}
                VoiceGatewayEvent::MediaConnected => {
                    let _ = forward_tx.send(BackendEvent::VoiceConnected);
                }
                VoiceGatewayEvent::Disconnected { .. } => {
                    let _ = forward_tx.send(BackendEvent::VoiceDisconnected);
                }
                VoiceGatewayEvent::Speaking { user_id: Some(user_id), speaking, .. } => {
                    let _ = forward_tx.send(BackendEvent::VoiceSpeaking { user_id, speaking });
                }
                VoiceGatewayEvent::Stats(stats) => {
                    let _ = forward_tx.send(BackendEvent::VoiceStats(stats));
                }
                _ => {}
            }
        }
    });
    tokio::spawn(async move {
        if let Err(e) = voice_gateway.run(vg_tx, vg_out_rx).await {
            tracing::warn!("voice gateway task exited: {e}");
        }
    });
}

async fn load_guild_emojis(client: &RestClient, guild_id: String, event_tx: &mpsc::UnboundedSender<BackendEvent>) {
    match client.guild_emojis(&guild_id).await {
        Ok(emojis) => {
            let _ = event_tx.send(BackendEvent::GuildEmojisLoaded { guild_id, emojis });
        }
        Err(e) => tracing::warn!("failed to load emojis for guild {guild_id}: {e}"),
    }
}

fn spawn_guild_emoji_loads(client: &RestClient, guild_ids: Vec<String>, event_tx: &mpsc::UnboundedSender<BackendEvent>) {
    const MAX_CONCURRENT: usize = 3;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
    for guild_id in guild_ids {
        let client = client.clone();
        let event_tx = event_tx.clone();
        let semaphore = semaphore.clone();
        tokio::spawn(async move {
            let _permit = semaphore.acquire().await;
            load_guild_emojis(&client, guild_id, &event_tx).await;
        });
    }
}

fn attempt_login<'a>(
    email: String,
    password: String,
    captcha: Option<(String, CaptchaChallenge)>,
    session: &'a LoginSession,
    rest: &'a mut Option<RestClient>,
    event_tx: &'a mpsc::UnboundedSender<BackendEvent>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let captcha_ref = captcha.as_ref().map(|(token, challenge)| (token.as_str(), challenge));
        match session.login_with_captcha(&email, &password, captcha_ref).await {
            Ok(LoginResponse::Success { token }) => {
                start_session(token, rest, event_tx).await;
            }
            Ok(LoginResponse::MfaRequired { ticket, .. }) => {
                let _ = event_tx.send(BackendEvent::MfaRequired { ticket });
            }
            Err(ClientError::CaptchaRequired(challenge)) => {
                let _ = event_tx.send(BackendEvent::CaptchaInProgress);
                let solve_result = tokio::task::spawn_blocking({
                    let challenge = challenge.clone();
                    move || captcha::request_solve(challenge)
                })
                .await;

                match solve_result {
                    Ok(Ok(token)) => {
                        attempt_login(email, password, Some((token, challenge)), session, rest, event_tx).await;
                    }
                    Ok(Err(e)) => {
                        let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                    }
                    Err(e) => {
                        let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                    }
                }
            }
            Err(e) => {
                tracing::error!("login failed: {e}");
                let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
            }
        }
    })
}

async fn start_session(
    token: String,
    rest: &mut Option<RestClient>,
    event_tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    let client = match RestClient::new(&token) {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
            return;
        }
    };

    match client.current_user().await {
        Ok(me) => presence::set_me(serde_json::to_value(me).unwrap_or_default()),
        Err(e) => {
            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
            return;
        }
    }

    session_store::save_token(&token);
    *rest = Some(client.clone());
    let _ = event_tx.send(BackendEvent::LoggedIn);

    let (gw_tx, mut gw_rx) = mpsc::unbounded_channel();
    let gateway = Gateway::new(token.clone());
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    *GATEWAY_TX.lock().unwrap() = Some(out_tx);
    let forward_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut my_user_id: Option<String> = None;
        let mut pending_session: Option<VoicePending> = None;
        while let Some(ev) = gw_rx.recv().await {
            match &ev {
                GatewayEvent::Ready { user, .. } => {
                    my_user_id = Some(user.id.clone());
                }
                GatewayEvent::VoiceStateUpdate(vs) if my_user_id.as_deref() == Some(vs.user_id.as_str()) => {
                    if vs.channel_id.is_some() {
                        let pending = pending_session.get_or_insert_with(VoicePending::default);
                        pending.session_id = Some(vs.session_id.clone());
                        pending.channel_id = vs.channel_id.clone();
                        try_spawn_voice_gateway(&mut pending_session, &my_user_id, &forward_tx);
                    } else {
                        pending_session = None;
                    }
                }
                GatewayEvent::VoiceServerUpdate { token: voice_server_token, guild_id, endpoint } => {
                    let pending = pending_session.get_or_insert_with(VoicePending::default);
                    pending.token = Some(voice_server_token.clone());
                    pending.guild_id = Some(guild_id.clone());
                    pending.endpoint = endpoint.clone();
                    try_spawn_voice_gateway(&mut pending_session, &my_user_id, &forward_tx);
                }
                _ => {}
            }
            let _ = forward_tx.send(BackendEvent::Gateway(ev));
        }
    });
    tokio::spawn(async move {
        if let Err(e) = gateway.run(gw_tx, out_rx).await {
            tracing::warn!("gateway task exited: {e}");
        }
    });

    if let Ok(dms) = client.dm_channels().await {
        let _ = event_tx.send(BackendEvent::DmChannelsLoaded(dms));
    }

    if let Ok(relationships) = client.relationships().await {
        let _ = event_tx.send(BackendEvent::RelationshipsLoaded(relationships));
    }
}

#[cfg(feature = "video")]
struct ParkedDecoder(crate::video::VideoDecoder, u64);

// SAFETY: the decoder's ffmpeg contexts hold raw pointers that are not Send,
// but they are only unsafe to use from two threads at once, not to move. Every
// access removes the decoder from the map, decodes on one blocking thread, and
// puts it back, so no two threads ever hold the same decoder.
#[cfg(feature = "video")]
unsafe impl Send for ParkedDecoder {}

#[cfg(feature = "video")]
static OPEN_VIDEOS: std::sync::Mutex<Option<std::collections::HashMap<String, ParkedDecoder>>> = std::sync::Mutex::new(None);

#[cfg(feature = "video")]
const MAX_VIDEO_BYTES: u64 = 256 * 1024 * 1024;

#[cfg(feature = "video")]
const PLAYABLE_HEAD_BYTES: u64 = 512 * 1024;

#[cfg(feature = "video")]
#[derive(Clone)]
struct DownloadHandle {
    state: std::sync::Arc<DownloadState>,
}

#[cfg(feature = "video")]
#[derive(Default)]
struct DownloadState {
    written: std::sync::atomic::AtomicU64,
    complete: std::sync::atomic::AtomicBool,
    failed: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "video")]
impl DownloadHandle {
    fn source_state(&self) -> crate::video::SourceState {
        if self.state.complete.load(std::sync::atomic::Ordering::Relaxed) {
            crate::video::SourceState::Complete
        } else {
            crate::video::SourceState::Growing
        }
    }

    fn failed(&self) -> bool {
        self.state.failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn written(&self) -> u64 {
        self.state.written.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(feature = "video")]
static DOWNLOADS: std::sync::Mutex<Option<std::collections::HashMap<String, DownloadHandle>>> = std::sync::Mutex::new(None);

#[cfg(feature = "video")]
async fn start_video_download(url: &str) -> Result<(std::path::PathBuf, DownloadHandle), String> {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncWriteExt;

    let final_path = cache_file(url).ok_or("no cache directory")?.with_extension("video");
    if final_path.exists() {
        let state = std::sync::Arc::new(DownloadState::default());
        state.complete.store(true, Ordering::Relaxed);
        let handle = DownloadHandle { state };
        DOWNLOADS.lock().unwrap().get_or_insert_with(Default::default).insert(url.to_string(), handle.clone());
        return Ok((final_path, handle));
    }
    if let Some(dir) = final_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let client = HTTP.get_or_init(reqwest::Client::new);
    let mut resp = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    if resp.content_length().is_some_and(|len| len > MAX_VIDEO_BYTES) {
        return Err(format!("video larger than {MAX_VIDEO_BYTES} bytes"));
    }

    let partial = final_path.with_extension("video.part");
    let mut file = tokio::fs::File::create(&partial).await.map_err(|e| e.to_string())?;
    let state = std::sync::Arc::new(DownloadState::default());
    let handle = DownloadHandle { state: state.clone() };
    DOWNLOADS.lock().unwrap().get_or_insert_with(Default::default).insert(url.to_string(), handle.clone());

    let mut written = 0u64;
    while written < PLAYABLE_HEAD_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                written += chunk.len() as u64;
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                state.written.store(written, Ordering::Relaxed);
            }
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;

    let rest_url = url.to_string();
    let rest_partial = partial.clone();
    let rest_final = final_path.clone();
    tokio::spawn(async move {
        let result = async {
            while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
                written += chunk.len() as u64;
                if written > MAX_VIDEO_BYTES {
                    return Err(format!("video larger than {MAX_VIDEO_BYTES} bytes"));
                }
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                file.flush().await.map_err(|e| e.to_string())?;
                state.written.store(written, Ordering::Relaxed);
            }
            file.flush().await.map_err(|e| e.to_string())?;
            drop(file);
            tokio::fs::rename(&rest_partial, &rest_final).await.map_err(|e| e.to_string())
        }
        .await;
        match result {
            Ok(()) => state.complete.store(true, Ordering::Relaxed),
            Err(e) => {
                tracing::warn!("video download failed for {rest_url}: {e}");
                state.failed.store(true, Ordering::Relaxed);
                state.complete.store(true, Ordering::Relaxed);
                let _ = tokio::fs::remove_file(&rest_partial).await;
            }
        }
    });

    Ok((partial, handle))
}

#[cfg(feature = "video")]
fn wait_for_complete_download(
    url: &str,
    partial: &std::path::Path,
    handle: &DownloadHandle,
) -> Option<crate::images::VideoBatch> {
    while handle.source_state() == crate::video::SourceState::Growing {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if handle.failed() {
        forget_download(url);
        return None;
    }
    let final_path = partial.with_extension("");
    let path = if final_path.exists() { final_path } else { partial.to_path_buf() };
    let mut decoder = match crate::video::VideoDecoder::open(&path) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("failed to open video {url} after download: {e}");
            forget_download(url);
            return None;
        }
    };
    let batch = decoder.next_batch(crate::video::BATCH_FRAMES, crate::video::SourceState::Complete);
    let meta = crate::images::VideoMeta {
        duration_secs: decoder.duration_secs,
        has_audio: decoder.has_audio(),
        width: decoder.width,
        height: decoder.height,
    };
    if !batch.finished {
        let mut guard = OPEN_VIDEOS.lock().unwrap();
        guard.get_or_insert_with(Default::default).insert(url.to_string(), ParkedDecoder(decoder, 0));
    }
    Some(crate::images::VideoBatch { url: url.to_string(), batch, meta: Some(meta), seek_generation: 0 })
}

#[cfg(feature = "video")]
async fn poster_source(url: &str) -> Option<std::path::PathBuf> {
    use tokio::io::AsyncWriteExt;
    let final_path = cache_file(url)?.with_extension("video");
    if final_path.exists() {
        return Some(final_path);
    }
    let poster_path = final_path.with_extension("poster");
    if poster_path.exists() {
        return Some(poster_path);
    }
    if let Some(dir) = poster_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let client = HTTP.get_or_init(reqwest::Client::new);
    let mut resp = client.get(url).send().await.and_then(|r| r.error_for_status()).ok()?;
    let partial = poster_path.with_extension("poster.part");
    let mut file = tokio::fs::File::create(&partial).await.ok()?;
    let mut written = 0u64;
    while written < PLAYABLE_HEAD_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                written += chunk.len() as u64;
                file.write_all(&chunk).await.ok()?;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    file.flush().await.ok()?;
    drop(file);
    tokio::fs::rename(&partial, &poster_path).await.ok()?;
    Some(poster_path)
}

#[cfg(feature = "video")]
fn download_handle(url: &str) -> Option<DownloadHandle> {
    DOWNLOADS.lock().unwrap().as_ref()?.get(url).cloned()
}

#[cfg(feature = "video")]
fn forget_download(url: &str) {
    if let Some(map) = DOWNLOADS.lock().unwrap().as_mut() {
        map.remove(url);
    }
}

#[cfg(feature = "video")]
fn open_video(url: &str, path: &std::path::Path, handle: &DownloadHandle) -> Option<crate::images::VideoBatch> {
    let mut decoder = match crate::video::VideoDecoder::open(path) {
        Ok(d) => d,
        Err(e) => {
            if handle.source_state() == crate::video::SourceState::Growing {
                tracing::debug!("video {url} not yet playable ({e}); waiting for the full download");
                return wait_for_complete_download(url, path, handle);
            }
            tracing::warn!("failed to open video {url}: {e}");
            forget_download(url);
            return None;
        }
    };
    let batch = decoder.next_batch(crate::video::BATCH_FRAMES, handle.source_state());
    let meta = crate::images::VideoMeta {
        duration_secs: decoder.duration_secs,
        has_audio: decoder.has_audio(),
        width: decoder.width,
        height: decoder.height,
    };
    if !batch.finished {
        let mut guard = OPEN_VIDEOS.lock().unwrap();
        guard.get_or_insert_with(Default::default).insert(url.to_string(), ParkedDecoder(decoder, 0));
    }
    Some(crate::images::VideoBatch { url: url.to_string(), batch, meta: Some(meta), seek_generation: 0 })
}

#[cfg(feature = "video")]
fn next_video_batch(url: &str) -> Option<crate::images::VideoBatch> {
    let handle = download_handle(url);
    let source = handle.as_ref().map(|h| h.source_state()).unwrap_or(crate::video::SourceState::Complete);
    let mut parked = OPEN_VIDEOS.lock().unwrap().as_mut()?.remove(url)?;
    let mut batch = parked.0.next_batch(crate::video::BATCH_FRAMES, source);
    if batch.awaiting_data && handle.as_ref().is_some_and(|h| h.failed()) {
        tracing::warn!("video download for {url} failed; ending playback at {} bytes", handle.map(|h| h.written()).unwrap_or(0));
        batch.awaiting_data = false;
        batch.finished = true;
    }
    let generation = parked.1;
    if !batch.finished {
        let mut guard = OPEN_VIDEOS.lock().unwrap();
        guard.get_or_insert_with(Default::default).insert(url.to_string(), parked);
    }
    Some(crate::images::VideoBatch { url: url.to_string(), batch, meta: None, seek_generation: generation })
}

#[cfg(feature = "video")]
fn seek_video(url: &str, target_secs: f64) -> Option<crate::images::VideoBatch> {
    let mut parked = OPEN_VIDEOS.lock().unwrap().as_mut()?.remove(url)?;
    if let Err(e) = parked.0.seek(target_secs) {
        tracing::warn!("seek failed for {url}: {e}");
    }
    parked.1 += 1;
    let batch = parked.0.next_batch(crate::video::BATCH_FRAMES, crate::video::SourceState::Complete);
    let generation = parked.1;
    if !batch.finished {
        let mut guard = OPEN_VIDEOS.lock().unwrap();
        guard.get_or_insert_with(Default::default).insert(url.to_string(), parked);
    }
    Some(crate::images::VideoBatch { url: url.to_string(), batch, meta: None, seek_generation: generation })
}

#[cfg(feature = "video")]
fn close_video(url: &str) {
    if let Some(decoders) = OPEN_VIDEOS.lock().unwrap().as_mut() {
        decoders.remove(url);
    }
    forget_download(url);
}
