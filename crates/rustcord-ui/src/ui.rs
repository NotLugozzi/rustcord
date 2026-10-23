use crate::app_state::AppState;
use crate::backend::UiCommand;
use crate::images::TextureCache;
use crate::markdown::{self, Block, Span, Style};
use imgui::Ui;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Copy)]
pub struct Fonts {
    pub regular: imgui::FontId,
    pub bold: imgui::FontId,
    pub italic: imgui::FontId,
    pub bold_italic: imgui::FontId,
    pub mono: imgui::FontId,
}

impl Fonts {
    fn pick(&self, style: Style) -> imgui::FontId {
        match (style.bold, style.italic) {
            (true, true) => self.bold_italic,
            (true, false) => self.bold,
            (false, true) => self.italic,
            (false, false) => self.regular,
        }
    }
}

fn format_timestamp(iso: &str) -> String {
    iso.get(11..16).unwrap_or("--:--").to_string()
}

fn draw_markdown(ui: &Ui, fonts: &Fonts, theme: &crate::theme::Theme, content: &str, revealed_spoilers: &mut std::collections::HashSet<usize>, id_seed: &str) {
    let blocks = markdown::parse(content);
    for (block_idx, block) in blocks.iter().enumerate() {
        match block {
            Block::Paragraph(spans) => draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx),
            Block::Heading { level, spans } => {
                let font = match level {
                    1 => fonts.bold,
                    2 => fonts.bold,
                    _ => fonts.bold,
                };
                let scale = match level {
                    1 => 1.4,
                    2 => 1.2,
                    _ => 1.05,
                };
                let token = ui.push_font(font);
                ui.set_window_font_scale(scale);
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx);
                ui.set_window_font_scale(1.0);
                token.pop();
            }
            Block::Blockquote(spans) => {
                let token = ui.push_style_color(imgui::StyleColor::Text, theme.text_muted);
                ui.text("| ");
                ui.same_line();
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx);
                token.pop();
            }
            Block::CodeBlock { code, language } => {
                if let Some(lang) = language {
                    let token = ui.push_style_color(imgui::StyleColor::Text, theme.text_muted);
                    ui.text(lang);
                    token.pop();
                }
                let token = ui.push_font(fonts.mono);
                let bg = ui.push_style_color(imgui::StyleColor::ChildBg, theme.panel);
                ui.child_window(format!("code_{id_seed}_{block_idx}"))
                    .size([0.0, ui.calc_text_size(code)[1] + 16.0])
                    .always_use_window_padding(true)
                    .build(|| {
                        ui.text_wrapped(code);
                    });
                bg.pop();
                token.pop();
            }
        }
    }
}

fn draw_spans(
    ui: &Ui,
    fonts: &Fonts,
    theme: &crate::theme::Theme,
    spans: &[Span],
    revealed_spoilers: &mut std::collections::HashSet<usize>,
    id_seed: &str,
    block_idx: usize,
) {
    for (span_idx, span) in spans.iter().enumerate() {
        if span_idx > 0 {
            ui.same_line_with_spacing(0.0, 0.0);
        }
        match span {
            Span::Text { text, style } => {
                if style.spoiler {
                    let key = spoiler_key(id_seed, block_idx, span_idx);
                    if revealed_spoilers.contains(&key) {
                        draw_styled_text(ui, fonts, theme, text, *style);
                    } else {
                        let bg = ui.push_style_color(imgui::StyleColor::Button, theme.text_muted);
                        if ui.button(format!("spoiler##{key}")) {
                            revealed_spoilers.insert(key);
                        }
                        bg.pop();
                    }
                    continue;
                }
                draw_styled_text(ui, fonts, theme, text, *style);
            }
            Span::Link { label, url } => {
                let color = ui.push_style_color(imgui::StyleColor::Text, theme.accent);
                if ui.selectable(label) {
                    let _ = open::that(url);
                }
                color.pop();
                if ui.is_item_hovered() {
                    ui.tooltip_text(url);
                }
            }
        }
    }
}

