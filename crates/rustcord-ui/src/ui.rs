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
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| "--:--".to_string())
}

fn same_group_window(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (chrono::DateTime::parse_from_rfc3339(a), chrono::DateTime::parse_from_rfc3339(b)) else {
        return false;
    };
    (b - a).num_minutes().abs() < 5
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
const RAIL_WIDTH: f32 = 72.0;
const RAIL_ICON_SIZE: [f32; 2] = [48.0, 48.0];
const RAIL_PADDING_TOP: f32 = 12.0;
const CHANNEL_LIST_WIDTH: f32 = 220.0;

fn draw_rail_circle(ui: &Ui, color: [f32; 4]) {
    let p_min = ui.cursor_screen_pos();
    let p_max = [p_min[0] + RAIL_ICON_SIZE[0], p_min[1] + RAIL_ICON_SIZE[1]];
    let center = [(p_min[0] + p_max[0]) / 2.0, (p_min[1] + p_max[1]) / 2.0];
    ui.get_window_draw_list()
        .add_circle(center, RAIL_ICON_SIZE[0] / 2.0, color)
        .filled(true)
        .build();
}

fn draw_circular_image_button_placeholder(ui: &Ui, id: &str, label: &str, size: [f32; 2]) -> bool {
    let p_min = ui.cursor_screen_pos();
    let p_max = [p_min[0] + size[0], p_min[1] + size[1]];
    let center = [(p_min[0] + p_max[0]) / 2.0, (p_min[1] + p_max[1]) / 2.0];
    let text_size = ui.calc_text_size(label);
    ui.set_cursor_screen_pos([center[0] - text_size[0] / 2.0, center[1] - text_size[1] / 2.0]);
    ui.text(label);
    ui.set_cursor_screen_pos(p_min);
    let clicked = ui.invisible_button(id, size);
    if ui.is_item_hovered() {
        ui.get_window_draw_list()
            .add_circle(center, size[0].min(size[1]) / 2.0 + 2.0, [1.0, 1.0, 1.0, 0.5])
            .thickness(2.0)
            .build();
    }
    clicked
}

fn clickable_text(ui: &Ui, id: &str, text: &str, color: [f32; 4]) -> bool {
    let start = ui.cursor_pos();
    ui.text_colored(color, text);
    let size = ui.item_rect_size();
    ui.set_cursor_pos(start);
    let clicked = ui.invisible_button(id, size);
    ui.set_cursor_pos(start);
    ui.text_colored(color, text);
    clicked
}

const USER_ROW_AVATAR_SIZE: [f32; 2] = [28.0, 28.0];

struct UserRowResult {
    row_clicked: bool,
    avatar_clicked: bool,
}

fn status_color(status: rustcord_client::models::PresenceStatus) -> [f32; 4] {
    use rustcord_client::models::PresenceStatus::*;
    match status {
        Online => [0.23, 0.79, 0.35, 1.0],
        Idle => [0.95, 0.68, 0.11, 1.0],
        Dnd => [0.91, 0.23, 0.23, 1.0],
        Offline | Invisible => [0.50, 0.52, 0.56, 1.0],
    }
}

fn draw_status_dot(ui: &Ui, center: [f32; 2], radius: f32, status: rustcord_client::models::PresenceStatus, bg_color: [f32; 4]) {
    let draw_list = ui.get_window_draw_list();
    draw_list.add_circle(center, radius + 2.0, bg_color).filled(true).build();
    draw_list.add_circle(center, radius, status_color(status)).filled(true).build();
}

fn draw_user_row(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    user: &rustcord_client::models::User,
    name: &str,
    selected: bool,
    status: rustcord_client::models::PresenceStatus,
    platform: Option<&str>,
    row_bg_color: [f32; 4],
) -> UserRowResult {
    let row_start = ui.cursor_pos();
    let row_height = USER_ROW_AVATAR_SIZE[1] + 4.0;

    let row_clicked = ui
        .selectable_config(format!("##row_{}", user.id))
        .selected(selected)
        .size([0.0, row_height])
        .build();

    ui.set_cursor_pos([row_start[0] + 4.0, row_start[1] + 2.0]);
    let avatar_screen_pos = ui.cursor_screen_pos();
    let avatar_url = rustcord_client::cdn::avatar_url(user, 32).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
    let avatar_clicked = if let Some((texture_id, _)) = textures.get(&avatar_url) {
        draw_circular_image_button(ui, &format!("avatar_row_{}", user.id), texture_id, USER_ROW_AVATAR_SIZE)
    } else {
        textures.request(&avatar_url, cmd_tx);
        ui.dummy(USER_ROW_AVATAR_SIZE);
        false
    };
    draw_status_dot(
        ui,
        [avatar_screen_pos[0] + USER_ROW_AVATAR_SIZE[0] - 3.0, avatar_screen_pos[1] + USER_ROW_AVATAR_SIZE[1] - 3.0],
        5.0,
        status,
        row_bg_color,
    );

    ui.same_line();
    let text_x = ui.cursor_pos()[0];
    let text_y = row_start[1] + row_height / 2.0 - ui.text_line_height() / 2.0;
    ui.set_cursor_pos([text_x, text_y]);
    let name_clicked = clickable_text(ui, &format!("name_row_{}", user.id), name, ui.style_color(imgui::StyleColor::Text));
    if let Some(platform) = platform {
        ui.same_line();
        let token = ui.push_style_color(imgui::StyleColor::Text, [0.6, 0.62, 0.66, 1.0]);
        ui.set_window_font_scale(0.8);
        ui.text(platform);
        ui.set_window_font_scale(1.0);
        token.pop();
    }

    ui.set_cursor_pos([row_start[0], row_start[1] + row_height]);
    UserRowResult { row_clicked, avatar_clicked: avatar_clicked || name_clicked }
}

fn draw_circular_image(ui: &Ui, texture_id: imgui::TextureId, size: [f32; 2]) {
    let p_min = ui.cursor_screen_pos();
    let p_max = [p_min[0] + size[0], p_min[1] + size[1]];
    ui.get_window_draw_list()
        .add_image_rounded(texture_id, p_min, p_max, size[0].min(size[1]) / 2.0)
        .build();
    ui.dummy(size);
}

fn draw_circular_image_button(ui: &Ui, id: &str, texture_id: imgui::TextureId, size: [f32; 2]) -> bool {
    let p_min = ui.cursor_screen_pos();
    let p_max = [p_min[0] + size[0], p_min[1] + size[1]];
    let draw_list = ui.get_window_draw_list();
    draw_list.add_image_rounded(texture_id, p_min, p_max, size[0].min(size[1]) / 2.0).build();
    let clicked = ui.invisible_button(id, size);
    if ui.is_item_hovered() {
        draw_list
            .add_circle(
                [(p_min[0] + p_max[0]) / 2.0, (p_min[1] + p_max[1]) / 2.0],
                size[0].min(size[1]) / 2.0 + 2.0,
                [1.0, 1.0, 1.0, 0.5],
            )
            .thickness(2.0)
            .build();
    }
    clicked
}

fn draw_attachment(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    attachment: &rustcord_client::models::Attachment,
    lightbox: &mut Option<String>,
) {
    if !is_image_attachment(attachment.content_type.as_deref(), &attachment.filename) {
        ui.text_disabled(format!("[attachment: {}]", attachment.filename));
        return;
    }
    if let Some((texture_id, natural_size)) = textures.get(&attachment.url) {
        let size = scaled_image_size(natural_size);
        let p_min = ui.cursor_screen_pos();
        imgui::Image::new(texture_id, size).build(ui);
        if ui.is_item_clicked() {
            *lightbox = Some(attachment.url.clone());
        }
        if ui.is_item_hovered() {
            let p_max = [p_min[0] + size[0], p_min[1] + size[1]];
            ui.get_window_draw_list()
                .add_rect(p_min, p_max, [1.0, 1.0, 1.0, 0.12])
                .filled(true)
                .build();
        }
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
        .size([RAIL_WIDTH, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .build(|| {
            let rail_center_x = (RAIL_WIDTH - RAIL_ICON_SIZE[0]) / 2.0;

            ui.set_cursor_pos([rail_center_x, RAIL_PADDING_TOP]);
            let dm_color = if state.selected_guild.is_none() { state.theme.accent_hover } else { state.theme.accent };
            draw_rail_circle(ui, dm_color);
            if draw_circular_image_button_placeholder(ui, "dm_button", "DM", RAIL_ICON_SIZE) {
                state.selected_guild = None;
                state.selected_channel = None;
            }

            ui.set_cursor_pos([8.0, RAIL_PADDING_TOP + RAIL_ICON_SIZE[1] + 8.0]);
            ui.separator();
            let after_separator_y = ui.cursor_pos()[1];
            ui.set_cursor_pos([rail_center_x, after_separator_y]);

            for guild in &state.guilds {
                let icon_url = rustcord_client::cdn::guild_icon_url(guild, 64);
                let clicked = if let Some(url) = &icon_url {
                    if let Some((texture_id, _)) = textures.get(url) {
                        draw_circular_image_button(ui, &format!("guild_icon_{}", guild.id), texture_id, RAIL_ICON_SIZE)
                    } else {
                        textures.request(url, cmd_tx);
                        let label = guild.name.chars().take(2).collect::<String>();
                        draw_rail_circle(ui, state.theme.panel);
                        draw_circular_image_button_placeholder(ui, &format!("guild_{}", guild.id), &label, RAIL_ICON_SIZE)
                    }
                } else {
                    let label = guild.name.chars().take(2).collect::<String>();
                    draw_rail_circle(ui, state.theme.panel);
                    draw_circular_image_button_placeholder(ui, &format!("guild_{}", guild.id), &label, RAIL_ICON_SIZE)
                };
                if clicked {
                    let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild.id.clone() });
                    state.selected_guild = Some(guild.id.clone());
                    state.selected_channel = None;
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(&guild.name);
                }
                let next_y = ui.cursor_pos()[1];
                ui.set_cursor_pos([rail_center_x, next_y]);
            }
        });

    ui.window("channels")
        .position([RAIL_WIDTH, 0.0], imgui::Condition::Always)
        .size([CHANNEL_LIST_WIDTH, display_h], imgui::Condition::Always)
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
                    let friends: Vec<rustcord_client::models::User> =
                        state.friends().into_iter().map(|r| r.user.clone()).collect();
                    if friends.is_empty() {
                        ui.text_disabled("No friends found.");
                    }
                    for user in &friends {
                        let name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
                        let status = state.presence_for(&user.id);
                        let platform = state.platform_for(&user.id).map(str::to_string);
                        let result = draw_user_row(
                            ui,
                            cmd_tx,
                            textures,
                            user,
                            &name,
                            false,
                            status,
                            platform.as_deref(),
                            state.theme.panel,
                        );
                        if result.avatar_clicked {
                            state.profile_popout = Some(user.clone());
                        } else if result.row_clicked {
                            let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: user.id.clone() });
                        }
                    }
                } else {
                    let dms: Vec<(String, String, Option<rustcord_client::models::User>)> = state
                        .filtered_dm_channels()
                        .into_iter()
                        .map(|dm| (dm.id.clone(), AppState::dm_display_name(dm), dm.recipients.first().cloned()))
                        .collect();
                    if dms.is_empty() {
                        ui.text_disabled("No direct messages.");
                    }
                    for (channel_id, name, recipient) in &dms {
                        let is_selected = state.selected_channel.as_deref() == Some(channel_id.as_str());
                        let (row_clicked, avatar_clicked) = if let Some(user) = recipient {
                            let status = state.presence_for(&user.id);
                            let platform = state.platform_for(&user.id).map(str::to_string);
                            let result = draw_user_row(
                                ui,
                                cmd_tx,
                                textures,
                                user,
                                name,
                                is_selected,
                                status,
                                platform.as_deref(),
                                state.theme.panel,
                            );
                            (result.row_clicked, result.avatar_clicked)
                        } else {
                            (ui.selectable_config(name).selected(is_selected).build(), false)
                        };
                        if avatar_clicked {
                            if let Some(user) = recipient {
                                state.profile_popout = Some(user.clone());
                            }
                        } else if row_clicked {
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
    draw_profile_popout(ui, state, cmd_tx, textures);

    ui.window("chat")
        .position([RAIL_WIDTH + CHANNEL_LIST_WIDTH, 0.0], imgui::Condition::Always)
        .size([display_w - RAIL_WIDTH - CHANNEL_LIST_WIDTH, display_h], imgui::Condition::Always)
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

                    let content_x_offset = AVATAR_SIZE[0] + 12.0;

                    for (idx, msg) in messages.iter().enumerate() {
                        let starts_group = msg.referenced_message.is_some()
                            || match messages.get(idx.wrapping_sub(1)) {
                                Some(prev) if idx > 0 => {
                                    prev.author.id != msg.author.id || !same_group_window(&prev.timestamp, &msg.timestamp)
                                }
                                _ => true,
                            };

                        let row_start = ui.cursor_pos();
                        let row_screen_start = ui.cursor_screen_pos();
                        let mut content_top = row_start[1];

                        if let Some(replied) = &msg.referenced_message {
                            let replied_author =
                                replied.author.global_name.clone().unwrap_or_else(|| replied.author.username.clone());
                            let preview: String = replied.content.chars().take(80).collect();
                            let preview = if replied.content.chars().count() > 80 { format!("{preview}...") } else { preview };
                            ui.set_cursor_pos([row_start[0] + content_x_offset, row_start[1]]);
                            let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                            ui.set_window_font_scale(0.85);
                            let label = if preview.is_empty() {
                                format!("\u{2514} Replying to {replied_author}")
                            } else {
                                format!("\u{2514} Replying to {replied_author}: {preview}")
                            };
                            ui.text(label);
                            ui.set_window_font_scale(1.0);
                            token.pop();
                            content_top = ui.cursor_pos()[1];
                        }

                        if starts_group {
                            let author = msg.author.global_name.clone().unwrap_or_else(|| msg.author.username.clone());
                            let avatar_url = rustcord_client::cdn::avatar_url(&msg.author, 64)
                                .unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&msg.author));

                            ui.set_cursor_pos([row_start[0], content_top]);
                            let avatar_clicked = if let Some((texture_id, _)) = textures.get(&avatar_url) {
                                draw_circular_image_button(ui, &format!("avatar_{}", msg.id), texture_id, AVATAR_SIZE)
                            } else {
                                textures.request(&avatar_url, cmd_tx);
                                ui.dummy(AVATAR_SIZE);
                                false
                            };
                            if avatar_clicked {
                                state.profile_popout = Some(msg.author.clone());
                            }

                            ui.set_cursor_pos([row_start[0] + content_x_offset, content_top]);
                            ui.group(|| {
                                if clickable_text(ui, &format!("name_{}", msg.id), &author, state.theme.accent) {
                                    state.profile_popout = Some(msg.author.clone());
                                }
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
                                    draw_attachment(ui, cmd_tx, textures, attachment, &mut state.image_lightbox);
                                }
                            });

                            let group_height = AVATAR_SIZE[1].max(ui.item_rect_size()[1]);
                            ui.set_cursor_pos([row_start[0], content_top + group_height]);
                        } else {
                            ui.set_cursor_pos([row_start[0] + content_x_offset, content_top]);
                            ui.group(|| {
                                if !msg.content.is_empty() {
                                    draw_markdown(ui, fonts, &state.theme, &msg.content, &mut state.revealed_spoilers, &msg.id);
                                }
                                for attachment in &msg.attachments {
                                    draw_attachment(ui, cmd_tx, textures, attachment, &mut state.image_lightbox);
                                }
                            });
                            ui.set_cursor_pos([row_start[0], content_top + ui.item_rect_size()[1]]);
                        }

                        let row_end_screen = ui.cursor_screen_pos();
                        let row_width = ui.window_content_region_max()[0] - row_start[0];
                        let abs_min = row_screen_start;
                        let abs_max = [row_screen_start[0] + row_width, row_end_screen[1]];
                        let mouse_pos = ui.io().mouse_pos;
                        let mouse_in_row = mouse_pos[0] >= abs_min[0]
                            && mouse_pos[0] <= abs_max[0]
                            && mouse_pos[1] >= abs_min[1]
                            && mouse_pos[1] <= abs_max[1];
                        if mouse_in_row && ui.is_mouse_clicked(imgui::MouseButton::Right) {
                            ui.open_popup(format!("msg_ctx_menu_{}", msg.id));
                        }
                        ui.popup(format!("msg_ctx_menu_{}", msg.id), || {
                            if ui.menu_item("Reply") {
                                state.reply_target = Some(msg.clone());
                            }
                        });

                        let next_starts_group = match messages.get(idx + 1) {
                            Some(next) => {
                                next.referenced_message.is_some()
                                    || next.author.id != msg.author.id
                                    || !same_group_window(&msg.timestamp, &next.timestamp)
                            }
                            None => true,
                        };
                        ui.dummy([0.0, if next_starts_group { MESSAGE_GAP } else { 2.0 }]);
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

            if let Some(reply_msg) = state.reply_target.clone() {
                let reply_author = reply_msg.author.global_name.clone().unwrap_or_else(|| reply_msg.author.username.clone());
                let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                ui.text(format!("Replying to {reply_author}"));
                token.pop();
                ui.same_line();
                if ui.small_button("x##cancel_reply") {
                    state.reply_target = None;
                }
            }

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

            let compose_focused = ui.is_item_focused();
            if compose_focused && ui.io().key_ctrl && ui.is_key_pressed(imgui::Key::V) {
                if let Some(channel_id) = state.selected_channel.clone() {
                    if let Some((filename, bytes)) = crate::clipboard::read_image_as_png() {
                        let content = std::mem::take(&mut state.compose_buf);
                        let reply_to = state.reply_target.take().map(|m| m.id);
                        let _ = cmd_tx.send(UiCommand::SendAttachmentBytes {
                            channel_id,
                            content,
                            filename,
                            bytes,
                            content_type: "image/png".to_string(),
                            reply_to,
                        });
                    }
                }
            }

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
                    let reply_to = state.reply_target.take().map(|m| m.id);
                    if let Some(path) = state.pending_attachment.take() {
                        let _ = cmd_tx.send(UiCommand::SendAttachment { channel_id, content, path, reply_to });
                    } else if !content.trim().is_empty() {
                        let _ = cmd_tx.send(UiCommand::SendMessage { channel_id, content, reply_to });
                    }
                }
            }
        });

    draw_image_lightbox(ui, state, textures);
}

