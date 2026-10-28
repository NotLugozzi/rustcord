mod app_state;
mod backend;
mod clipboard;
mod emoji;
mod images;
mod ipc;
mod markdown;
mod presence;
mod renderer;
mod session_store;
mod settings;
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

    let saved_settings = settings::load();
    let (rail_folders, rail_order) = app_state::load_rail();
    let mut state = AppState {
        sidebar_width: 240.0,
        my_status: rustcord_client::models::PresenceStatus::Online,
        notifications_enabled: true,
        window_focused: true,
        groups: app_state::load_groups(),
        notes: app_state::load_notes(),
        mic_gain: saved_settings.mic_gain.unwrap_or(1.0),
        denoise: saved_settings.denoise,
        user_volumes: saved_settings.user_volumes,
        selected_input_device: saved_settings.input_device,
        selected_output_device: saved_settings.output_device,
        rail_folders,
        rail_order,
        last_read_message_id: app_state::load_read_state(),
        ..Default::default()
    };
    theme::Theme::install_bundled();
    if let Some(dir) = theme::Theme::themes_dir() {
        let selected_file = saved_settings.theme_file.unwrap_or_else(|| "discord-dark.toml".into());
        let path = dir.join(&selected_file);
        if let Ok(t) = theme::Theme::load_from_file(&path) {
            state.theme = t;
        } else {
            tracing::warn!("saved theme {} could not be loaded; using the default theme", path.display());
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
            BackendEvent::ProfileLoaded { user_id, profile, user } => {
                if let Some(user) = user {
                    state.known_users.insert(user_id.clone(), user);
                }
                state.profiles.insert(user_id, profile);
            }
            BackendEvent::RpcApps(apps) => {
                state.rpc_apps = apps;
            }
            BackendEvent::Error(err) | BackendEvent::Notice(err) => {
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
                state.reconcile_rail_order();
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
            BackendEvent::Gateway(GatewayEvent::InitialVoiceStates(voice_states)) => {
                for voice_state in voice_states {
                    state.apply_voice_state(voice_state);
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
                let is_actively_viewing = state.window_focused && state.selected_channel.as_deref() == Some(message.channel_id.as_str());
                if is_actively_viewing {
                    state.mark_channel_read(&message.channel_id, &message.id);
                }
                state.push_message(message);
            }
            BackendEvent::Gateway(GatewayEvent::TypingStart { channel_id, user_id }) => {
                state.note_typing(channel_id, user_id);
            }
            BackendEvent::Gateway(GatewayEvent::PresenceUpdate(presence)) => {
                state.presences.insert(presence.user.id.clone(), presence);
            }
            BackendEvent::Gateway(GatewayEvent::MessageUpdate { channel_id, id, content, edited_timestamp, call }) => {
                if let Some(msg) = state.messages.get_mut(&channel_id).and_then(|l| l.iter_mut().find(|m| m.id == id)) {
                    if let Some(content) = content {
                        msg.content = content;
                    }
                    if edited_timestamp.is_some() {
                        msg.edited_timestamp = edited_timestamp;
                    }
                    if call.is_some() {
                        msg.call = call;
                    }
                }
            }
            BackendEvent::Gateway(GatewayEvent::MessageDelete { channel_id, id }) => {
                if let Some(list) = state.messages.get_mut(&channel_id) {
                    list.retain(|m| m.id != id);
                }
            }
            BackendEvent::Gateway(GatewayEvent::VoiceStateUpdate(voice_state)) => {
                state.ding_for_voice_state(&voice_state);
                if state.current_user.as_ref().is_some_and(|u| u.id == voice_state.user_id) {
                    let new_channel = voice_state.channel_id.clone().map(|cid| (voice_state.guild_id.clone(), cid));
                    if new_channel != state.connected_voice_channel {
                        state.voice_joined_at = new_channel.is_some().then(std::time::Instant::now);
                    }
                    state.connected_voice_channel = new_channel;
                    if let Some(cid) = &voice_state.channel_id {
                        state.incoming_calls.retain(|c| c != cid);
                        state.sync_ringtone();
                    }
                }
                state.apply_voice_state(voice_state);
            }
            BackendEvent::Gateway(GatewayEvent::VoiceServerUpdate { .. }) => {}
            BackendEvent::Gateway(GatewayEvent::Unknown { op_name, raw }) => {
                let channel_id = raw["channel_id"].as_str().unwrap_or_default().to_string();
                let me = state.current_user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                let ringing_me = raw["ringing"].as_array().is_some_and(|r| r.iter().any(|id| id.as_str() == Some(me.as_str())));
                match op_name.as_str() {
                    "CALL_CREATE" | "CALL_UPDATE" if ringing_me => {
                        if !state.incoming_calls.contains(&channel_id) {
                            if state.notifications_enabled {
                                std::thread::spawn(|| {
                                    let _ = notify_rust::Notification::new().summary("Incoming call").show();
                                });
                            }
                            state.incoming_calls.push(channel_id);
                        }
                    }
                    "CALL_CREATE" | "CALL_UPDATE" | "CALL_DELETE" => state.incoming_calls.retain(|c| *c != channel_id),
                    _ => {}
                }
                state.sync_ringtone();
            }
            BackendEvent::VoiceConnecting => {
                state.voice_connecting = true;
            }
            BackendEvent::VoiceConnected => {
                state.voice_connecting = false;
            }
            BackendEvent::VoiceDisconnected => {
                state.voice_connecting = false;
                state.connected_voice_channel = None;
                state.pending_voice_channel = None;
                state.voice_speaking_users.clear();
                state.voice_stats = Default::default();
                state.voice_history.clear();
                state.show_voice_info = false;
                state.voice_joined_at = None;
            }
            BackendEvent::VoiceSpeaking { user_id, speaking } => {
                state.set_voice_speaking(user_id, speaking);
            }
            BackendEvent::VoiceStats(stats) => {
                if state.voice_history.len() >= 120 {
                    state.voice_history.pop_front();
                }
                state.voice_history.push_back(stats.clone());
                state.voice_stats = stats;
            }
            BackendEvent::VoiceDevicesLoaded { inputs, outputs } => {
                state.voice_input_devices = inputs;
                state.voice_output_devices = outputs;
            }
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
                if let Some(newest) = messages.iter().max_by_key(|m| m.id.parse::<u64>().unwrap_or(0)) {
                    state.mark_channel_read(&channel_id, &newest.id);
                }
                state.set_message_history(channel_id, messages);
            }
            BackendEvent::OlderMessagesLoaded { channel_id, messages } => {
                state.loading_older_messages = false;
                if messages.is_empty() {
                    state.history_exhausted.insert(channel_id.clone());
                }
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