fn draw_styled_text(ui: &Ui, fonts: &Fonts, theme: &crate::theme::Theme, text: &str, style: Style) {
    let font_token = ui.push_font(fonts.pick(style));
    let color_token = if style.code {
        Some(ui.push_style_color(imgui::StyleColor::Text, theme.accent))
    } else {
        None
    };
    if style.code {
        let mono_token = ui.push_font(fonts.mono);
        ui.text(text);
        mono_token.pop();
    } else {
        ui.text_wrapped(text);
    }
    if let Some(t) = color_token {
        t.pop();
    }
    font_token.pop();
    if style.strikethrough || style.underline {
        let [x, y] = ui.item_rect_min();
        let [x2, y2] = ui.item_rect_max();
        let line_y = if style.strikethrough { (y + y2) / 2.0 } else { y2 };
        ui.get_window_draw_list()
            .add_line([x, line_y], [x2, line_y], theme.text)
            .build();
    }
}

fn spoiler_key(id_seed: &str, block_idx: usize, span_idx: usize) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id_seed.hash(&mut hasher);
    block_idx.hash(&mut hasher);
    span_idx.hash(&mut hasher);
    hasher.finish() as usize
}

fn is_image_attachment(content_type: Option<&str>, filename: &str) -> bool {
    if let Some(ct) = content_type {
        if ct.starts_with("image/") {
            return true;
        }
    }
    let lower = filename.to_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|ext| lower.ends_with(ext))
}

const MAX_IMAGE_WIDTH: f32 = 360.0;
const MAX_IMAGE_HEIGHT: f32 = 300.0;

fn scaled_image_size(natural: [f32; 2]) -> [f32; 2] {
    let [w, h] = natural;
    if w <= 0.0 || h <= 0.0 {
        return [MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT];
    }
    let scale = (MAX_IMAGE_WIDTH / w).min(MAX_IMAGE_HEIGHT / h).min(1.0);
    [w * scale, h * scale]
}

const AVATAR_SIZE: [f32; 2] = [40.0, 40.0];
const MESSAGE_GAP: f32 = 12.0;

fn draw_attachment(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    attachment: &rustcord_client::models::Attachment,
) {
    if !is_image_attachment(attachment.content_type.as_deref(), &attachment.filename) {
        ui.text_disabled(format!("[attachment: {}]", attachment.filename));
        return;
    }
    if let Some((texture_id, natural_size)) = textures.get(&attachment.url) {
        imgui::Image::new(texture_id, scaled_image_size(natural_size)).build(ui);
    } else if textures.is_failed(&attachment.url) {
        ui.text_disabled(format!("[failed to load image: {}]", attachment.filename));
    } else {
        textures.request(&attachment.url, cmd_tx);
        ui.text_disabled("[loading image...]");
    }
}