fn draw_user_bar(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(user) = state.current_user.clone() else { return };
    let avatar_url =
        rustcord_client::cdn::avatar_url(&user, 32).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
    let avatar_screen_pos = ui.cursor_screen_pos();
    if let Some((texture_id, _)) = textures.get(&avatar_url) {
        draw_circular_image(ui, texture_id, [28.0, 28.0]);
    } else {
        textures.request(&avatar_url, cmd_tx);
        ui.dummy([28.0, 28.0]);
    }
    draw_status_dot(
        ui,
        [avatar_screen_pos[0] + 25.0, avatar_screen_pos[1] + 25.0],
        5.0,
        state.presence_for(&user.id),
        state.theme.background,
    );
    ui.same_line();

    let display_name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
    ui.text(&display_name);
    ui.same_line();

    if ui.small_button("Settings") {
        state.show_settings = !state.show_settings;
    }
}

fn draw_image_lightbox(ui: &Ui, state: &mut AppState, textures: &mut TextureCache) {
    let Some(url) = state.image_lightbox.clone() else { return };
    let Some((texture_id, natural_size)) = textures.get(&url) else { return };

    let [display_w, display_h] = ui.io().display_size;
    let max_w = display_w * 0.85;
    let max_h = display_h * 0.85;
    let [nw, nh] = natural_size;
    let scale = if nw > 0.0 && nh > 0.0 { (max_w / nw).min(max_h / nh).min(1.0) } else { 1.0 };
    let size = [nw * scale, nh * scale];

    ui.window("##lightbox_backdrop")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([display_w, display_h], imgui::Condition::Always)
        .no_decoration()
        .no_inputs()
        .draw_background(true)
        .bg_alpha(0.85)
        .build(|| {});

    let close_clicked = ui
        .window("##lightbox")
        .position([display_w / 2.0 - size[0] / 2.0, display_h / 2.0 - size[1] / 2.0], imgui::Condition::Always)
        .size(size, imgui::Condition::Always)
        .no_decoration()
        .draw_background(false)
        .build(|| {
            imgui::Image::new(texture_id, size).build(ui);
            ui.is_item_clicked()
        })
        .unwrap_or(false);

    if close_clicked || ui.is_key_pressed(imgui::Key::Escape) {
        state.image_lightbox = None;
    }
}

