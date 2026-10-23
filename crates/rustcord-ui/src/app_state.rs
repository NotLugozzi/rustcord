use rustcord_client::models::{Channel, ChannelType, Guild, Message, User};
use std::collections::HashMap;

#[derive(Default)]
pub struct AppState {
    pub current_user: Option<User>,
    pub guilds: Vec<Guild>,
    pub dm_channels: Vec<Channel>,
    pub guild_channels: HashMap<String, Vec<Channel>>,
    pub messages: HashMap<String, Vec<Message>>,
    pub selected_guild: Option<String>,
    pub selected_channel: Option<String>,
    pub compose_buf: String,
    pub email_input: String,
    pub password_input: String,
    pub mfa_code_input: String,
    pub mfa_ticket: Option<String>,
    pub login_error: Option<String>,
    pub login_status: Option<String>,
    pub logged_in: bool,
    pub theme: crate::theme::Theme,
    pub status_message: Option<String>,
}

impl AppState {
    pub fn push_message(&mut self, message: Message) {
        self.messages
            .entry(message.channel_id.clone())
            .or_default()
            .push(message);
    }

    pub fn active_messages(&self) -> &[Message] {
        self.selected_channel
            .as_ref()
            .and_then(|id| self.messages.get(id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn text_channels_for_selected_guild(&self) -> Vec<&Channel> {
        let Some(guild_id) = &self.selected_guild else { return Vec::new() };
        let Some(channels) = self.guild_channels.get(guild_id) else { return Vec::new() };
        let mut channels: Vec<&Channel> = channels
            .iter()
            .filter(|c| matches!(c.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement))
            .collect();
        channels.sort_by_key(|c| c.position.unwrap_or(i32::MAX));
        channels
    }
}