pub fn draw(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache, fonts: &Fonts) {
    if !state.logged_in {
        draw_login(ui, state, cmd_tx);
        let _ = textures;
        return;
    }

    let [display_w, display_h] = ui.io().display_size;

    ui.window("servers")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([64.0, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .build(|| {
            let dm_button_color = if state.selected_guild.is_none() {
                state.theme.accent_hover
            } else {
                state.theme.accent
            };
            let token = ui.push_style_color(imgui::StyleColor::Button, dm_button_color);
            if ui.button_with_size("DM", [48.0, 48.0]) {
                state.selected_guild = None;
                state.selected_channel = None;
            }
            token.pop();
            ui.separator();

            for guild in &state.guilds {
                let icon_url = rustcord_client::cdn::guild_icon_url(guild, 64);
                let clicked = if let Some(url) = &icon_url {
                    if let Some((texture_id, _)) = textures.get(url) {
                        ui.image_button(format!("guild_icon_{}", guild.id), texture_id, [48.0, 48.0])
                    } else {
                        textures.request(url, cmd_tx);
                        let label = guild.name.chars().take(2).collect::<String>();
                        ui.button_with_size(&label, [48.0, 48.0])
                    }
                } else {
                    let label = guild.name.chars().take(2).collect::<String>();
                    ui.button_with_size(&label, [48.0, 48.0])
                };
                if clicked {
                    let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild.id.clone() });
                    state.selected_guild = Some(guild.id.clone());
                    state.selected_channel = None;
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(&guild.name);
                }
            }
            ui.set_cursor_pos([8.0, display_h - 56.0]);
            if ui.button_with_size("Out", [48.0, 48.0]) {
                let _ = cmd_tx.send(UiCommand::Logout);
                *state = AppState {
                    theme: state.theme.clone(),
                    ..Default::default()
                };
            }
        });

    ui.window("channels")
        .position([64.0, 0.0], imgui::Condition::Always)
        .size([200.0, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .build(|| {
            ui.child_window("channels_scroll").size([0.0, display_h - 48.0]).build(|| {
            if state.selected_guild.is_some() {
                let groups: Vec<(Option<String>, Vec<(String, String)>)> = state
                    .grouped_channels_for_selected_guild()
                    .into_iter()
                    .map(|g| {
                        let channels = g
                            .channels
                            .into_iter()
                            .map(|c| (c.id.clone(), c.name.clone().unwrap_or_else(|| c.id.clone())))
                            .collect();
                        (g.name.map(str::to_string), channels)
                    })
                    .collect();
                if groups.is_empty() {
                    ui.text_disabled("No text channels loaded yet.");
                }
                for (category_name, channels) in &groups {
                    if let Some(name) = category_name {
                        ui.spacing();
                        let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                        ui.text(name.to_uppercase());
                        token.pop();
                    }
                    for (channel_id, name) in channels {
                        if ui.selectable(format!("# {name}")) {
                            let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
                            state.selected_channel = Some(channel_id.clone());
                        }
                    }
                }
            } else {
                ui.input_text("##search", &mut state.dm_search)
                    .hint("Search friends or DMs")
                    .build();

                let friends_label = if state.show_friends_list { "< Back to DMs" } else { "Friends" };
                if ui.selectable(friends_label) {
                    state.show_friends_list = !state.show_friends_list;
                }
                ui.separator();

                if state.show_friends_list {
                    let friends: Vec<(String, String)> = state
                        .friends()
                        .into_iter()
                        .map(|r| (r.user.id.clone(), r.user.global_name.clone().unwrap_or_else(|| r.user.username.clone())))
                        .collect();
                    if friends.is_empty() {
                        ui.text_disabled("No friends found.");
                    }
                    for (user_id, name) in &friends {
                        if ui.selectable(name) {
                            let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: user_id.clone() });
                        }
                    }
                } else {
                    let dms: Vec<(String, String)> = state
                        .filtered_dm_channels()
                        .into_iter()
                        .map(|dm| (dm.id.clone(), AppState::dm_display_name(dm)))
                        .collect();
                    if dms.is_empty() {
                        ui.text_disabled("No direct messages.");
                    }
                    for (channel_id, name) in &dms {
                        if ui.selectable(name) {
                            let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
                            state.selected_channel = Some(channel_id.clone());
                        }
                    }
                }
            }
            });

            ui.separator();
            draw_user_bar(ui, state, cmd_tx, textures);
        });

    draw_settings_dialog(ui, state, cmd_tx);

    ui.window("chat")
        .position([264.0, 0.0], imgui::Condition::Always)
        .size([display_w - 264.0, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .build(|| {
            let status_height = if state.status_message.is_some() { 24.0 } else { 0.0 };
            let messages = state.active_messages().to_vec();
            let channel_key = state.selected_channel.clone().unwrap_or_default();
            let seen_count = state.last_seen_message_count.get(&channel_key).copied().unwrap_or(0);
            let has_new_messages = messages.len() > seen_count;

            ui.child_window("history")
                .size([0.0, display_h - 60.0 - status_height])
                .build(|| {
                    if messages.is_empty() {
                        ui.text_disabled("No messages loaded yet.");
                    }

                    if state.loading_older_messages {
                        ui.text_disabled("Loading older messages...");
                    }

                    for msg in &messages {
                        let author = msg.author.global_name.clone().unwrap_or_else(|| msg.author.username.clone());
                        let avatar_url = rustcord_client::cdn::avatar_url(&msg.author, 64)
                            .unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&msg.author));

                        let row_start = ui.cursor_pos();
                        if let Some((texture_id, _)) = textures.get(&avatar_url) {
                            imgui::Image::new(texture_id, AVATAR_SIZE).build(ui);
                        } else {
                            textures.request(&avatar_url, cmd_tx);
                            ui.dummy(AVATAR_SIZE);
                        }

                        ui.set_cursor_pos([row_start[0] + AVATAR_SIZE[0] + 12.0, row_start[1]]);
                        ui.group(|| {
                            ui.text_colored(state.theme.accent, &author);
                            ui.same_line();
                            let time = format_timestamp(&msg.timestamp);
                            let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                            ui.set_window_font_scale(0.85);
                            ui.text(time);
                            ui.set_window_font_scale(1.0);
                            token.pop();

                            if !msg.content.is_empty() {
                                draw_markdown(ui, fonts, &state.theme, &msg.content, &mut state.revealed_spoilers, &msg.id);
                            }

                            for attachment in &msg.attachments {
                                draw_attachment(ui, cmd_tx, textures, attachment);
                            }
                        });

                        ui.set_cursor_pos([row_start[0], row_start[1] + AVATAR_SIZE[1].max(ui.item_rect_size()[1])]);
                        ui.dummy([0.0, MESSAGE_GAP]);
                    }

                    let scroll_y = ui.scroll_y();
                    let near_bottom = scroll_y >= ui.scroll_max_y() - 40.0;
                    if has_new_messages && (near_bottom || seen_count == 0) {
                        ui.set_scroll_here_y_with_ratio(1.0);
                    }

                    if scroll_y <= 0.0 && !messages.is_empty() && !state.loading_older_messages {
                        if let (Some(channel_id), Some(oldest)) = (state.selected_channel.clone(), messages.first()) {
                            state.loading_older_messages = true;
                            let _ = cmd_tx.send(UiCommand::LoadOlderMessages {
                                channel_id,
                                before: oldest.id.clone(),
                            });
                        }
                    }
                });
            state.last_seen_message_count.insert(channel_key.clone(), messages.len());

            if !channel_key.is_empty() {
                let typing_names: Vec<String> = state
                    .typing_users_for(&channel_key)
                    .into_iter()
                    .filter(|id| state.current_user.as_ref().is_none_or(|u| u.id != *id))
                    .map(|id| state.typing_display_name(&channel_key, id))
                    .collect();
                if !typing_names.is_empty() {
                    let text = match typing_names.as_slice() {
                        [a] => format!("{a} is typing..."),
                        [a, b] => format!("{a} and {b} are typing..."),
                        _ => format!("{} people are typing...", typing_names.len()),
                    };
                    let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                    ui.text(text);
                    token.pop();
                }
            }

            if let Some(status) = state.status_message.clone() {
                let token = ui.push_style_color(imgui::StyleColor::Text, [1.0, 0.6, 0.3, 1.0]);
                if ui.selectable(format!("{status}  (click to dismiss)")) {
                    state.status_message = None;
                }
                token.pop();
            }

            ui.separator();

            if let Some(path) = state.pending_attachment.clone() {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                ui.text_colored(state.theme.accent, format!("Attached: {name}"));
                ui.same_line();
                if ui.small_button("x") {
                    state.pending_attachment = None;
                }
            }

            if ui.button("Attach") {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_file()
                {
                    state.pending_attachment = Some(path);
                }
            }
            ui.same_line();

            let before_edit = state.compose_buf.clone();
            let enter_pressed = ui
                .input_text("##compose", &mut state.compose_buf)
                .enter_returns_true(true)
                .build();

            if state.compose_buf != before_edit && !state.compose_buf.is_empty() {
                if let Some(channel_id) = &state.selected_channel {
                    let should_send = state
                        .last_typing_sent
                        .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(8));
                    if should_send {
                        state.last_typing_sent = Some(std::time::Instant::now());
                        let _ = cmd_tx.send(UiCommand::StartTyping { channel_id: channel_id.clone() });
                    }
                }
            }

            if enter_pressed {
                if let Some(channel_id) = state.selected_channel.clone() {
                    let content = std::mem::take(&mut state.compose_buf);
                    if let Some(path) = state.pending_attachment.take() {
                        let _ = cmd_tx.send(UiCommand::SendAttachment { channel_id, content, path });
                    } else if !content.trim().is_empty() {
                        let _ = cmd_tx.send(UiCommand::SendMessage { channel_id, content });
                    }
                }
            }
        });
}

