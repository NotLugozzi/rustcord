mod app_state;
mod build_info;
mod backend;
mod clipboard;
mod devtools;
mod emoji;
mod fonts;
mod icons;
mod images;
mod invites;
mod ipc;
mod keybinds;
mod markdown;
mod memory;
mod message_rows;
mod presence;
mod renderer;
mod session_store;
mod settings;
mod switcher;
mod theme;
mod timefmt;
mod ui;
#[cfg(feature = "video")]
mod video;
#[cfg(feature = "video")]
mod video_audio;

use app_state::AppState;
use backend::{BackendEvent, UiCommand};
use rustcord_client::gateway::GatewayEvent;
use tokio::sync::mpsc;

#[cfg(target_os = "macos")]
static MAC_NOTIFICATION_APPLICATION: std::sync::OnceLock<()> = std::sync::OnceLock::new();

pub(crate) fn show_notification(notification: &notify_rust::Notification) {
    #[cfg(target_os = "macos")]
    MAC_NOTIFICATION_APPLICATION.get_or_init(|| {
        if let Err(error) = notify_rust::set_application("com.apple.Terminal") {
            tracing::warn!("could not set macOS notification application: {error}");
        }
    });
    if let Err(error) = notification.show() {
        tracing::warn!("notification failed: {error}");
    }
}

fn main() {
    memory::configure();
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        use tracing_subscriber::{EnvFilter, Layer};
        let console = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let captured = EnvFilter::new("info,rustcord=debug,rustcord_client=debug");
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_filter(console))
            .with(devtools::LogCapture.with_filter(captured))
            .init();
    }

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
        frequently_used: app_state::FrequentlyUsed::load(),
        mic_gain: saved_settings.mic_gain.unwrap_or(1.0),
        denoise: saved_settings.denoise,
        share_system_audio: true,
        screen_share_max_height: 1080,
        screen_share_fps: 30,
        user_volumes: saved_settings.user_volumes,
        selected_input_device: saved_settings.input_device,
        selected_output_device: saved_settings.output_device,
        camera_device: saved_settings.camera_device,
        keybinds: keybinds::Keybinds::from_map(&saved_settings.keybinds),
        rail_folders,
        rail_order,
        last_read_message_id: app_state::load_read_state(),
        clock_24h: saved_settings.clock_24h,
        self_muted: saved_settings.self_mute || saved_settings.self_deaf,
        self_deafened: saved_settings.self_deaf,
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

    match session_store::load_token() {
        Ok(Some(token)) => {
            state.login_status = Some("Resuming saved session...".into());
            let _ = cmd_tx.send(UiCommand::LoginWithToken { token });
        }
        Ok(None) => {}
        Err(_) => tracing::warn!("failed to read saved credentials"),
    }

    renderer::run(state, cmd_tx, event_rx);
}

fn notify_if_relevant(state: &AppState, message: &rustcord_client::models::Message, cmd_tx: &mpsc::UnboundedSender<UiCommand>) {
    if !state.notifications_enabled || !state.should_notify(message, chrono::Utc::now()) {
        return;
    }
    let watching = state.window_focused && state.selected_channel.as_deref() == Some(message.channel_id.as_str());
    if watching {
        return;
    }
    let title = message.author.global_name.clone().unwrap_or_else(|| message.author.username.clone());
    let poll_question = message.poll.as_ref().and_then(|p| p.question.text.as_deref());
    let body = match (message.content.is_empty(), message.sticker_items.first(), poll_question) {
        (false, _, _) => message.content.chars().take(160).collect(),
        (true, Some(sticker), _) => format!("[sticker: {}]", sticker.name),
        (true, None, Some(question)) => format!("[poll: {question}]"),
        (true, None, None) => "[attachment]".to_string(),
    };
    let avatar_url = rustcord_client::cdn::avatar_url(&message.author, 64)
        .unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&message.author));
    let _ = cmd_tx.send(UiCommand::ShowNotification { title, body, avatar_url });
}

