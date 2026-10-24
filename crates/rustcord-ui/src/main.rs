mod app_state;
mod backend;
mod clipboard;
mod emoji_sprites;
mod images;
mod ipc;
mod markdown;
mod presence;
mod renderer;
mod session_store;
mod theme;
mod ui;

use app_state::AppState;
use backend::{BackendEvent, UiCommand};
use rustcord_client::gateway::GatewayEvent;
use tokio::sync::mpsc;

fn main() {
    tracing_subscriber::fmt::init();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<UiCommand>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<BackendEvent>();
    backend::spawn(&rt, cmd_rx, event_tx);

    let mut state = AppState { sidebar_width: 240.0, my_status: rustcord_client::models::PresenceStatus::Online, notifications_enabled: true, window_focused: true, groups: app_state::load_groups(), notes: app_state::load_notes(), ..Default::default() };
    theme::Theme::install_bundled();
    if let Some(dir) = theme::Theme::themes_dir() {
        let default_path = dir.join("discord-dark.toml");
        if let Ok(t) = theme::Theme::load_from_file(&default_path) {
            state.theme = t;
        }
    }

    if let Some(token) = session_store::load_token() {
        state.login_status = Some("Resuming saved session...".into());
        let _ = cmd_tx.send(UiCommand::LoginWithToken { token });
    }

    renderer::run(state, cmd_tx, event_rx);
}

fn notify_if_relevant(state: &AppState, message: &rustcord_client::models::Message) {
    let Some(me) = &state.current_user else { return };
    if !state.notifications_enabled || message.author.id == me.id {
        return;
    }
    let relevant = message.guild_id.is_none() || message.mentions.iter().any(|u| u.id == me.id);
    let watching = state.window_focused && state.selected_channel.as_deref() == Some(message.channel_id.as_str());
    if !relevant || watching {
        return;
    }
    let title = message.author.global_name.clone().unwrap_or_else(|| message.author.username.clone());
    let body = if message.content.is_empty() { "[attachment]".to_string() } else { message.content.chars().take(160).collect() };
    std::thread::spawn(move || {
        if let Err(e) = notify_rust::Notification::new().summary(&title).body(&body).show() {
            tracing::warn!("notification failed: {e}");
        }
    });
}

pub fn drain_backend_events(
    state: &mut AppState,
    event_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    cmd_tx: &mpsc::UnboundedSender<UiCommand>,
) -> Vec<(String, images::Decoded)> {
    let mut image_downloads = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        match event {
            BackendEvent::LoggedIn => {
                state.logged_in = true;
                state.login_error = None;
                state.login_status = None;
            }
            BackendEvent::LoginFailed(err) => {
                state.login_error = Some(err);
                state.login_status = None;
            }
            BackendEvent::MfaRequired { ticket } => {
                state.mfa_ticket = Some(ticket);
                state.login_status = None;
            }
            BackendEvent::CaptchaInProgress => {
                state.login_status = Some("Solve the captcha in the popup window...".into());
            }
            BackendEvent::ProfileLoaded { user_id, profile } => {
                state.profiles.insert(user_id, profile);
            }
            BackendEvent::RpcApps(apps) => {
                state.rpc_apps = apps;
            }
            BackendEvent::Error(err) => {
                state.status_message = Some(err);
            }
            BackendEvent::Gateway(GatewayEvent::Connecting) => {
                state.gateway_connected = false;
            }
            BackendEvent::Gateway(GatewayEvent::Connected) => {}
            BackendEvent::Gateway(GatewayEvent::Resumed) => {
                state.gateway_connected = true;
            }
            BackendEvent::Gateway(GatewayEvent::Disconnected { .. }) => {
                state.gateway_connected = false;
            }
            BackendEvent::Gateway(GatewayEvent::Ready { user, guilds, friend_presences }) => {
                state.gateway_connected = true;
                state.current_user = Some(user);
                let guild_ids = guilds.iter().map(|g| g.id.clone()).collect();
                state.guilds = guilds;
                for presence in friend_presences {
                    state.presences.insert(presence.user.id.clone(), presence);
                }
                let _ = cmd_tx.send(UiCommand::LoadAllGuildEmojis { guild_ids });
            }
            BackendEvent::Gateway(GatewayEvent::Presences(presences)) => {
                for presence in presences {
                    state.presences.insert(presence.user.id.clone(), presence);
                }
            }
            BackendEvent::Gateway(GatewayEvent::MessageCreate(message)) => {
                if let Some(typing) = state.typing_users.get_mut(&message.channel_id) {
                    typing.remove(&message.author.id);
                }
                notify_if_relevant(state, &message);
                if message.guild_id.is_none() && !state.dm_channels.iter().any(|c| c.id == message.channel_id) {
                    let _ = cmd_tx.send(UiCommand::RefreshDmChannels);
                }
                state.push_message(message);
            }
            BackendEvent::Gateway(GatewayEvent::TypingStart { channel_id, user_id }) => {
                state.note_typing(channel_id, user_id);
            }
            BackendEvent::Gateway(GatewayEvent::PresenceUpdate(presence)) => {
                state.presences.insert(presence.user.id.clone(), presence);
            }
            BackendEvent::Gateway(GatewayEvent::MessageUpdate { channel_id, id, content, edited_timestamp }) => {
                if let Some(msg) = state.messages.get_mut(&channel_id).and_then(|l| l.iter_mut().find(|m| m.id == id)) {
                    if let Some(content) = content {
                        msg.content = content;
                    }
                    if edited_timestamp.is_some() {
                        msg.edited_timestamp = edited_timestamp;
                    }
                }
            }
            BackendEvent::Gateway(GatewayEvent::MessageDelete { channel_id, id }) => {
                if let Some(list) = state.messages.get_mut(&channel_id) {
                    list.retain(|m| m.id != id);
                }
            }
            BackendEvent::Gateway(GatewayEvent::Unknown { .. }) => {}
            BackendEvent::GuildChannelsLoaded { guild_id, channels } => {
                state.guild_channels.insert(guild_id, channels);
            }
            BackendEvent::GuildEmojisLoaded { guild_id, emojis } => {
                state.guild_emojis.insert(guild_id, emojis);
            }
            BackendEvent::DmChannelsLoaded(channels) => {
                state.dm_channels = channels;
            }
            BackendEvent::MessagesLoaded { channel_id, messages } => {
                state.set_message_history(channel_id, messages);
            }
            BackendEvent::OlderMessagesLoaded { channel_id, messages } => {
                state.loading_older_messages = false;
                state.prepend_message_history(channel_id, messages);
            }
            BackendEvent::RelationshipsLoaded(relationships) => {
                state.relationships = relationships;
            }
            BackendEvent::DmOpened(channel) => {
                let channel_id = channel.id.clone();
                if !state.dm_channels.iter().any(|c| c.id == channel.id) {
                    state.dm_channels.insert(0, channel);
                }
                state.selected_guild = None;
                state.selected_channel = Some(channel_id);
                state.sidebar_tab = 0;
            }
            BackendEvent::ImageDownloaded { url, image } => {
                image_downloads.push((url, image));
            }
        }
    }
    image_downloads
}