fn draw_user_bar(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(user) = state.current_user.clone() else { return };
    let avatar_url =
        rustcord_client::cdn::avatar_url(&user, 32).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
    if let Some((texture_id, _)) = textures.get(&avatar_url) {
        imgui::Image::new(texture_id, [28.0, 28.0]).build(ui);
    } else {
        textures.request(&avatar_url, cmd_tx);
        ui.dummy([28.0, 28.0]);
    }
    ui.same_line();

    let display_name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
    ui.text(&display_name);
    ui.same_line();

    if ui.small_button("Settings") {
        state.show_settings = !state.show_settings;
    }
}

fn draw_settings_dialog(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    if !state.show_settings {
        return;
    }
    let [display_w, display_h] = ui.io().display_size;
    let mut open = state.show_settings;
    ui.window("Settings")
        .opened(&mut open)
        .position([display_w / 2.0 - 220.0, display_h / 2.0 - 180.0], imgui::Condition::Appearing)
        .size([440.0, 360.0], imgui::Condition::Appearing)
        .build(|| {
            if let Some(user) = &state.current_user {
                ui.text("Account");
                ui.separator();
                let display_name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
                ui.text(format!("Username: {display_name} (@{})", user.username));
                ui.text(format!("User ID: {}", user.id));
                ui.spacing();
                if ui.button("Log out") {
                    let _ = cmd_tx.send(UiCommand::Logout);
                    let theme = state.theme.clone();
                    *state = AppState { theme, ..Default::default() };
                    return;
                }
            }

            ui.spacing();
            ui.separator();
            ui.text("Theme");
            ui.separator();

            let theme_paths = crate::theme::Theme::list_available();
            if theme_paths.is_empty() {
                ui.text_disabled("No themes found in config directory.");
            }
            for path in &theme_paths {
                let label = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let is_current = label == state.theme.name;
                if ui.selectable_config(&label).selected(is_current).build() {
                    if let Ok(theme) = crate::theme::Theme::load_from_file(path) {
                        state.theme = theme;
                    }
                }
            }
        });
    state.show_settings = open;
}

