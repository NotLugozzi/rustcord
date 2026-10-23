mod app_state;
mod backend;
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

    let mut state = AppState::default();
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

pub fn drain_backend_events(state: &mut AppState, event_rx: &mut mpsc::UnboundedReceiver<BackendEvent>) {
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
            BackendEvent::Error(err) => {
                state.status_message = Some(err);
            }
            BackendEvent::Gateway(GatewayEvent::Ready { user, guilds }) => {
                state.current_user = Some(user);
                state.guilds = guilds;
            }
            BackendEvent::Gateway(GatewayEvent::MessageCreate(message)) => {
                state.push_message(message);
            }
            BackendEvent::Gateway(GatewayEvent::Unknown { .. }) => {}
            BackendEvent::GuildChannelsLoaded { guild_id, channels } => {
                state.guild_channels.insert(guild_id, channels);
            }
            BackendEvent::DmChannelsLoaded(channels) => {
                state.dm_channels = channels;
            }
            BackendEvent::MessagesLoaded { channel_id, messages } => {
                state.messages.insert(channel_id, messages);
            }
        }
    }
}
