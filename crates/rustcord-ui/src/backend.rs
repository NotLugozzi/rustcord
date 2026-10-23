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

pub enum UiCommand {
    LoginWithPassword { email: String, password: String },
    LoginWithToken { token: String },
    SubmitMfa { ticket: String, code: String },
    SelectGuild { guild_id: String },
    SelectChannel { channel_id: String },
    LoadOlderMessages { channel_id: String, before: String },
    SendMessage { channel_id: String, content: String },
    SendAttachment { channel_id: String, content: String, path: std::path::PathBuf },
    DownloadImage { url: String },
    OpenDmWithUser { user_id: String },
    StartTyping { channel_id: String },
    Logout,
}

pub enum BackendEvent {
    MfaRequired { ticket: String },
    CaptchaInProgress,
    LoginFailed(String),
    LoggedIn,
    Error(String),
    Gateway(GatewayEvent),
    GuildChannelsLoaded { guild_id: String, channels: Vec<rustcord_client::models::Channel> },
    DmChannelsLoaded(Vec<rustcord_client::models::Channel>),
    MessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    OlderMessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    RelationshipsLoaded(Vec<rustcord_client::models::Relationship>),
    DmOpened(rustcord_client::models::Channel),
    ImageDownloaded { url: String, bytes: Vec<u8> },
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
) {
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
                                let _ = event_tx.send(BackendEvent::GuildChannelsLoaded { guild_id, channels });
                            }
                            Err(e) => {
                                tracing::error!("failed to load channels for guild {guild_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load channels: {e}")));
                            }
                        }
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
                UiCommand::SendMessage { channel_id, content } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.send_message(&channel_id, &content).await {
                            tracing::warn!("send failed: {e}");
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to send message: {e}")));
                        }
                    }
                }
                UiCommand::SendAttachment { channel_id, content, path } => {
                    if let Some(client) = &rest {
                        let filename = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "file".into());
                        let content_type = mime_guess_from_path(&path);
                        match tokio::fs::read(&path).await {
                            Ok(bytes) => {
                                if let Err(e) = client
                                    .send_message_with_attachment(&channel_id, &content, &filename, bytes, content_type)
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
                UiCommand::DownloadImage { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        match reqwest::get(&url).await.and_then(|r| r.error_for_status()) {
                            Ok(resp) => match resp.bytes().await {
                                Ok(bytes) => {
                                    let _ = event_tx.send(BackendEvent::ImageDownloaded { url, bytes: bytes.to_vec() });
                                }
                                Err(e) => tracing::warn!("failed to read image bytes for {url}: {e}"),
                            },
                            Err(e) => tracing::warn!("failed to download image {url}: {e}"),
                        }
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
                UiCommand::Logout => {
                    session_store::clear_token();
                    rest = None;
                    login_session = None;
                }
            }
        }
    });
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
                    move || captcha::solve_blocking(&challenge)
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

    if let Err(e) = client.current_user().await {
        let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
        return;
    }

    session_store::save_token(&token);
    *rest = Some(client.clone());
    let _ = event_tx.send(BackendEvent::LoggedIn);

    let (gw_tx, mut gw_rx) = mpsc::unbounded_channel();
    let gateway = Gateway::new(token.clone());
    let forward_tx = event_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = gw_rx.recv().await {
            let _ = forward_tx.send(BackendEvent::Gateway(ev));
        }
    });
    tokio::spawn(async move {
        if let Err(e) = gateway.run(gw_tx).await {
            tracing::warn!("gateway closed: {e}");
        }
    });

    if let Ok(dms) = client.dm_channels().await {
        let _ = event_tx.send(BackendEvent::DmChannelsLoaded(dms));
    }

    if let Ok(relationships) = client.relationships().await {
        let _ = event_tx.send(BackendEvent::RelationshipsLoaded(relationships));
    }
}