fn draw_profile_popout(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(user) = state.profile_popout.clone() else { return };
    let [display_w, display_h] = ui.io().display_size;
    let mut open = true;
    let mut message_clicked = false;
    ui.window(format!("Profile##{}", user.id))
        .opened(&mut open)
        .position([display_w / 2.0 - 160.0, display_h / 2.0 - 140.0], imgui::Condition::Appearing)
        .size([320.0, 280.0], imgui::Condition::Appearing)
        .build(|| {
            let avatar_url =
                rustcord_client::cdn::avatar_url(&user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
            let avatar_screen_pos = ui.cursor_screen_pos();
            if let Some((texture_id, _)) = textures.get(&avatar_url) {
                draw_circular_image(ui, texture_id, [80.0, 80.0]);
            } else {
                textures.request(&avatar_url, cmd_tx);
                ui.dummy([80.0, 80.0]);
            }
            draw_status_dot(
                ui,
                [avatar_screen_pos[0] + 70.0, avatar_screen_pos[1] + 70.0],
                8.0,
                state.presence_for(&user.id),
                state.theme.panel,
            );

            let display_name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
            ui.text(&display_name);
            if let Some(platform) = state.platform_for(&user.id) {
                ui.same_line();
                ui.text_disabled(format!("({platform})"));
            }
            ui.text_disabled(format!("@{}", user.username));
            ui.separator();
            ui.text(format!("User ID: {}", user.id));
            if user.bot {
                ui.text_disabled("BOT");
            }
            ui.spacing();
            if ui.button("Message") {
                let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: user.id.clone() });
                message_clicked = true;
            }
        });
    if !open || message_clicked {
        state.profile_popout = None;
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
