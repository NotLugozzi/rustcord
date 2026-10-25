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

pub(crate) static GATEWAY_TX: std::sync::Mutex<Option<mpsc::UnboundedSender<String>>> = std::sync::Mutex::new(None);

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
    OpenDmWithUser { user_id: String },
    RefreshDmChannels,
    StartTyping { channel_id: String },
    LoadProfile { user_id: String },
    EditMessage { channel_id: String, message_id: String, content: String },
    DeleteMessage { channel_id: String, message_id: String },
    SetPresence { status: String, custom_status: Option<String> },
    SetManualActivity { name: String, details: String, state: String },
    SetRpcEnabled { conn_id: u64, enabled: bool },
    JoinVoiceChannel { guild_id: String, channel_id: String },
    LeaveVoiceChannel { guild_id: String },
    Logout,
}

pub enum BackendEvent {
    MfaRequired { ticket: String },
    CaptchaInProgress,
    LoginFailed(String),
    LoggedIn,
    Error(String),
    RpcApps(Vec<crate::presence::RpcAppInfo>),
    ProfileLoaded { user_id: String, profile: rustcord_client::models::UserProfile },
    Gateway(GatewayEvent),
    GuildChannelsLoaded { guild_id: String, channels: Vec<rustcord_client::models::Channel> },
    GuildEmojisLoaded { guild_id: String, emojis: Vec<rustcord_client::models::Emoji> },
    DmChannelsLoaded(Vec<rustcord_client::models::Channel>),
    MessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    OlderMessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    RelationshipsLoaded(Vec<rustcord_client::models::Relationship>),
    DmOpened(rustcord_client::models::Channel),
    ImageDownloaded { url: String, image: crate::images::Decoded },
    VoiceConnecting,
    VoiceConnected,
    VoiceDisconnected,
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
                UiCommand::LoadProfile { user_id } => {
                    if let Some(client) = &rest {
                        match client.profile(&user_id).await {
                            Ok(v) => {
                                let profile = rustcord_client::models::UserProfile::from_value(&v);
                                let _ = event_tx.send(BackendEvent::ProfileLoaded { user_id, profile });
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
                UiCommand::JoinVoiceChannel { guild_id, channel_id } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::voice_state_update_frame(&guild_id, Some(&channel_id), false, false);
                        let _ = tx.send(frame);
                    }
                    let _ = event_tx.send(BackendEvent::VoiceConnecting);
                }
                UiCommand::LeaveVoiceChannel { guild_id } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::voice_state_update_frame(&guild_id, None, false, false);
                        let _ = tx.send(frame);
                    }
                    let _ = event_tx.send(BackendEvent::VoiceDisconnected);
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

    let voice_gateway = rustcord_client::voice_gateway::VoiceGateway::new(endpoint, guild_id, channel_id, user_id, session_id, token);
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
    const MAX_ATTEMPTS: u32 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        match client.guild_emojis(&guild_id).await {
            Ok(emojis) => {
                let _ = event_tx.send(BackendEvent::GuildEmojisLoaded { guild_id, emojis });
                return;
            }
            Err(ClientError::Api { status: 429, .. }) if attempt < MAX_ATTEMPTS => {
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
            Err(e) => {
                tracing::warn!("failed to load emojis for guild {guild_id}: {e}");
                return;
            }
        }
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
                        pending.guild_id = vs.guild_id.clone();
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