fn draw_login(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let [display_w, display_h] = ui.io().display_size;
    ui.window("login")
        .position([display_w / 2.0 - 220.0, display_h / 2.0 - 110.0], imgui::Condition::Always)
        .size([440.0, 260.0], imgui::Condition::Always)
        .no_decoration()
        .build(|| {
            if let Some(ticket) = state.mfa_ticket.clone() {
                ui.text("Enter 2FA code:");
                ui.input_text("##mfa", &mut state.mfa_code_input).build();
                if ui.button("Verify") {
                    let _ = cmd_tx.send(UiCommand::SubmitMfa { ticket, code: state.mfa_code_input.clone() });
                }
            } else {
                ui.text("Email:");
                ui.input_text("##email", &mut state.email_input).build();
                ui.text("Password:");
                ui.input_text("##password", &mut state.password_input)
                    .password(true)
                    .build();
                if ui.button("Login") {
                    let _ = cmd_tx.send(UiCommand::LoginWithPassword {
                        email: state.email_input.clone(),
                        password: state.password_input.clone(),
                    });
                }
            }
            if let Some(err) = &state.login_error {
                let token = ui.push_style_color(imgui::StyleColor::Text, [1.0, 0.4, 0.4, 1.0]);
                ui.text_wrapped(err);
                token.pop();
            }
            if let Some(status) = &state.login_status {
                let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                ui.text_wrapped(status);
                token.pop();
            }
        });
}