pub fn drain_backend_events(
    state: &mut AppState,
    event_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    cmd_tx: &mpsc::UnboundedSender<UiCommand>,
) -> DrainedMedia {
    let mut media = DrainedMedia::default();
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
                let mutual = profile.mutual_guild_ids.len();
                let label = format!("{mutual} Mutual Server{}", if mutual == 1 { "" } else { "s" });
                state.mutual_server_labels.insert(user_id.clone(), label);
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
                notify_if_relevant(state, &message, cmd_tx);
                if message.guild_id.is_none() && !state.dm_channels.iter().any(|c| c.id == message.channel_id) {
                    let _ = cmd_tx.send(UiCommand::RefreshDmChannels);
                }
                let is_actively_viewing = state.window_focused && state.selected_channel.as_deref() == Some(message.channel_id.as_str());
                if is_actively_viewing && state.mark_channel_read(&message.channel_id, &message.id) {
                    let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: message.channel_id.clone(), message_id: message.id.clone() });
                }
                state.push_message(message);
            }
            BackendEvent::MessagesAroundLoaded { channel_id, message_id, messages } => {
                state.set_message_history(channel_id.clone(), messages);
                state.reached_start.remove(&channel_id);
                state.scroll_to_message = Some(message_id);
                state.viewing_jumped_history = true;
            }
            BackendEvent::Gateway(GatewayEvent::InitialGuildChannels(guilds)) => {
                let count = state.seed_guild_channels(guilds);
                tracing::info!("seeded channels for {count} guilds from READY");
            }
            BackendEvent::Gateway(GatewayEvent::ReadState(states)) => {
                let advanced = state.apply_remote_read_state(&states);
                tracing::info!("adopted {advanced} of {} remote read states", states.len());
            }
            BackendEvent::Gateway(GatewayEvent::MessageAck { channel_id, message_id }) => {
                state.mark_channel_read(&channel_id, &message_id);
            }
            BackendEvent::Gateway(GatewayEvent::ChannelUpsert(channel)) => {
                state.upsert_channel(channel);
            }
            BackendEvent::Gateway(GatewayEvent::ChannelDelete { channel_id, guild_id }) => {
                state.remove_channel(&channel_id, guild_id.as_deref());
            }
            BackendEvent::Gateway(GatewayEvent::GuildUpdate { guild_id, name, icon }) => {
                state.update_guild(&guild_id, name, icon);
            }
            BackendEvent::Gateway(GatewayEvent::GuildDelete { guild_id }) => {
                state.remove_guild(&guild_id);
            }
            BackendEvent::Gateway(GatewayEvent::MessageDeleteBulk { channel_id, ids }) => {
                state.delete_messages(&channel_id, &ids);
            }
            BackendEvent::Gateway(GatewayEvent::GuildMemberUpsert { guild_id, member }) => {
                state.upsert_guild_member(&guild_id, member);
            }
            BackendEvent::Gateway(GatewayEvent::GuildMemberRemove { guild_id, user_id }) => {
                state.remove_guild_member(&guild_id, &user_id);
            }
            BackendEvent::Gateway(GatewayEvent::GuildRoleUpsert { guild_id, role }) => {
                state.upsert_role(&guild_id, role);
            }
            BackendEvent::Gateway(GatewayEvent::GuildRoleDelete { guild_id, role_id }) => {
                state.remove_role(&guild_id, &role_id);
            }
            BackendEvent::Gateway(GatewayEvent::GuildEmojisUpdate { guild_id, emojis }) => {
                state.guild_emojis.insert(guild_id, emojis);
            }
            BackendEvent::Gateway(GatewayEvent::InitialGuildStickers(guilds)) => {
                state.guild_stickers.extend(guilds);
            }
            BackendEvent::Gateway(GatewayEvent::GuildStickersUpdate { guild_id, stickers }) => {
                state.guild_stickers.insert(guild_id, stickers);
            }
            BackendEvent::Gateway(GatewayEvent::GuildSettings(settings)) => {
                state.apply_guild_settings(settings);
            }
            BackendEvent::GuildSettingsUpdated(settings) => {
                state.apply_guild_settings(vec![settings]);
            }
            BackendEvent::Gateway(GatewayEvent::RelationshipAdd(relationship)) => {
                state.upsert_relationship(relationship);
            }
            BackendEvent::Gateway(GatewayEvent::RelationshipRemove { user_id }) => {
                state.remove_relationship(&user_id);
            }
            BackendEvent::Gateway(GatewayEvent::TypingStart { channel_id, user_id }) => {
                state.note_typing(channel_id, user_id);
            }
            BackendEvent::Gateway(GatewayEvent::PresenceUpdate(presence)) => {
                state.presences.insert(presence.user.id.clone(), presence);
            }
            BackendEvent::Gateway(GatewayEvent::MessageUpdate { channel_id, id, content, edited_timestamp, call, poll }) => {
                state.update_message(&channel_id, &id, content, edited_timestamp, call, poll);
            }
            BackendEvent::Gateway(GatewayEvent::PollVote { channel_id, message_id, user_id, answer_id, added }) => {
                state.apply_poll_vote(&channel_id, &message_id, &user_id, answer_id, added);
            }
            BackendEvent::Gateway(GatewayEvent::MessageDelete { channel_id, id }) => {
                state.delete_message(&channel_id, &id);
            }
            BackendEvent::Gateway(GatewayEvent::ReactionAdd { channel_id, message_id, user_id, emoji }) => {
                state.apply_reaction(&channel_id, &message_id, &user_id, emoji, true);
            }
            BackendEvent::Gateway(GatewayEvent::ReactionRemove { channel_id, message_id, user_id, emoji }) => {
                state.apply_reaction(&channel_id, &message_id, &user_id, emoji, false);
            }
            BackendEvent::Gateway(GatewayEvent::ReactionRemoveAll { channel_id, message_id }) => {
                state.clear_reactions(&channel_id, &message_id);
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
            BackendEvent::Gateway(GatewayEvent::StreamCreate { stream_key, .. }) => {
                if state.unwatching_stream.as_deref() == Some(stream_key.as_str()) {
                    state.unwatching_stream = None;
                }
                if let Some(owner_id) = stream_key.rsplit(':').next() {
                    let owner_in_voice = state.voice_channel_states.values().flatten().any(|voice| voice.user_id == owner_id);
                    tracing::info!(owner_in_voice, "registered active screen share");
                    state.active_streams.insert(owner_id.to_string(), stream_key);
                }
            }
            BackendEvent::Gateway(GatewayEvent::StreamServerUpdate { .. }) => {}
            BackendEvent::Gateway(GatewayEvent::StreamDelete { stream_key }) => {
                let left_only_this_view = state.unwatching_stream.as_deref() == Some(stream_key.as_str());
                if left_only_this_view {
                    state.unwatching_stream = None;
                } else {
                    state.active_streams.retain(|_, key| key != &stream_key);
                }
                if state.watching_stream.as_deref() == Some(stream_key.as_str()) {
                    state.watching_stream = None;
                    state.screen_share_stats = Default::default();
                    state.screen_share_history.clear();
                }
            }
            BackendEvent::Gateway(GatewayEvent::GuildCreate { guild_id, roles, member_roles }) => {
                state.guild_roles.insert(guild_id.clone(), roles);
                if let Some(me) = &state.current_user {
                    if let Some((_, my_roles)) = member_roles.into_iter().find(|(user_id, _)| user_id == &me.id) {
                        state.my_guild_roles.insert(guild_id, my_roles);
                    }
                }
            }
            BackendEvent::Gateway(GatewayEvent::GuildMembersChunk { guild_id, members, presences }) => {
                for presence in presences {
                    state.presences.insert(presence.user.id.clone(), presence);
                }
                for member in &members {
                    state.known_users.insert(member.user.id.clone(), member.user.clone());
                }
                state.guild_members.entry(guild_id).or_default().extend(members);
            }
            BackendEvent::Gateway(GatewayEvent::Unknown { op_name, raw }) => {
                let channel_id = raw["channel_id"].as_str().unwrap_or_default().to_string();
                let me = state.current_user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                let ringing_me = raw["ringing"].as_array().is_some_and(|r| r.iter().any(|id| id.as_str() == Some(me.as_str())));
                match op_name.as_str() {
                    "CALL_CREATE" | "CALL_UPDATE" if ringing_me => {
                        if !state.incoming_calls.contains(&channel_id) {
                            if state.notifications_enabled {
                                std::thread::spawn(|| {
                                    crate::show_notification(&notify_rust::Notification::new().summary("Incoming call"));
                                    rustcord_client::voice_audio::play_notification_sound();
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
                state.screen_sharing = false;
                state.camera_on = false;
            }
            BackendEvent::VoiceDisconnected => {
                state.voice_connecting = false;
                state.screen_sharing = false;
                state.camera_on = false;
                state.connected_voice_channel = None;
                state.pending_voice_channel = None;
                state.voice_speaking_users.clear();
                state.voice_stats = Default::default();
                state.voice_history.clear();
                state.screen_share_stats = Default::default();
                state.screen_share_history.clear();
                state.show_voice_info = false;
                state.voice_joined_at = None;
            }
            BackendEvent::CameraStopped { error } => {
                state.camera_on = false;
                if let Some(error) = error {
                    state.status_message = Some(format!("Camera stopped: {error}"));
                }
            }
            BackendEvent::FontFetched { spec, error } => match error {
                Some(error) => {
                    tracing::warn!("theme font failed to load: {error}");
                    state.status_message = Some(format!("Theme font unavailable, using the default: {error}"));
                    state.font_fetches.insert(spec, Some(error));
                }
                None => {
                    state.font_fetches.remove(&spec);
                }
            },
            BackendEvent::CamerasLoaded(cameras) => {
                state.cameras = cameras;
                state.cameras_loaded = true;
            }
            BackendEvent::ScreenShareStopped => {
                state.screen_sharing = false;
                state.screen_share_stats = Default::default();
                state.screen_share_history.clear();
            }
            #[cfg(feature = "screenshare")]
            BackendEvent::ScreenAudioSourcesLoaded { sources, error } => {
                state.screen_audio_sources = sources;
                state.screen_audio_sources_error = error;
                state.screen_audio_sources_loaded = true;
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
            #[cfg(feature = "screenshare")]
            BackendEvent::ScreenShareStats(stats) => {
                if state.screen_share_history.len() >= 120 {
                    state.screen_share_history.pop_front();
                }
                state.screen_share_history.push_back(stats.clone());
                state.screen_share_stats = stats;
            }
            BackendEvent::VoiceDevicesLoaded { inputs, outputs } => {
                state.voice_input_devices = inputs;
                state.voice_output_devices = outputs;
            }
            BackendEvent::InviteResolved { code, result } => {
                let state_for = match result {
                    Ok(invite) => invites::InviteState::Ready(Box::new(invites::ResolvedInvite::new(invite))),
                    Err(backend::InviteFailure::Invalid) => invites::InviteState::Invalid,
                    Err(backend::InviteFailure::Unavailable) => invites::InviteState::Unavailable,
                };
                state.invites.insert(code, invites::InviteEntry { state: state_for, updated: std::time::Instant::now() });
            }
            BackendEvent::PinsLoaded { channel_id, pins } => {
                let entry = match pins {
                    Some(pins) => app_state::PinsState::Ready(pins),
                    None => app_state::PinsState::Failed,
                };
                state.pins.insert(channel_id, entry);
            }
            BackendEvent::UploadFinished { upload_id } => {
                state.uploads.retain(|u| u.id != upload_id);
            }
            BackendEvent::VoiceClipLoaded { url, result } => state.on_voice_clip_loaded(url, result),
            BackendEvent::VoiceMessageSent { channel_id, error } => {
                state.voice_messages_sending.remove(&channel_id);
                if let Some(e) = error {
                    state.status_message = Some(format!("Couldn't send the voice message: {e}"));
                }
            }
            BackendEvent::MessageForwarded { sent, failed } => {
                state.status_message = Some(match (sent, failed.first()) {
                    (_, None) if sent == 1 => "Message forwarded".to_string(),
                    (_, None) => format!("Message forwarded to {sent} conversations"),
                    (0, Some(e)) => format!("Couldn't forward the message: {e}"),
                    (_, Some(e)) => format!("Forwarded to {sent}, {} failed: {e}", failed.len()),
                });
            }
            BackendEvent::RelationshipSettled { user_id } => {
                state.relationship_busy.remove(&user_id);
            }
            BackendEvent::GuildJoined(guild) => {
                state.add_joined_guild(guild);
            }
            BackendEvent::MemberUpdated { guild_id, member } => {
                state.upsert_guild_member(&guild_id, member);
            }
            BackendEvent::MemberRoleChanged { guild_id, user_id, role_id, add } => {
                state.set_member_role(&guild_id, &user_id, &role_id, add);
            }
            BackendEvent::ChannelSaved(channel) => {
                state.upsert_channel(channel);
            }
            BackendEvent::InviteCreated { code } => {
                let link = format!("https://discord.gg/{code}");
                state.status_message = Some(if crate::clipboard::write_text(&link) {
                    format!("Invite link copied: {link}")
                } else {
                    format!("Invite created: {link}")
                });
            }
            BackendEvent::GuildInvitesLoaded { guild_id, result } => {
                if let Some(manager) = state.invite_manager.as_mut().filter(|m| m.guild_id == guild_id) {
                    manager.invites = Some(result);
                }
            }
            BackendEvent::InviteRevoked { code } => {
                if let Some(Some(Ok(invites))) = state.invite_manager.as_mut().map(|m| m.invites.as_mut()) {
                    invites.retain(|i| i.code != code);
                }
            }
            BackendEvent::MemberRemoved { guild_id, user_id } => {
                if let Some(members) = state.guild_members.get_mut(&guild_id) {
                    members.retain(|m| m.user.id != user_id);
                }
            }
            BackendEvent::MicTestStarted => {
                state.mic_test_running = true;
            }
            BackendEvent::MicTestFinished => {
                state.mic_test_running = false;
            }
            BackendEvent::GuildChannelsLoaded { guild_id, channels } => {
                state.guild_channels.insert(guild_id, channels);
            }
            BackendEvent::GuildRolesLoaded { guild_id, roles } => {
                state.guild_roles.insert(guild_id, roles);
            }
            BackendEvent::MyGuildMemberLoaded { guild_id, roles } => {
                state.my_guild_roles.insert(guild_id, roles);
            }
            BackendEvent::GuildEmojisLoaded { guild_id, emojis } => {
                state.guild_emojis.insert(guild_id, emojis);
            }
            BackendEvent::GuildStickersLoaded { guild_id, stickers } => {
                state.guild_stickers.insert(guild_id, stickers);
            }
            BackendEvent::StickerPacksLoaded(packs) => {
                state.sticker_packs = packs;
            }
            BackendEvent::GifsLoaded { query, result } => {
                state.gif_picker.apply(query, result);
            }
            BackendEvent::DmChannelsLoaded(channels) => {
                state.dm_channels = channels;
            }
            BackendEvent::MessagesLoaded { channel_id, messages } => {
                state.viewing_jumped_history = false;
                if let Some(newest) = messages.iter().max_by_key(|m| m.id.parse::<u64>().unwrap_or(0)) {
                    if state.mark_channel_read(&channel_id, &newest.id) {
                        let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: channel_id.clone(), message_id: newest.id.clone() });
                    }
                }
                state.note_history_page(&channel_id, messages.len());
                state.set_message_history(channel_id, messages);
            }
            BackendEvent::OlderMessagesLoaded { channel_id, messages } => {
                state.loading_older_messages = false;
                if messages.len() < backend::HISTORY_PAGE_SIZE as usize {
                    state.reached_start.insert(channel_id.clone());
                }
                state.prepend_message_history(channel_id, messages);
            }
            BackendEvent::SearchResults { query, filters, offset, total, messages } => {
                if let Some(search) = &mut state.search {
                    if search.query == query && search.offset == offset && search.filters == filters {
                        search.total = total;
                        search.results = messages;
                        search.status = app_state::SearchStatus::Ready;
                    }
                }
            }
            BackendEvent::SearchIndexing { retry_after } => {
                if let Some(search) = &mut state.search {
                    search.status = app_state::SearchStatus::Indexing { retry_after };
                }
            }
            BackendEvent::SearchFailed(message) => {
                if let Some(search) = &mut state.search {
                    search.status = app_state::SearchStatus::Failed(message);
                }
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
                media.images.push((url, image));
            }
            #[cfg(feature = "screenshare")]
            BackendEvent::RemoteVideoFrame { user_id, rgba } => {
                media.remote_videos.push((user_id, rgba));
            }
            #[cfg(feature = "video")]
            BackendEvent::VideoBatchReady(batch) => {
                media.videos.push(batch);
            }
        }
    }
    media
}

#[derive(Default)]
pub struct DrainedMedia {
    pub images: Vec<(String, images::Decoded)>,
    #[cfg(feature = "screenshare")]
    pub remote_videos: Vec<(String, image::RgbaImage)>,
    #[cfg(feature = "video")]
    pub videos: Vec<images::VideoBatch>,
}
