use crate::app_state::{AppState, GuildFolder, RailItem};
use crate::backend::UiCommand;
use crate::images::TextureCache;
use crate::markdown::{self, Block, Span, Style};
use imgui::Ui;
use std::collections::HashMap;
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

#[allow(clippy::too_many_arguments)]
fn draw_markdown(
    ui: &Ui,
    fonts: &Fonts,
    theme: &crate::theme::Theme,
    content: &str,
    revealed_spoilers: &mut std::collections::HashSet<usize>,
    id_seed: &str,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    known_users: &HashMap<String, rustcord_client::models::User>,
    profiles_requested: &mut std::collections::HashSet<String>,
) {
    let blocks = markdown::parse(content);
    for (block_idx, block) in blocks.iter().enumerate() {
        match block {
            Block::Paragraph(spans) => draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, known_users, profiles_requested),
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
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, known_users, profiles_requested);
                ui.set_window_font_scale(1.0);
                token.pop();
            }
            Block::Blockquote(spans) => {
                let token = ui.push_style_color(imgui::StyleColor::Text, theme.text_muted);
                ui.text("| ");
                ui.same_line();
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, known_users, profiles_requested);
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

#[allow(clippy::too_many_arguments)]
fn draw_spans(
    ui: &Ui,
    fonts: &Fonts,
    theme: &crate::theme::Theme,
    spans: &[Span],
    revealed_spoilers: &mut std::collections::HashSet<usize>,
    id_seed: &str,
    block_idx: usize,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    known_users: &HashMap<String, rustcord_client::models::User>,
    profiles_requested: &mut std::collections::HashSet<String>,
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
            Span::CustomEmoji { name, id, animated } => {
                let ext = if *animated { "gif" } else { "png" };
                let url = format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=32");
                if let Some((texture_id, size)) = textures.get(&url) {
                    let scale = (EMOJI_SIZE / size[0].max(1.0)).min(1.0);
                    imgui::Image::new(texture_id, [size[0] * scale, size[1] * scale]).build(ui);
                } else {
                    textures.request(&url, cmd_tx);
                    ui.dummy([EMOJI_SIZE, EMOJI_SIZE]);
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!(":{name}:"));
                }
            }
            Span::UnicodeEmoji { ch } => {
                if !draw_unicode_emoji(ui, textures, *ch, [EMOJI_SIZE, EMOJI_SIZE]) {
                    ui.dummy([EMOJI_SIZE, EMOJI_SIZE]);
                }
            }
            Span::Mention { user_id } => {
                let name = known_users.get(user_id).map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()));
                if name.is_none() && profiles_requested.insert(user_id.clone()) {
                    let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: user_id.clone() });
                }
                let label = format!("@{}", name.as_deref().unwrap_or(user_id));
                let bg = ui.push_style_color(imgui::StyleColor::Button, [theme.accent[0], theme.accent[1], theme.accent[2], 0.25]);
                let text = ui.push_style_color(imgui::StyleColor::Text, theme.accent);
                let font = ui.push_font(fonts.bold);
                ui.small_button(&label);
                font.pop();
                text.pop();
                bg.pop();
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!("@{}", known_users.get(user_id).map(|u| u.username.as_str()).unwrap_or(user_id)));
                }
            }
        }
    }
}

const EMOJI_SIZE: f32 = 22.0;

enum EmojiTexture {
    Ready(imgui::TextureId),
    Loading,
    Failed,
}

fn emoji_texture(textures: &TextureCache, ch: char) -> EmojiTexture {
    let url = crate::emoji::url(ch);
    match textures.get_still(&url) {
        Some((id, _)) => EmojiTexture::Ready(id),
        None if textures.is_failed(&url) => EmojiTexture::Failed,
        None => {
            textures.want(&url);
            EmojiTexture::Loading
        }
    }
}

fn draw_unicode_emoji(ui: &Ui, textures: &TextureCache, ch: char, display_size: [f32; 2]) -> bool {
    match emoji_texture(textures, ch) {
        EmojiTexture::Ready(id) => {
            imgui::Image::new(id, display_size).build(ui);
            true
        }
        EmojiTexture::Loading => {
            ui.dummy(display_size);
            true
        }
        EmojiTexture::Failed => false,
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
const USER_BAR_HEIGHT: f32 = 44.0;
const VOICE_STATUS_BAR_HEIGHT: f32 = 92.0;
const HEADER_HEIGHT: f32 = 48.0;
const SIDEBAR_MIN_WIDTH: f32 = 160.0;
const SIDEBAR_MAX_WIDTH: f32 = 480.0;
const SPLITTER_WIDTH: f32 = 6.0;
const MEMBERS_PANEL_WIDTH: f32 = 240.0;

fn draw_rail_circle(ui: &Ui, color: [f32; 4], size: [f32; 2]) {
    let p_min = ui.cursor_screen_pos();
    let p_max = [p_min[0] + size[0], p_min[1] + size[1]];
    let center = [(p_min[0] + p_max[0]) / 2.0, (p_min[1] + p_max[1]) / 2.0];
    ui.get_window_draw_list()
        .add_circle(center, size[0].min(size[1]) / 2.0, color)
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
    right_clicked: bool,
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
    activity: Option<&str>,
    row_bg_color: [f32; 4],
) -> UserRowResult {
    let row_start = ui.cursor_pos();
    let row_height = USER_ROW_AVATAR_SIZE[1] + 8.0;

    let row_clicked = ui
        .selectable_config(format!("##row_{}", user.id))
        .selected(selected)
        .size([0.0, row_height])
        .build();
    let right_clicked = ui.is_item_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Right);

    ui.set_cursor_pos([row_start[0] + 4.0, row_start[1] + 4.0]);
    let avatar_screen_pos = ui.cursor_screen_pos();
    let avatar_url = rustcord_client::cdn::avatar_url(user, 32).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
    let avatar_clicked = if let Some((texture_id, _)) = textures.get_still(&avatar_url) {
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
    let text_y = if activity.is_some() {
        row_start[1] + 3.0
    } else {
        row_start[1] + row_height / 2.0 - ui.text_line_height() / 2.0
    };
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

    if let Some(activity) = activity {
        ui.set_cursor_pos([text_x, text_y + ui.text_line_height()]);
        let token = ui.push_style_color(imgui::StyleColor::Text, [0.6, 0.62, 0.66, 1.0]);
        ui.set_window_font_scale(0.8);
        ui.text(activity);
        ui.set_window_font_scale(1.0);
        token.pop();
    }

    ui.set_cursor_pos([row_start[0], row_start[1] + row_height + ui.clone_style().item_spacing[1]]);
    UserRowResult { row_clicked, avatar_clicked: avatar_clicked || name_clicked, right_clicked }
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

const VOICE_ACTIVE_COLOR: [f32; 4] = [0.23, 0.79, 0.35, 1.0];

fn draw_voice_active_dot(ui: &Ui, icon_min: [f32; 2], size: [f32; 2]) {
    let radius = 7.0;
    let center = [icon_min[0] + size[0] - radius + 1.0, icon_min[1] + size[1] - radius + 1.0];
    let draw_list = ui.get_window_draw_list();
    draw_list.add_circle(center, radius + 2.0, [0.0, 0.0, 0.0, 0.0]).filled(true).build();
    draw_list.add_circle(center, radius + 2.0, [0.13, 0.14, 0.17, 1.0]).filled(true).build();
    draw_list.add_circle(center, radius, VOICE_ACTIVE_COLOR).filled(true).build();
}

fn draw_unread_badge(ui: &Ui, icon_min: [f32; 2]) {
    let radius = 6.0;
    let center = [icon_min[0] + radius - 1.0, icon_min[1] + radius - 1.0];
    let draw_list = ui.get_window_draw_list();
    draw_list.add_circle(center, radius + 2.0, [0.13, 0.14, 0.17, 1.0]).filled(true).build();
    draw_list.add_circle(center, radius, [1.0, 1.0, 1.0, 0.95]).filled(true).build();
}

fn draw_unread_dot(ui: &Ui, row_screen_pos: [f32; 2]) {
    let center = [row_screen_pos[0] - 8.0, row_screen_pos[1] + ui.text_line_height() / 2.0];
    ui.get_window_draw_list().add_circle(center, 3.0, [1.0, 1.0, 1.0, 0.9]).filled(true).build();
}

fn draw_guild_icon(ui: &Ui, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, guild: &rustcord_client::models::Guild, size: [f32; 2]) -> bool {
    let icon_url = rustcord_client::cdn::guild_icon_url(guild, 64);
    if let Some(url) = &icon_url {
        if let Some((texture_id, _)) = textures.get_still(url) {
            draw_circular_image_button(ui, &format!("guild_icon_{}", guild.id), texture_id, size)
        } else {
            textures.request(url, cmd_tx);
            let label = guild.name.chars().take(2).collect::<String>();
            draw_rail_circle(ui, [0.23, 0.24, 0.27, 1.0], size);
            draw_circular_image_button_placeholder(ui, &format!("guild_{}", guild.id), &label, size)
        }
    } else {
        let label = guild.name.chars().take(2).collect::<String>();
        draw_rail_circle(ui, [0.23, 0.24, 0.27, 1.0], size);
        draw_circular_image_button_placeholder(ui, &format!("guild_{}", guild.id), &label, size)
    }
}

const RAIL_DRAG_GUILD: &str = "RAIL_DRAG_GUILD";

fn draw_folder_icon(ui: &Ui, state: &AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, folder: &GuildFolder, folder_idx: usize, size: [f32; 2]) -> bool {
    let p_min = ui.cursor_screen_pos();
    let p_max = [p_min[0] + size[0], p_min[1] + size[1]];
    let draw_list = ui.get_window_draw_list();
    draw_list.add_rect(p_min, p_max, folder.color).filled(true).rounding(size[0] * 0.3).build();

    let guilds: Vec<&rustcord_client::models::Guild> = folder.guild_ids.iter().filter_map(|id| state.guilds.iter().find(|g| &g.id == id)).collect();
    let pad = 6.0;
    let cell = (size[0] - pad * 3.0) / 2.0;
    for (i, guild) in guilds.iter().take(4).enumerate() {
        let col = (i % 2) as f32;
        let row = (i / 2) as f32;
        let cell_min = [p_min[0] + pad + col * (cell + pad), p_min[1] + pad + row * (cell + pad)];
        let cell_max = [cell_min[0] + cell, cell_min[1] + cell];
        let icon_url = rustcord_client::cdn::guild_icon_url(guild, 32);
        if let Some(url) = &icon_url {
            if let Some((texture_id, _)) = textures.get_still(url) {
                draw_list.add_image_rounded(texture_id, cell_min, cell_max, cell * 0.2).build();
                continue;
            }
            textures.request(url, cmd_tx);
        }
        draw_list.add_rect(cell_min, cell_max, [1.0, 1.0, 1.0, 0.25]).filled(true).rounding(cell * 0.2).build();
        if icon_url.is_none() {
            let label = guild.name.chars().take(1).collect::<String>();
            let text_size = ui.calc_text_size(&label);
            let center = [(cell_min[0] + cell_max[0]) / 2.0, (cell_min[1] + cell_max[1]) / 2.0];
            draw_list.add_text([center[0] - text_size[0] / 2.0, center[1] - text_size[1] / 2.0], [1.0, 1.0, 1.0, 0.9], &label);
        }
    }

    ui.set_cursor_screen_pos(p_min);
    let clicked = ui.invisible_button(format!("folder_toggle_{}", folder_idx), size);
    if ui.is_item_hovered() {
        draw_list
            .add_circle([(p_min[0] + p_max[0]) / 2.0, (p_min[1] + p_max[1]) / 2.0], size[0].min(size[1]) / 2.0 + 2.0, [1.0, 1.0, 1.0, 0.5])
            .thickness(2.0)
            .build();
    }
    clicked
}

const FOLDER_ITEM_GAP: f32 = 10.0;
const FOLDER_PILL_PAD: f32 = 10.0;
const FOLDER_ITEM_SIZE: [f32; 2] = [RAIL_ICON_SIZE[0] * 0.82, RAIL_ICON_SIZE[1] * 0.82];

#[allow(clippy::too_many_arguments)]
fn draw_expanded_folder(
    ui: &Ui,
    state: &AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    folder: &GuildFolder,
    folder_idx: usize,
    rail_center_x: f32,
    toggle_folder: &mut Option<usize>,
    select_guild: &mut Option<String>,
    remove_from_folder: &mut Option<(usize, String)>,
    start_rename_folder: &mut Option<usize>,
    drop_action: &mut Option<RailDropAction>,
) {
    let item_spacing_y = ui.clone_style().item_spacing[1];
    let header_step = RAIL_ICON_SIZE[1] + item_spacing_y + FOLDER_ITEM_GAP;
    let member_step = FOLDER_ITEM_SIZE[1] + item_spacing_y + FOLDER_ITEM_GAP;

    let member_count = folder.guild_ids.len() as f32;
    let pill_width = RAIL_ICON_SIZE[0] + FOLDER_PILL_PAD * 2.0;
    let pill_height = FOLDER_PILL_PAD * 2.0 + header_step + member_step * member_count;
    let pill_left_x = rail_center_x - (pill_width - RAIL_ICON_SIZE[0]) / 2.0;
    let pill_top_y = ui.cursor_pos()[1];
    ui.set_cursor_pos([pill_left_x, pill_top_y]);
    let pill_screen = ui.cursor_screen_pos();
    let pill_max_screen = [pill_screen[0] + pill_width, pill_screen[1] + pill_height];
    let pill_bg = [folder.color[0] * 0.35, folder.color[1] * 0.35, folder.color[2] * 0.35, 1.0];
    let draw_list = ui.get_window_draw_list();
    let rounding = pill_width * 0.22;
    draw_list.add_rect(pill_screen, pill_max_screen, pill_bg).filled(true).rounding(rounding).build();
    draw_list.add_rect(pill_screen, pill_max_screen, folder.color).rounding(rounding).thickness(1.5).build();
    drop(draw_list);

    ui.set_cursor_pos([rail_center_x, pill_top_y + FOLDER_PILL_PAD]);
    let icon_min = ui.cursor_screen_pos();
    let header_clicked = draw_folder_icon(ui, state, textures, cmd_tx, folder, folder_idx, RAIL_ICON_SIZE);
    if state.folder_has_active_voice(folder) {
        draw_voice_active_dot(ui, icon_min, RAIL_ICON_SIZE);
    }
    if state.folder_has_unread(folder) {
        draw_unread_badge(ui, icon_min);
    }
    if header_clicked {
        *toggle_folder = Some(folder_idx);
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(&folder.name);
        if ui.is_mouse_clicked(imgui::MouseButton::Right) {
            *start_rename_folder = Some(folder_idx);
        }
    }
    if drop_action.is_none() {
        if let Some(target) = ui.drag_drop_target() {
            if let Some(Ok(payload)) = target.accept_payload::<[u8; 32], _>(RAIL_DRAG_GUILD, imgui::DragDropFlags::empty()) {
                if let Some(DraggedRailItem::Guild(dragged_id)) = payload_to_rail_item(&payload.data) {
                    *drop_action = Some(RailDropAction::IntoFolder(dragged_id, folder_idx));
                }
            }
            target.pop();
        }
    }

    let item_center_x = rail_center_x + (RAIL_ICON_SIZE[0] - FOLDER_ITEM_SIZE[0]) / 2.0;
    for (i, guild_id) in folder.guild_ids.iter().enumerate() {
        let Some(guild) = state.guilds.iter().find(|g| &g.id == guild_id).cloned() else { continue };
        let item_y = pill_top_y + FOLDER_PILL_PAD + header_step + member_step * i as f32;
        ui.set_cursor_pos([item_center_x, item_y]);
        let icon_min = ui.cursor_screen_pos();
        let clicked = draw_guild_icon(ui, textures, cmd_tx, &guild, FOLDER_ITEM_SIZE);
        if state.guild_has_active_voice(guild_id) {
            draw_voice_active_dot(ui, icon_min, FOLDER_ITEM_SIZE);
        }
        if state.guild_has_unread(guild_id) {
            draw_unread_badge(ui, icon_min);
        }
        if clicked {
            *select_guild = Some(guild_id.clone());
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(&guild.name);
            if ui.is_mouse_clicked(imgui::MouseButton::Right) {
                *remove_from_folder = Some((folder_idx, guild_id.clone()));
            }
        }
        if let Some(_tooltip) = ui.drag_drop_source_config(RAIL_DRAG_GUILD).begin_payload(rail_item_payload(&DraggedRailItem::Guild(guild_id.clone()))) {
            ui.text(&guild.name);
        }
    }

    ui.set_cursor_pos([rail_center_x, pill_top_y + pill_height + FOLDER_ITEM_GAP]);
}

enum RailDropAction {
    MoveGuildToIndex(String, usize),
    MoveFolderToIndex(usize, usize),
    IntoFolder(String, usize),
    CreateFolder(String, String),
}

fn extract_invite_code(input: &str) -> &str {
    let trimmed = input.trim();
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn draw_add_server_popup(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    ui.text("Join a Server");
    ui.input_text("Invite link or code", &mut state.join_invite_input).build();
    if ui.button("Join##join_server") {
        let code = extract_invite_code(&state.join_invite_input).to_string();
        if !code.is_empty() {
            let _ = cmd_tx.send(UiCommand::JoinGuildByInvite { invite_code: code });
            state.join_invite_input.clear();
            ui.close_current_popup();
        }
    }

    ui.spacing();
    ui.separator();
    ui.spacing();

    ui.text("Create a Server");
    ui.input_text("Server name", &mut state.create_server_input).build();
    if ui.button("Create##create_server") {
        let name = state.create_server_input.trim().to_string();
        if !name.is_empty() {
            let _ = cmd_tx.send(UiCommand::CreateGuild { name });
            state.create_server_input.clear();
            ui.close_current_popup();
        }
    }
}

fn draw_rail_items(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache, rail_center_x: f32) {
    let mut drop_action: Option<RailDropAction> = None;
    let mut toggle_folder: Option<usize> = None;
    let mut remove_from_folder: Option<(usize, String)> = None;
    let mut select_guild: Option<String> = None;
    let mut start_rename_folder: Option<usize> = None;

    let order_len = state.rail_order.len();
    for index in 0..order_len {
        let item = state.rail_order[index].clone();
        match item {
            RailItem::Guild(guild_id) => {
                let Some(guild) = state.guilds.iter().find(|g| g.id == guild_id).cloned() else { continue };
                let icon_min = ui.cursor_screen_pos();
                let clicked = draw_guild_icon(ui, textures, cmd_tx, &guild, RAIL_ICON_SIZE);
                if state.guild_has_active_voice(&guild.id) {
                    draw_voice_active_dot(ui, icon_min, RAIL_ICON_SIZE);
                }
                if state.guild_has_unread(&guild.id) {
                    draw_unread_badge(ui, icon_min);
                }
                if clicked {
                    select_guild = Some(guild.id.clone());
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(&guild.name);
                }

                if let Some(_tooltip) = ui.drag_drop_source_config(RAIL_DRAG_GUILD).begin_payload(rail_item_payload(&DraggedRailItem::Guild(guild.id.clone()))) {
                    ui.text(&guild.name);
                }
                if drop_action.is_none() {
                    if let Some(target) = ui.drag_drop_target() {
                        if let Some(Ok(payload)) = target.accept_payload::<[u8; 32], _>(RAIL_DRAG_GUILD, imgui::DragDropFlags::empty()) {
                            match payload_to_rail_item(&payload.data) {
                                Some(DraggedRailItem::Guild(dragged_id)) if dragged_id != guild.id => {
                                    drop_action = Some(RailDropAction::CreateFolder(dragged_id, guild.id.clone()));
                                }
                                Some(DraggedRailItem::Folder(dragged_idx)) => {
                                    drop_action = Some(RailDropAction::MoveFolderToIndex(dragged_idx, index));
                                }
                                _ => {}
                            }
                        }
                        target.pop();
                    }
                }
            }
            RailItem::Folder(folder_idx) => {
                let Some(folder) = state.rail_folders.get(folder_idx).cloned() else { continue };

                if folder.expanded {
                    draw_expanded_folder(
                        ui,
                        state,
                        textures,
                        cmd_tx,
                        &folder,
                        folder_idx,
                        rail_center_x,
                        &mut toggle_folder,
                        &mut select_guild,
                        &mut remove_from_folder,
                        &mut start_rename_folder,
                        &mut drop_action,
                    );
                } else {
                    let icon_min = ui.cursor_screen_pos();
                    let clicked = draw_folder_icon(ui, state, textures, cmd_tx, &folder, folder_idx, RAIL_ICON_SIZE);
                    if state.folder_has_active_voice(&folder) {
                        draw_voice_active_dot(ui, icon_min, RAIL_ICON_SIZE);
                    }
                    if state.folder_has_unread(&folder) {
                        draw_unread_badge(ui, icon_min);
                    }
                    if clicked {
                        toggle_folder = Some(folder_idx);
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text(&folder.name);
                        if ui.is_mouse_clicked(imgui::MouseButton::Right) {
                            start_rename_folder = Some(folder_idx);
                        }
                    }
                    if let Some(_tooltip) = ui.drag_drop_source_config(RAIL_DRAG_GUILD).begin_payload(rail_item_payload(&DraggedRailItem::Folder(folder_idx))) {
                        ui.text(&folder.name);
                    }
                    if drop_action.is_none() {
                        if let Some(target) = ui.drag_drop_target() {
                            if let Some(Ok(payload)) = target.accept_payload::<[u8; 32], _>(RAIL_DRAG_GUILD, imgui::DragDropFlags::empty()) {
                                match payload_to_rail_item(&payload.data) {
                                    Some(DraggedRailItem::Guild(dragged_id)) => {
                                        drop_action = Some(RailDropAction::IntoFolder(dragged_id, folder_idx));
                                    }
                                    Some(DraggedRailItem::Folder(dragged_idx)) if dragged_idx != folder_idx => {
                                        drop_action = Some(RailDropAction::MoveFolderToIndex(dragged_idx, index));
                                    }
                                    _ => {}
                                }
                            }
                            target.pop();
                        }
                    }
                }

                if state.renaming_folder == Some(folder_idx) {
                    ui.open_popup(format!("rename_folder_{}", folder_idx));
                }
                ui.popup(format!("rename_folder_{}", folder_idx), || {
                    ui.text("Rename folder");
                    ui.input_text("##folder_name", &mut state.folder_rename_buf).build();
                    if ui.button("Save") || ui.is_key_pressed(imgui::Key::Enter) {
                        if let Some(f) = state.rail_folders.get_mut(folder_idx) {
                            f.name = state.folder_rename_buf.trim().to_string();
                            if f.name.is_empty() {
                                f.name = "Folder".to_string();
                            }
                        }
                        state.save_rail();
                        state.renaming_folder = None;
                        ui.close_current_popup();
                    }
                });
            }
        }
        ui.set_cursor_pos([0.0, ui.cursor_pos()[1]]);
        let gap_width = ui.content_region_avail()[0];
        let gap_min = ui.cursor_screen_pos();
        ui.invisible_button(&format!("rail_gap_{}", index), [gap_width, 8.0]);
        if ui.is_item_hovered() {
            ui.get_window_draw_list()
                .add_line([gap_min[0] + 8.0, gap_min[1] + 4.0], [gap_min[0] + gap_width - 8.0, gap_min[1] + 4.0], state.theme.accent)
                .thickness(2.0)
                .build();
        }
        if drop_action.is_none() {
            if let Some(target) = ui.drag_drop_target() {
                if let Some(Ok(payload)) = target.accept_payload::<[u8; 32], _>(RAIL_DRAG_GUILD, imgui::DragDropFlags::empty()) {
                    drop_action = match payload_to_rail_item(&payload.data) {
                        Some(DraggedRailItem::Guild(dragged_id)) => Some(RailDropAction::MoveGuildToIndex(dragged_id, index + 1)),
                        Some(DraggedRailItem::Folder(dragged_idx)) => Some(RailDropAction::MoveFolderToIndex(dragged_idx, index + 1)),
                        None => None,
                    };
                }
                target.pop();
            }
        }
        let next_y = ui.cursor_pos()[1];
        ui.set_cursor_pos([rail_center_x, next_y]);
    }

    draw_rail_circle(ui, [0.17, 0.52, 0.27, 1.0], RAIL_ICON_SIZE);
    if draw_circular_image_button_placeholder(ui, "add_server_button", "+", RAIL_ICON_SIZE) {
        ui.open_popup("add_server_popup");
    }
    if ui.is_item_hovered() {
        ui.tooltip_text("Add a Server");
    }
    ui.popup("add_server_popup", || {
        draw_add_server_popup(ui, state, cmd_tx);
    });

    if let Some(guild_id) = select_guild {
        let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild_id.clone() });
        state.selected_guild = Some(guild_id);
        state.selected_channel = None;
    }
    match drop_action {
        Some(RailDropAction::MoveGuildToIndex(dragged_id, target_index)) => state.move_guild_to_rail_index(&dragged_id, target_index),
        Some(RailDropAction::MoveFolderToIndex(dragged_idx, target_index)) => state.move_folder_to_rail_index(dragged_idx, target_index),
        Some(RailDropAction::IntoFolder(dragged_id, folder_idx)) => state.move_guild_into_folder(&dragged_id, folder_idx),
        Some(RailDropAction::CreateFolder(dragged_id, target_id)) => state.create_folder_from_guilds(&dragged_id, &target_id),
        None => {}
    }
    if let Some(folder_idx) = toggle_folder {
        if let Some(folder) = state.rail_folders.get_mut(folder_idx) {
            folder.expanded = !folder.expanded;
            state.save_rail();
        }
    }
    if let Some((folder_idx, guild_id)) = remove_from_folder {
        state.remove_guild_from_folder(folder_idx, &guild_id);
    }
    if let Some(folder_idx) = start_rename_folder {
        state.folder_rename_buf = state.rail_folders.get(folder_idx).map(|f| f.name.clone()).unwrap_or_default();
        state.renaming_folder = Some(folder_idx);
    }
}

#[derive(Clone, Debug)]
enum DraggedRailItem {
    Guild(String),
    Folder(usize),
}

fn rail_item_payload(item: &DraggedRailItem) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let (tag, text) = match item {
        DraggedRailItem::Guild(id) => (0u8, id.clone()),
        DraggedRailItem::Folder(idx) => (1u8, idx.to_string()),
    };
    buf[0] = tag;
    let bytes = text.as_bytes();
    let len = bytes.len().min(31);
    buf[1..1 + len].copy_from_slice(&bytes[..len]);
    buf
}

fn payload_to_rail_item(payload: &[u8; 32]) -> Option<DraggedRailItem> {
    let tag = payload[0];
    let rest = &payload[1..];
    let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    let text = String::from_utf8_lossy(&rest[..end]).into_owned();
    match tag {
        0 => Some(DraggedRailItem::Guild(text)),
        1 => text.parse().ok().map(DraggedRailItem::Folder),
        _ => None,
    }
}

fn has_sprite(s: &str) -> bool {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => crate::emoji::is_emoji(ch),
        _ => false,
    }
}

static UNICODE_EMOJI: std::sync::LazyLock<Vec<&'static emojis::Emoji>> =
    std::sync::LazyLock::new(|| emojis::iter().filter(|e| has_sprite(e.as_str())).collect());

const EMOJI_CATEGORIES: &[(&str, emojis::Group)] = &[
    ("Smileys", emojis::Group::SmileysAndEmotion),
    ("People", emojis::Group::PeopleAndBody),
    ("Animals", emojis::Group::AnimalsAndNature),
    ("Food", emojis::Group::FoodAndDrink),
    ("Travel", emojis::Group::TravelAndPlaces),
    ("Activities", emojis::Group::Activities),
    ("Objects", emojis::Group::Objects),
    ("Symbols", emojis::Group::Symbols),
    ("Flags", emojis::Group::Flags),
];

const EMOJI_GRID_COLUMNS: usize = 8;
const AUTOCOMPLETE_MAX_RESULTS: usize = 8;

enum EmojiMatch<'a> {
    Unicode(&'static emojis::Emoji),
    Custom(&'a rustcord_client::models::Emoji),
}

impl EmojiMatch<'_> {
    fn shortcode(&self) -> &str {
        match self {
            EmojiMatch::Unicode(e) => e.shortcode().unwrap_or_else(|| e.name()),
            EmojiMatch::Custom(e) => &e.name,
        }
    }

    fn insert_text(&self) -> String {
        match self {
            EmojiMatch::Unicode(e) => e.as_str().to_string(),
            EmojiMatch::Custom(e) => {
                let tag = if e.animated { "a" } else { "" };
                format!("<{tag}:{}:{}>", e.name, e.id)
            }
        }
    }
}

fn trailing_mention_token(text: &str) -> Option<(usize, &str)> {
    let at_pos = text.rfind('@')?;
    let preceding_ok = at_pos == 0 || text[..at_pos].ends_with(char::is_whitespace);
    if !preceding_ok {
        return None;
    }
    let partial = &text[at_pos + 1..];
    if partial.len() > 32 || partial.contains(char::is_whitespace) {
        return None;
    }
    Some((at_pos, partial))
}

fn trailing_emoji_token(text: &str) -> Option<(usize, &str)> {
    let colon_pos = text.rfind(':')?;
    let partial = &text[colon_pos + 1..];
    if partial.is_empty() || partial.len() > 32 || partial.contains(char::is_whitespace) {
        return None;
    }
    if text[..colon_pos].ends_with(':') {
        return None;
    }
    Some((colon_pos, partial))
}

fn emoji_autocomplete_matches<'a>(state: &'a AppState, partial: &str) -> Vec<EmojiMatch<'a>> {
    let partial = partial.to_lowercase();
    let mut matches: Vec<EmojiMatch> = Vec::new();
    for (_guild, emoji) in state.available_emojis() {
        if emoji.name.to_lowercase().starts_with(&partial) {
            matches.push(EmojiMatch::Custom(emoji));
        }
    }
    for emoji in UNICODE_EMOJI.iter() {
        if let Some(shortcode) = emoji.shortcode() {
            if shortcode.starts_with(&partial) {
                matches.push(EmojiMatch::Unicode(emoji));
            }
        }
    }
    matches.truncate(AUTOCOMPLETE_MAX_RESULTS);
    matches
}

fn draw_emoji_autocomplete(
    ui: &Ui,
    state: &mut AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    bar_start: [f32; 2],
) -> Option<(usize, String)> {
    let (token_start, partial) = trailing_emoji_token(&state.compose_buf)?;
    let partial = partial.to_string();
    let match_count = emoji_autocomplete_matches(state, &partial).len();
    if match_count == 0 {
        return None;
    }
    if state.emoji_autocomplete_selected >= match_count {
        state.emoji_autocomplete_selected = 0;
    }
    if ui.is_key_pressed(imgui::Key::DownArrow) {
        state.emoji_autocomplete_selected = (state.emoji_autocomplete_selected + 1) % match_count;
    }
    if ui.is_key_pressed(imgui::Key::UpArrow) {
        state.emoji_autocomplete_selected = (state.emoji_autocomplete_selected + match_count - 1) % match_count;
    }
    let confirmed_by_key = ui.is_key_pressed(imgui::Key::Tab) || ui.is_key_pressed(imgui::Key::Enter);
    let matches = emoji_autocomplete_matches(state, &partial);

    let mut picked = None;
    let popup_pos = [bar_start[0], bar_start[1] - matches.len().min(AUTOCOMPLETE_MAX_RESULTS) as f32 * 28.0 - 8.0];
    ui.window("##emoji_autocomplete")
        .position(popup_pos, imgui::Condition::Always)
        .size([260.0, matches.len() as f32 * 28.0 + 12.0], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .no_inputs()
        .focus_on_appearing(false)
        .build(|| {
            for (idx, m) in matches.iter().enumerate() {
                let selected = idx == state.emoji_autocomplete_selected;
                let row_height = 20.0;
                let row_start = ui.cursor_pos();
                let clicked = ui.selectable_config(format!("##autocomplete_{idx}")).selected(selected).size([0.0, row_height]).build();
                ui.set_cursor_pos(row_start);
                match m {
                    EmojiMatch::Unicode(e) => {
                        let ch = e.as_str().chars().next().unwrap_or(' ');
                        if !draw_unicode_emoji(ui, textures, ch, [18.0, 18.0]) {
                            ui.dummy([18.0, 18.0]);
                        }
                        ui.same_line();
                        ui.text(format!(":{}:", m.shortcode()));
                    }
                    EmojiMatch::Custom(e) => {
                        let url = rustcord_client::cdn::custom_emoji_url(e, 32);
                        if let Some((texture_id, size)) = textures.get(&url) {
                            let scale = (18.0 / size[0].max(1.0)).min(1.0);
                            imgui::Image::new(texture_id, [size[0] * scale, size[1] * scale]).build(ui);
                        } else {
                            textures.request(&url, cmd_tx);
                            ui.dummy([18.0, 18.0]);
                        }
                        ui.same_line();
                        ui.text(format!(":{}:", e.name));
                    }
                }
                if (selected && confirmed_by_key) || clicked {
                    picked = Some(m.insert_text());
                }
            }
        });
    picked.map(|text| (token_start, text))
}

fn mention_autocomplete_matches<'a>(state: &'a AppState, partial: &str) -> Vec<&'a rustcord_client::models::User> {
    let partial = partial.to_lowercase();
    let mut matches: Vec<&rustcord_client::models::User> = state
        .mentionable_users_for_active_channel()
        .into_iter()
        .filter(|u| {
            partial.is_empty()
                || u.username.to_lowercase().starts_with(&partial)
                || u.global_name.as_deref().is_some_and(|n| n.to_lowercase().starts_with(&partial))
        })
        .collect();
    matches.truncate(AUTOCOMPLETE_MAX_RESULTS);
    matches
}

fn draw_mention_autocomplete(
    ui: &Ui,
    state: &mut AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    bar_start: [f32; 2],
) -> Option<(usize, String)> {
    let (token_start, partial) = trailing_mention_token(&state.compose_buf)?;
    let partial = partial.to_string();
    let match_count = mention_autocomplete_matches(state, &partial).len();
    if match_count == 0 {
        return None;
    }
    if state.mention_autocomplete_selected >= match_count {
        state.mention_autocomplete_selected = 0;
    }
    if ui.is_key_pressed(imgui::Key::DownArrow) {
        state.mention_autocomplete_selected = (state.mention_autocomplete_selected + 1) % match_count;
    }
    if ui.is_key_pressed(imgui::Key::UpArrow) {
        state.mention_autocomplete_selected = (state.mention_autocomplete_selected + match_count - 1) % match_count;
    }
    let confirmed_by_key = ui.is_key_pressed(imgui::Key::Tab) || ui.is_key_pressed(imgui::Key::Enter);
    let matches: Vec<(String, String)> = mention_autocomplete_matches(state, &partial)
        .into_iter()
        .map(|u| (u.id.clone(), u.global_name.clone().unwrap_or_else(|| u.username.clone())))
        .collect();

    let mut picked = None;
    let popup_pos = [bar_start[0], bar_start[1] - matches.len() as f32 * 28.0 - 8.0];
    ui.window("##mention_autocomplete")
        .position(popup_pos, imgui::Condition::Always)
        .size([260.0, matches.len() as f32 * 28.0 + 12.0], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .no_inputs()
        .focus_on_appearing(false)
        .build(|| {
            for (idx, (user_id, display_name)) in matches.iter().enumerate() {
                let selected = idx == state.mention_autocomplete_selected;
                let row_height = 20.0;
                let row_start = ui.cursor_pos();
                let clicked = ui.selectable_config(format!("##mention_autocomplete_{idx}")).selected(selected).size([0.0, row_height]).build();
                ui.set_cursor_pos(row_start);
                let avatar_url = state
                    .known_users
                    .get(user_id)
                    .map(|u| rustcord_client::cdn::avatar_url(u, 32).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(u)));
                if let Some(url) = &avatar_url {
                    if let Some((texture_id, _)) = textures.get_still(url) {
                        imgui::Image::new(texture_id, [18.0, 18.0]).build(ui);
                    } else {
                        textures.request(url, cmd_tx);
                        ui.dummy([18.0, 18.0]);
                    }
                } else {
                    ui.dummy([18.0, 18.0]);
                }
                ui.same_line();
                ui.text(format!("@{display_name}"));
                if (selected && confirmed_by_key) || clicked {
                    picked = Some(format!("<@{user_id}>"));
                }
            }
        });
    picked.map(|text| (token_start, text))
}

const EMOJI_PICKER_WIDTH: f32 = 360.0;
const EMOJI_PICKER_HEIGHT: f32 = 340.0;
const EMOJI_SIDEBAR_WIDTH: f32 = 72.0;
const CUSTOM_TAB: usize = 0;

fn draw_custom_emoji_grid(
    ui: &Ui,
    custom: &[(&rustcord_client::models::Guild, &rustcord_client::models::Emoji)],
    filter: &str,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
) -> Option<String> {
    let mut insert = None;
    let mut col = 0;
    for (_guild, emoji) in custom {
        if !filter.is_empty() && !emoji.name.to_lowercase().contains(filter) {
            continue;
        }
        let url = rustcord_client::cdn::custom_emoji_url(emoji, 32);
        if col > 0 {
            ui.same_line();
        }
        if let Some((texture_id, size)) = textures.get(&url) {
            let scale = (24.0 / size[0].max(1.0)).min(1.0);
            if ui.invisible_button(format!("##custom_emoji_{}", emoji.id), [size[0] * scale, size[1] * scale]) {
                let tag = if emoji.animated { "a" } else { "" };
                insert = Some(format!("<{tag}:{}:{}>", emoji.name, emoji.id));
            }
            let p_min = ui.item_rect_min();
            let p_max = ui.item_rect_max();
            ui.get_window_draw_list().add_image(texture_id, p_min, p_max).build();
        } else {
            textures.request(&url, cmd_tx);
            ui.dummy([24.0, 24.0]);
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(format!(":{}:", emoji.name));
        }
        col = (col + 1) % EMOJI_GRID_COLUMNS;
    }
    insert
}

fn draw_unicode_emoji_grid(ui: &Ui, textures: &TextureCache, group: Option<emojis::Group>, filter: &str) -> Option<String> {
    let mut insert = None;
    let mut col = 0;
    for emoji in UNICODE_EMOJI.iter() {
        if !filter.is_empty() {
            let shortcode = emoji.shortcode().unwrap_or_default();
            if !shortcode.contains(filter) && !emoji.name().to_lowercase().contains(filter) {
                continue;
            }
        } else if Some(emoji.group()) != group {
            continue;
        }
        if col > 0 {
            ui.same_line();
        }
        let shortcode = emoji.shortcode().unwrap_or_else(|| emoji.name());
        let ch = emoji.as_str().chars().next().unwrap_or(' ');
        let picked = if crate::emoji::is_emoji(ch) {
            match emoji_texture(textures, ch) {
                EmojiTexture::Ready(texture_id) => ui.image_button_config(format!("##emoji_{shortcode}"), texture_id, [24.0, 24.0]).build(),
                EmojiTexture::Loading => {
                    ui.dummy([24.0, 24.0]);
                    false
                }
                EmojiTexture::Failed => ui.button(format!("{}##emoji_{shortcode}", emoji.as_str())),
            }
        } else {
            ui.button(format!("{}##emoji_{shortcode}", emoji.as_str()))
        };
        if picked {
            insert = Some(emoji.as_str().to_string());
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(format!(":{shortcode}:"));
        }
        col = (col + 1) % EMOJI_GRID_COLUMNS;
    }
    insert
}

fn draw_emoji_picker(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let mut insert: Option<String> = None;
    ui.popup("emoji_picker", || {
        ui.set_next_item_width(EMOJI_PICKER_WIDTH);
        ui.input_text("##emoji_search", &mut state.emoji_picker_filter).hint("Search emoji...").build();
        let filter = state.emoji_picker_filter.to_lowercase();
        ui.separator();

        let has_custom = !state.guild_emojis.is_empty();
        if filter.is_empty() {
            if state.emoji_picker_category == CUSTOM_TAB && !has_custom {
                state.emoji_picker_category = 1;
            }
            ui.child_window("emoji_sidebar").size([EMOJI_SIDEBAR_WIDTH, EMOJI_PICKER_HEIGHT]).build(|| {
                if has_custom {
                    let selected = state.emoji_picker_category == CUSTOM_TAB;
                    let token = selected.then(|| ui.push_style_color(imgui::StyleColor::Button, state.theme.accent));
                    if ui.button_with_size("Custom", [EMOJI_SIDEBAR_WIDTH - 16.0, 0.0]) {
                        state.emoji_picker_category = CUSTOM_TAB;
                    }
                    drop(token);
                    ui.separator();
                }
                for (idx, (label, _)) in EMOJI_CATEGORIES.iter().enumerate() {
                    let tab = idx + 1;
                    let selected = state.emoji_picker_category == tab;
                    let token = selected.then(|| ui.push_style_color(imgui::StyleColor::Button, state.theme.accent));
                    if ui.button_with_size(*label, [EMOJI_SIDEBAR_WIDTH - 16.0, 0.0]) {
                        state.emoji_picker_category = tab;
                    }
                    drop(token);
                }
            });
            ui.same_line();
        }

        ui.child_window("emoji_grid")
            .size([EMOJI_PICKER_WIDTH - if filter.is_empty() { EMOJI_SIDEBAR_WIDTH + 8.0 } else { 0.0 }, EMOJI_PICKER_HEIGHT])
            .build(|| {
                let custom = state.available_emojis();
                let showing_custom_tab = filter.is_empty() && state.emoji_picker_category == CUSTOM_TAB;

                if showing_custom_tab {
                    insert = draw_custom_emoji_grid(ui, &custom, &filter, textures, cmd_tx);
                    return;
                }

                if !filter.is_empty() && !custom.is_empty() {
                    if let Some(picked) = draw_custom_emoji_grid(ui, &custom, &filter, textures, cmd_tx) {
                        insert = Some(picked);
                    }
                    if custom.iter().any(|(_, e)| e.name.to_lowercase().contains(&filter)) {
                        ui.separator();
                    }
                }

                let group = if filter.is_empty() {
                    Some(EMOJI_CATEGORIES[state.emoji_picker_category.saturating_sub(1).min(EMOJI_CATEGORIES.len() - 1)].1)
                } else {
                    None
                };
                if let Some(picked) = draw_unicode_emoji_grid(ui, textures, group, &filter) {
                    insert = Some(picked);
                }
            });
    });
    if let Some(glyph) = insert {
        state.compose_buf.push_str(&glyph);
        state.focus_compose = true;
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

fn draw_file_attachment(ui: &Ui, cmd_tx: &UnboundedSender<UiCommand>, textures: &TextureCache, attachment: &rustcord_client::models::Attachment) {
    let content_type = attachment.content_type.as_deref().unwrap_or_default();
    let ext = attachment.filename.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    let is_video = content_type.starts_with("video/") || matches!(ext.as_str(), "mp4" | "webm" | "mov" | "mkv");
    let is_audio = content_type.starts_with("audio/") || matches!(ext.as_str(), "mp3" | "ogg" | "wav" | "flac" | "m4a" | "opus");
    let (icon, open_label) = if is_video {
        ('\u{1F3AC}', "Play")
    } else if is_audio {
        ('\u{1F3B5}', "Play")
    } else if matches!(ext.as_str(), "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst") {
        ('\u{1F4E6}', "Open")
    } else if matches!(ext.as_str(), "txt" | "md" | "log" | "pdf" | "doc" | "docx") {
        ('\u{1F4C4}', "Open")
    } else {
        ('\u{1F4C1}', "Open")
    };
    ui.group(|| {
        if !draw_unicode_emoji(ui, textures, icon, [28.0, 28.0]) {
            ui.dummy([28.0, 28.0]);
        }
        ui.same_line();
        ui.group(|| {
            ui.text(&attachment.filename);
            ui.text_disabled(human_size(attachment.size));
            if ui.small_button(format!("Save##save_{}", attachment.id)) {
                let _ = cmd_tx.send(UiCommand::SaveAttachment { url: attachment.url.clone(), filename: attachment.filename.clone() });
            }
            if ui.is_item_hovered() {
                ui.tooltip_text("Download to your Downloads folder");
            }
            ui.same_line();
            if ui.small_button(format!("{open_label}##open_{}", attachment.id)) {
                let _ = open::that(&attachment.url);
            }
        });
    });
}

fn draw_attachment(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    attachment: &rustcord_client::models::Attachment,
    lightbox: &mut Option<String>,
) {
    if !is_image_attachment(attachment.content_type.as_deref(), &attachment.filename) {
        draw_file_attachment(ui, cmd_tx, textures, attachment);
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
    if let Some([left, right]) = state.theme.gradient {
        let [display_w, display_h] = ui.io().display_size;
        ui.get_background_draw_list().add_rect_filled_multicolor(
            [0.0, 0.0],
            [display_w, display_h],
            left,
            right,
            right,
            left,
        );
    }
    if !state.logged_in {
        draw_login(ui, state, cmd_tx);
        let _ = textures;
        return;
    }

    dispatch_keybinds(ui, state, cmd_tx);

    let [display_w, display_h] = ui.io().display_size;
    state.sidebar_width = state.sidebar_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
    let sidebar_width = state.sidebar_width;

    let _server_bg = ui.push_style_color(imgui::StyleColor::WindowBg, state.theme.servers_bg());
    ui.window("servers")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([RAIL_WIDTH, display_h], imgui::Condition::Always)
        .no_decoration()
        .scrollable(false)
        .draw_background(true)
        .build(|| {
            let rail_center_x = (RAIL_WIDTH - RAIL_ICON_SIZE[0]) / 2.0;

            ui.set_cursor_pos([rail_center_x, RAIL_PADDING_TOP]);
            let dm_color = if state.selected_guild.is_none() { state.theme.accent_hover } else { state.theme.accent };
            draw_rail_circle(ui, dm_color, RAIL_ICON_SIZE);
            if draw_circular_image_button_placeholder(ui, "dm_button", "DM", RAIL_ICON_SIZE) {
                state.selected_guild = None;
                state.selected_channel = None;
            }

            ui.set_cursor_pos([8.0, RAIL_PADDING_TOP + RAIL_ICON_SIZE[1] + 8.0]);
            ui.separator();
            let after_separator_y = ui.cursor_pos()[1];
            ui.set_cursor_pos([0.0, after_separator_y]);

            let _no_pad = ui.push_style_var(imgui::StyleVar::WindowPadding([0.0, 0.0]));
            let _child_bg = ui.push_style_color(imgui::StyleColor::ChildBg, state.theme.servers_bg());
            ui.child_window("guilds_scroll").size([RAIL_WIDTH, display_h - after_separator_y]).build(|| {
                ui.set_cursor_pos([rail_center_x, 0.0]);
                draw_rail_items(ui, state, cmd_tx, textures, rail_center_x);
            });
            _child_bg.pop();
            _no_pad.pop();
        });
        _server_bg.pop(); // <- Non dimenticare il pop()!

    let _channels_bg = ui.push_style_color(imgui::StyleColor::WindowBg, state.theme.panel);
    ui.window("channels")
        .position([RAIL_WIDTH, 0.0], imgui::Condition::Always)
        .size([sidebar_width, display_h], imgui::Condition::Always)
        .no_decoration()
        .scrollable(false)
        .draw_background(true)
        .build(|| {
            let voice_bar_reserved = if state.connected_voice_channel.is_some() {
                VOICE_STATUS_BAR_HEIGHT + ui.clone_style().item_spacing[1] * 2.0 + 1.0
            } else {
                0.0
            };
            ui.child_window("channels_scroll").size([0.0, display_h - USER_BAR_HEIGHT - voice_bar_reserved - 36.0]).build(|| {
            if state.selected_guild.is_some() {
                let groups: Vec<(Option<String>, Vec<(String, String, Option<String>, bool)>)> = state
                    .grouped_channels_for_selected_guild()
                    .into_iter()
                    .map(|g| {
                        let channels = g
                            .channels
                            .into_iter()
                            .map(|(c, visible)| (c.id.clone(), c.name.clone().unwrap_or_else(|| c.id.clone()), c.last_message_id.clone(), visible))
                            .collect();
                        (g.name.map(str::to_string), channels)
                    })
                    .collect();
                if groups.is_empty() {
                    ui.text_disabled("No text channels loaded yet.");
                }
                if ui.is_window_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Right) {
                    ui.open_popup("channel_list_ctx");
                }
                ui.popup("channel_list_ctx", || {
                    let label = if state.show_hidden_channels { "Hide channels you can't access" } else { "Show channels you can't access" };
                    if ui.selectable(label) {
                        state.show_hidden_channels = !state.show_hidden_channels;
                    }
                });
                for (category_name, channels) in &groups {
                    if let Some(name) = category_name {
                        ui.spacing();
                        let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                        ui.text(name.to_uppercase());
                        token.pop();
                    }
                    for (channel_id, name, last_message_id, visible) in channels {
                        let is_unread = *visible && state.channel_is_unread(channel_id, last_message_id.as_deref());
                        let row_start = ui.cursor_screen_pos();
                        let _dim = (!visible).then(|| ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted));
                        if *visible {
                            if ui.selectable(format!("# {name}")) {
                                let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
                                state.selected_channel = Some(channel_id.clone());
                                if let Some(last_id) = last_message_id {
                                    if state.mark_channel_read(channel_id, last_id) {
                                        let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: channel_id.clone(), message_id: last_id.clone() });
                                    }
                                }
                            }
                        } else {
                            ui.text(format!("# {name}"));
                            if ui.is_item_hovered() {
                                ui.tooltip_text("You don't have access to this channel");
                            }
                        }
                        if is_unread {
                            draw_unread_dot(ui, row_start);
                        }
                    }
                }

                let guild_id = state.selected_guild.clone().unwrap_or_default();
                let voice_channels: Vec<(String, String)> = state
                    .voice_channels_for_selected_guild()
                    .into_iter()
                    .map(|c| (c.id.clone(), c.name.clone().unwrap_or_else(|| c.id.clone())))
                    .collect();
                if !voice_channels.is_empty() {
                    ui.spacing();
                    let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                    ui.text("VOICE CHANNELS");
                    token.pop();
                    for (channel_id, name) in &voice_channels {
                        let is_connected = state.connected_voice_channel.as_ref() == Some(&(Some(guild_id.clone()), channel_id.clone()));
                        let label = if is_connected { format!("[voice] {name} (connected)") } else { format!("[voice] {name}") };
                        if ui.selectable(label) && !is_connected && !state.voice_connecting {
                            let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: Some(guild_id.clone()), channel_id: channel_id.clone(), ring: false });
                            state.pending_voice_channel = Some((Some(guild_id.clone()), channel_id.clone()));
                        }
                        let member_ids: Vec<String> = state.members_in_voice_channel(&guild_id, channel_id).into_iter().map(|m| m.user_id.clone()).collect();
                        for member_id in member_ids {
                            let Some(member) = state.voice_channel_states.get(&guild_id).and_then(|states| states.iter().find(|s| s.user_id == member_id)).cloned() else { continue };
                            ui.indent();
                            let name = state.voice_state_display_name(&member);
                            let unresolved = name == member.user_id;
                            let speaking = state.voice_speaking_users.contains(&member.user_id);
                            let user = state.known_users.get(&member.user_id).cloned().or_else(|| member.member.as_ref().and_then(|m| m.user.clone()));
                            const ROW_H: f32 = 30.0;
                            const AVATAR: f32 = 24.0;
                            let row_pos = ui.cursor_screen_pos();
                            let row_width = ui.content_region_avail()[0];
                            ui.selectable_config(format!("##vc_member_{member_id}")).size([row_width, ROW_H]).build();
                            if ui.is_item_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Right) {
                                state.context_vc_member = Some(member_id.clone());
                                ui.open_popup("vc_member_context");
                            }
                            {
                                let dl = ui.get_window_draw_list();
                                let center = [row_pos[0] + 4.0 + AVATAR / 2.0, row_pos[1] + ROW_H / 2.0];
                                let (p_min, p_max) = ([center[0] - AVATAR / 2.0, center[1] - AVATAR / 2.0], [center[0] + AVATAR / 2.0, center[1] + AVATAR / 2.0]);
                                let avatar_url = user.as_ref().map(|u| rustcord_client::cdn::avatar_url(u, 64).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(u)));
                                match avatar_url.as_deref().and_then(|url| textures.get_still(url)) {
                                    Some((texture_id, _)) => dl.add_image_rounded(texture_id, p_min, p_max, AVATAR / 2.0).build(),
                                    None => {
                                        if let Some(url) = &avatar_url {
                                            textures.request(url, cmd_tx);
                                        }
                                        dl.add_circle(center, AVATAR / 2.0, state.theme.text_muted).filled(true).num_segments(24).build();
                                    }
                                }
                                if speaking {
                                    dl.add_circle(center, AVATAR / 2.0 + 2.0, [0.25, 0.8, 0.4, 1.0]).thickness(2.0).num_segments(32).build();
                                }
                                let text_color = ui.style_color(imgui::StyleColor::Text);
                                dl.add_text([center[0] + AVATAR / 2.0 + 8.0, row_pos[1] + (ROW_H - ui.text_line_height()) / 2.0], text_color, &name);
                                let status_icon = if member.self_deaf || member.deaf { Some('\u{1F515}') } else if member.self_mute || member.mute { Some('\u{1F507}') } else { None };
                                if let Some(EmojiTexture::Ready(tex)) = status_icon.map(|ch| emoji_texture(textures, ch)) {
                                    let x = row_pos[0] + row_width - 22.0;
                                    dl.add_image(tex, [x, center[1] - 8.0], [x + 16.0, center[1] + 8.0]).build();
                                }
                            }
                            ui.unindent();
                            if unresolved && state.profiles_requested.insert(member_id.clone()) {
                                let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: member_id });
                            }
                        }
                    }
                    if state.voice_connecting {
                        ui.text_disabled("Connecting to voice...");
                    }
                    ui.popup("vc_member_context", || {
                        let Some(member_id) = state.context_vc_member.clone() else { return };
                        let display_name = state.known_users.get(&member_id).map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone())).unwrap_or_else(|| member_id.clone());
                        ui.text_disabled(&display_name);
                        ui.separator();
                        if ui.selectable("Profile") {
                            if let Some(user) = state.known_users.get(&member_id).cloned() {
                                state.profile_popout = Some(user);
                            }
                        }
                        if ui.selectable("Mention") {
                            state.compose_buf.push_str(&format!("<@{member_id}> "));
                            state.focus_compose = true;
                        }
                        if ui.selectable("Message") {
                            let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: member_id.clone() });
                        }
                        ui.separator();
                        draw_user_volume_controls(ui, state, cmd_tx, &member_id);
                    });
                }
            } else {
                draw_dm_sidebar(ui, state, cmd_tx, textures);
            }
            });

            if state.connected_voice_channel.is_some() {
                ui.separator();
                ui.child_window("voice_status_bar").size([0.0, VOICE_STATUS_BAR_HEIGHT]).build(|| {
                    draw_voice_status_bar(ui, state, textures, cmd_tx);
                });
            }
            ui.separator();
            draw_user_bar(ui, state, cmd_tx, textures);
        });
        _channels_bg.pop();

    ui.window("##sidebar_splitter")
        .position([RAIL_WIDTH + sidebar_width - SPLITTER_WIDTH / 2.0, 0.0], imgui::Condition::Always)
        .size([SPLITTER_WIDTH, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(false)
        .build(|| {
            ui.invisible_button("##splitter", [SPLITTER_WIDTH, display_h]);
            if ui.is_item_hovered() || ui.is_item_active() {
                ui.set_mouse_cursor(Some(imgui::MouseCursor::ResizeEW));
            }
            if ui.is_item_active() {
                state.sidebar_width += ui.io().mouse_delta[0];
            }
        });

    draw_settings_dialog(ui, state, cmd_tx, textures);
    draw_profile_popout(ui, state, cmd_tx, textures);

    let members_panel_open = state.show_members_panel && state.selected_guild.is_some();
    let members_panel_width = if members_panel_open { MEMBERS_PANEL_WIDTH } else { 0.0 };

    ui.window("chat")
        .position([RAIL_WIDTH + sidebar_width, 0.0], imgui::Condition::Always)
        .size([display_w - RAIL_WIDTH - sidebar_width - members_panel_width, display_h], imgui::Condition::Always)
        .no_decoration()
        .scrollable(false)
        .draw_background(true)
        .build(|| {
            let show_reconnecting = state.logged_in && !state.gateway_connected;
            let status_height = if state.status_message.is_some() || show_reconnecting { 24.0 } else { 0.0 };
            let messages = state.active_messages().to_vec();
            let channel_key = state.selected_channel.clone().unwrap_or_default();
            let seen_count = state.last_seen_message_count.get(&channel_key).copied().unwrap_or(0);
            let has_new_messages = messages.len() > seen_count;

            let dm_channel = state.selected_channel.as_ref().and_then(|id| state.dm_channels.iter().find(|c| c.id == *id).cloned());
            let mut header_height = if dm_channel.is_some() { HEADER_HEIGHT } else { 0.0 };
            if let Some(dm) = &dm_channel {
                draw_dm_header(ui, state, cmd_tx, dm);
                if state.connected_voice_channel.as_ref().is_some_and(|(_, cid)| *cid == dm.id) {
                    draw_dm_call_panel(ui, state, textures, cmd_tx, dm);
                    header_height += CALL_PANEL_HEIGHT;
                }
            }

            if state.selected_guild.is_some() {
                let button_label = if state.show_members_panel { "Members >" } else { "< Members" };
                let button_width = ui.calc_text_size(button_label)[0] + 20.0;
                ui.set_cursor_pos([ui.content_region_avail()[0] - button_width, 4.0]);
                if ui.button(button_label) {
                    state.show_members_panel = !state.show_members_panel;
                    if state.show_members_panel {
                        if let Some(guild_id) = state.selected_guild.clone() {
                            if state.guild_members_requested.insert(guild_id.clone()) {
                                let _ = cmd_tx.send(UiCommand::RequestGuildMembers { guild_id });
                            }
                        }
                    }
                }
            }

            let line_height = ui.text_line_height_with_spacing();
            let typing_visible = !channel_key.is_empty()
                && state
                    .typing_users_for(&channel_key)
                    .into_iter()
                    .any(|id| state.current_user.as_ref().is_none_or(|u| u.id != *id));
            let extra_banner_height = (typing_visible as u32 as f32
                + state.reply_target.is_some() as u32 as f32
                + state.pending_attachment.is_some() as u32 as f32
                + state.editing.is_some() as u32 as f32)
                * line_height;

            ui.child_window("history")
                .size([0.0, display_h - 80.0 - status_height - header_height - extra_banner_height])
                .build(|| {
                    if messages.is_empty() {
                        ui.text_disabled("No messages loaded yet.");
                    }

                    if state.loading_older_messages {
                        ui.text_disabled("Loading older messages...");
                    }

                    let content_x_offset = AVATAR_SIZE[0] + 12.0;

                    for (idx, msg) in messages.iter().enumerate() {
                        if msg.kind == 3 {
                            let me = state.current_user.as_ref().map(|u| u.id.as_str()).unwrap_or_default();
                            let (text, color) = call_message_text(msg, me);
                            let missed = color.is_none();
                            let mut pos = ui.cursor_pos();
                            pos[0] += 24.0 + AVATAR_SIZE[0] + 12.0 - EMOJI_SIZE - 12.0;
                            ui.set_cursor_pos(pos);
                            if !draw_unicode_emoji(ui, textures, if missed { '\u{1F4F5}' } else { '\u{1F4DE}' }, [EMOJI_SIZE, EMOJI_SIZE]) {
                                ui.dummy([EMOJI_SIZE, EMOJI_SIZE]);
                            }
                            ui.same_line();
                            ui.text_colored(state.theme.text_muted, format!("{text}  {}", format_timestamp(&msg.timestamp)));
                            ui.dummy([0.0, MESSAGE_GAP]);
                            continue;
                        }
                        let starts_group = msg.referenced_message.is_some()
                            || match messages.get(idx.wrapping_sub(1)) {
                                Some(prev) if idx > 0 => {
                                    prev.author.id != msg.author.id || !same_group_window(&prev.timestamp, &msg.timestamp)
                                }
                                _ => true,
                            };

                        let mut row_start = ui.cursor_pos();
                        let mut row_screen_start = ui.cursor_screen_pos();
                        row_start[0] += 24.0;
                        row_screen_start[0] += 24.0;
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
                                    draw_markdown(ui, fonts, &state.theme, &msg.content, &mut state.revealed_spoilers, &msg.id, textures, cmd_tx, &state.known_users, &mut state.profiles_requested);
                                    if msg.edited_timestamp.is_some() {
                                        ui.text_colored(state.theme.text_muted, "(edited)");
                                    }
                                }
                                for (embed_idx, embed) in msg.embeds.iter().enumerate() {
                                    draw_embed(ui, fonts, &state.theme, embed, &format!("{}_{embed_idx}", msg.id), textures, cmd_tx, &mut state.image_lightbox, &mut state.revealed_spoilers, &state.known_users, &mut state.profiles_requested);
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
                                    draw_markdown(ui, fonts, &state.theme, &msg.content, &mut state.revealed_spoilers, &msg.id, textures, cmd_tx, &state.known_users, &mut state.profiles_requested);
                                    if msg.edited_timestamp.is_some() {
                                        ui.text_colored(state.theme.text_muted, "(edited)");
                                    }
                                }
                                for (embed_idx, embed) in msg.embeds.iter().enumerate() {
                                    draw_embed(ui, fonts, &state.theme, embed, &format!("{}_{embed_idx}", msg.id), textures, cmd_tx, &mut state.image_lightbox, &mut state.revealed_spoilers, &state.known_users, &mut state.profiles_requested);
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
                            if ui.menu_item("Copy Text") {
                                ui.set_clipboard_text(&msg.content);
                            }
                            if state.current_user.as_ref().is_some_and(|u| u.id == msg.author.id) {
                                if ui.menu_item("Edit") {
                                    let previous = std::mem::replace(&mut state.compose_buf, msg.content.clone());
                                    let draft = state.editing.take().map_or(previous, |(_, d)| d);
                                    state.editing = Some((msg.id.clone(), draft));
                                    state.focus_compose = true;
                                }
                                if let Some(_m) = ui.begin_menu("Delete") {
                                    if ui.menu_item("Confirm delete") {
                                        let _ = cmd_tx.send(UiCommand::DeleteMessage {
                                            channel_id: msg.channel_id.clone(),
                                            message_id: msg.id.clone(),
                                        });
                                    }
                                }
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
                    let snapping_to_bottom = has_new_messages && (near_bottom || seen_count == 0);
                    if snapping_to_bottom {
                        ui.set_scroll_here_y_with_ratio(1.0);
                    }

                    if !snapping_to_bottom && scroll_y <= 0.0 && !messages.is_empty() && !state.loading_older_messages && !state.history_exhausted.contains(&channel_key) {
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
            if state.window_focused && !channel_key.is_empty() {
                if let Some(newest) = messages.last() {
                    if state.mark_channel_read(&channel_key, &newest.id) {
                        let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: channel_key.clone(), message_id: newest.id.clone() });
                    }
                }
            }

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

            if show_reconnecting {
                let token = ui.push_style_color(imgui::StyleColor::Text, [1.0, 0.6, 0.3, 1.0]);
                ui.text("Reconnecting to Discord...");
                token.pop();
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

            if state.editing.is_some() {
                ui.text_colored(state.theme.accent, "Editing message");
                ui.same_line();
                ui.text_colored(state.theme.text_muted, "Enter to save, Esc to cancel");
            }

            let _pad = ui.push_style_var(imgui::StyleVar::FramePadding([10.0, 11.0]));
            let _c1 = ui.push_style_color(imgui::StyleColor::FrameBg, [0.0; 4]);
            let _c2 = ui.push_style_color(imgui::StyleColor::Button, [0.0; 4]);
            let _c3 = ui.push_style_color(imgui::StyleColor::Border, [0.0; 4]);
            let spacing = ui.clone_style().item_spacing[0];
            let bar_start = ui.cursor_screen_pos();
            let bar_width = ui.content_region_avail()[0];
            ui.get_window_draw_list()
                .add_rect(bar_start, [bar_start[0] + bar_width, bar_start[1] + ui.frame_height()], state.theme.panel)
                .filled(true)
                .rounding(20.0)
                .build();

            if ui.button("+") {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_file()
                {
                    state.pending_attachment = Some(path);
                }
            }
            if ui.is_item_hovered() {
                ui.tooltip_text("Attach a file");
            }
            let plus_width = ui.item_rect_size()[0];
            let right_labels = ["GIF", "Sticker", "Emoji"];
            let right_width: f32 =
                right_labels.iter().map(|l| ui.calc_text_size(l)[0] + 20.0 + spacing).sum();
            ui.same_line();

            if std::mem::take(&mut state.focus_compose) {
                ui.set_keyboard_focus_here();
            }
            let before_edit = state.compose_buf.clone();
            ui.set_next_item_width((bar_width - plus_width - right_width - spacing).max(40.0));
            let enter_pressed_raw = ui
                .input_text("##compose", &mut state.compose_buf)
                .enter_returns_true(true)
                .build();
            let input_focused = ui.is_item_focused();
            let emoji_autocomplete_active = trailing_emoji_token(&state.compose_buf).is_some();
            let mention_autocomplete_active = !emoji_autocomplete_active && trailing_mention_token(&state.compose_buf).is_some();

            let mut enter_pressed = enter_pressed_raw;
            if input_focused && emoji_autocomplete_active {
                enter_pressed = false;
                if let Some((token_start, insert_text)) = draw_emoji_autocomplete(ui, state, textures, cmd_tx, bar_start) {
                    state.compose_buf.truncate(token_start);
                    state.compose_buf.push_str(&insert_text);
                    state.compose_buf.push(' ');
                    state.focus_compose = true;
                    state.emoji_autocomplete_selected = 0;
                }
            } else if input_focused && mention_autocomplete_active {
                enter_pressed = false;
                if let Some((token_start, insert_text)) = draw_mention_autocomplete(ui, state, textures, cmd_tx, bar_start) {
                    state.compose_buf.truncate(token_start);
                    state.compose_buf.push_str(&insert_text);
                    state.compose_buf.push(' ');
                    state.focus_compose = true;
                    state.mention_autocomplete_selected = 0;
                }
            }

            for label in right_labels {
                ui.same_line();
                if ui.button(label) {
                    if label == "Emoji" {
                        ui.open_popup("emoji_picker");
                    } else {
                        ui.open_popup("picker_soon");
                    }
                }
            }
            ui.popup("picker_soon", || {
                ui.text_disabled("GIFs and stickers are coming soon.");
            });
            draw_emoji_picker(ui, state, textures, cmd_tx);

            let compose_focused = input_focused;
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

            if state.editing.is_some() && ui.is_key_pressed(imgui::Key::Escape) {
                if let Some((_, draft)) = state.editing.take() {
                    state.compose_buf = draft;
                }
            } else if enter_pressed && state.editing.is_some() {
                let (message_id, draft) = state.editing.take().unwrap();
                let content = std::mem::replace(&mut state.compose_buf, draft);
                if let (Some(channel_id), false) = (state.selected_channel.clone(), content.trim().is_empty()) {
                    let _ = cmd_tx.send(UiCommand::EditMessage { channel_id, message_id, content });
                }
                state.focus_compose = true;
            } else if enter_pressed {
                state.focus_compose = true;
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

    if members_panel_open {
        let _members_bg = ui.push_style_color(imgui::StyleColor::WindowBg, state.theme.panel);
        ui.window("members_panel")
            .position([display_w - MEMBERS_PANEL_WIDTH, 0.0], imgui::Condition::Always)
            .size([MEMBERS_PANEL_WIDTH, display_h], imgui::Condition::Always)
            .no_decoration()
            .draw_background(true)
            .build(|| {
                draw_members_panel(ui, state, cmd_tx, textures);
            });
        _members_bg.pop();
    }

    draw_image_lightbox(ui, state, textures);
    draw_incoming_calls(ui, state, cmd_tx);
    draw_voice_info(ui, state);
}

fn format_bitrate(bps: u32) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f32 / 1_000_000.0)
    } else {
        format!("{} kbps", bps / 1000)
    }
}

fn draw_voice_status_bar(ui: &Ui, state: &mut AppState, textures: &TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some((guild_id, channel_id)) = state.connected_voice_channel.clone() else { return };
    let channel_name = state.connected_voice_channel_name().unwrap_or_else(|| channel_id.clone());

    ui.text(format!("[voice] {channel_name}"));
    let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
    ui.set_window_font_scale(0.85);
    let stats = &state.voice_stats;
    let rtt = stats.rtc_rtt_ms.map(|ms| ms as u32).or(stats.ws_rtt_ms).map(|ms| format!(" | {ms} ms")).unwrap_or_default();
    ui.text(format!(
        "up {} / down {}{rtt}",
        format_bitrate(stats.bitrate_up_bps),
        format_bitrate(stats.bitrate_down_bps),
    ));
    ui.set_window_font_scale(1.0);
    token.pop();

    draw_voice_controls(ui, state, textures, cmd_tx, guild_id, &channel_id, true);
}

fn draw_user_bar(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(user) = state.current_user.clone() else { return };
    const SETTINGS_WIDTH: f32 = 70.0;
    let avatar_url =
        rustcord_client::cdn::avatar_url(&user, 64).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
    let bar_start = ui.cursor_pos();
    let bar_screen = ui.cursor_screen_pos();
    let width = ui.content_region_avail()[0];

    let clicked = ui
        .selectable_config("##user_bar")
        .size([(width - SETTINGS_WIDTH).max(40.0), USER_BAR_HEIGHT])
        .build();
    let hovered = ui.is_item_hovered();

    let avatar_pos = [bar_screen[0] + 6.0, bar_screen[1] + (USER_BAR_HEIGHT - 28.0) / 2.0];
    if let Some((texture_id, _)) = textures.get(&avatar_url) {
        ui.get_window_draw_list().add_image_rounded(texture_id, avatar_pos, [avatar_pos[0] + 28.0, avatar_pos[1] + 28.0], 14.0).build();
    } else {
        textures.request(&avatar_url, cmd_tx);
    }
    let status = state.presence_for(&user.id);
    let dot_bg = if hovered { state.theme.accent } else { state.theme.panel };
    draw_status_dot(ui, [avatar_pos[0] + 25.0, avatar_pos[1] + 25.0], 5.0, status, dot_bg);

    let status_label = if !state.custom_status_input.is_empty() {
        state.custom_status_input.clone()
    } else {
        use rustcord_client::models::PresenceStatus::*;
        match status {
            Online => "Online",
            Idle => "Idle",
            Dnd => "Do Not Disturb",
            Offline | Invisible => "Invisible",
        }
        .to_string()
    };
    let display_name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
    ui.set_cursor_pos([bar_start[0] + 42.0, bar_start[1] + 3.0]);
    ui.text(&display_name);
    ui.set_cursor_pos([bar_start[0] + 42.0, bar_start[1] + 3.0 + ui.text_line_height()]);
    let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
    ui.set_window_font_scale(0.85);
    ui.text(&status_label);
    ui.set_window_font_scale(1.0);
    token.pop();

    ui.set_cursor_pos([bar_start[0] + width - SETTINGS_WIDTH + 4.0, bar_start[1] + (USER_BAR_HEIGHT - ui.frame_height()) / 2.0]);
    if ui.button("Settings") {
        state.show_settings = !state.show_settings;
        if state.show_settings && state.voice_input_devices.is_empty() && state.voice_output_devices.is_empty() {
            let _ = cmd_tx.send(UiCommand::LoadVoiceDevices);
        }
    }
    ui.set_cursor_pos([bar_start[0], bar_start[1] + USER_BAR_HEIGHT]);

    if clicked {
        ui.open_popup("status_popup");
    }
    ui.popup("status_popup", || {
        use rustcord_client::models::PresenceStatus::*;
        if ui.button("View profile") {
            state.profile_popout = Some(user.clone());
            ui.close_current_popup();
        }
        ui.separator();
        for (st, label, key) in [
            (Online, "Online", "online"),
            (Idle, "Idle", "idle"),
            (Dnd, "Do Not Disturb", "dnd"),
            (Invisible, "Invisible", "invisible"),
        ] {
            if ui.selectable_config(label).selected(state.my_status == st).build() {
                state.my_status = st;
                let custom = Some(state.custom_status_input.clone());
                let _ = cmd_tx.send(UiCommand::SetPresence { status: key.into(), custom_status: custom });
            }
        }
        ui.separator();
        ui.set_next_item_width(220.0);
        ui.input_text("##custom_status", &mut state.custom_status_input).hint("Custom status").build();
        if ui.button("Set") {
            let key = match state.my_status {
                Idle => "idle",
                Dnd => "dnd",
                Invisible | Offline => "invisible",
                Online => "online",
            };
            let custom = Some(state.custom_status_input.clone());
            let _ = cmd_tx.send(UiCommand::SetPresence { status: key.into(), custom_status: custom });
            ui.close_current_popup();
        }
        ui.same_line();
        if ui.button("Clear") {
            state.custom_status_input.clear();
        }
    });
}

fn call_message_text(msg: &rustcord_client::models::Message, me: &str) -> (String, Option<[f32; 4]>) {
    let name = msg.author.global_name.clone().unwrap_or_else(|| msg.author.username.clone());
    let participated = msg.call.as_ref().is_some_and(|c| c.participants.iter().any(|p| p == me));
    let duration = msg.call.as_ref().and_then(|c| c.ended_timestamp.as_deref()).and_then(|end| {
        let start = chrono::DateTime::parse_from_rfc3339(&msg.timestamp).ok()?;
        let secs = (chrono::DateTime::parse_from_rfc3339(end).ok()? - start).num_seconds();
        let plural = |n: i64, unit: &str| if n == 1 { format!("a {unit}") } else { format!("{n} {unit}s") };
        Some(match secs {
            ..=59 => "a few seconds".to_string(),
            60..=3599 => plural(secs / 60, "minute"),
            _ => plural(secs / 3600, "hour"),
        })
    });
    let tail = duration.map(|d| format!(" that lasted {d}")).unwrap_or_default();
    if msg.author.id != me && !participated {
        (format!("You missed a call from {name}{tail}."), None)
    } else {
        (format!("{name} started a call{tail}."), Some([0.35, 0.7, 0.4, 1.0]))
    }
}

fn draw_voice_info(ui: &Ui, state: &mut AppState) {
    if !state.show_voice_info || state.connected_voice_channel.is_none() {
        return;
    }
    let mut open = true;
    let s = state.voice_stats.clone();
    let history = &state.voice_history;
    let series = |f: &dyn Fn(&rustcord_client::voice_gateway::VoiceStats) -> f32| -> Vec<f32> { history.iter().map(f).collect() };
    let opaque_bg = [state.theme.background[0], state.theme.background[1], state.theme.background[2], 1.0];
    let _bg_token = ui.push_style_color(imgui::StyleColor::WindowBg, opaque_bg);
    ui.window("Voice connection")
        .opened(&mut open)
        .size([380.0, 0.0], imgui::Condition::FirstUseEver)
        .always_auto_resize(true)
        .build(|| {
            let muted = state.theme.text_muted;
            let row = |label: &str, value: String| {
                ui.text_colored(muted, label);
                ui.same_line_with_pos(110.0);
                ui.text(value);
            };
            let opt = |v: Option<f32>, unit: &str| v.map(|v| format!("{v:.1} {unit}")).unwrap_or_else(|| "-".into());
            row("Server", if s.endpoint.is_empty() { "-".into() } else { s.endpoint.clone() });
            row("Audio SSRC", if s.audio_ssrc == 0 { "-".into() } else { s.audio_ssrc.to_string() });
            let encryption = match (s.dave_epoch, s.dave_code) {
                (Some(epoch), Some(c)) => format!(
                    "DAVE v{} | epoch {epoch} | code {:02x}{:02x}{:02x}{:02x}{}",
                    s.dave_version,
                    c[0],
                    c[1],
                    c[2],
                    c[3],
                    if s.dave_active { "" } else { " (not yet enabled)" }
                ),
                _ if s.dave_version > 0 => format!("DAVE v{}: no key exchange yet (alone in the call?)", s.dave_version),
                _ => "transport only (no end-to-end)".into(),
            };
            row("Encryption", encryption);
            ui.separator();
            row("Latency", format!("media {} | gateway {}", opt(s.rtc_rtt_ms, "ms"), s.ws_rtt_ms.map(|v| format!("{v} ms")).unwrap_or_else(|| "-".into())));
            row("Bitrate", format!("up {} / down {}", format_bitrate(s.bitrate_up_bps), format_bitrate(s.bitrate_down_bps)));
            row("Packet loss", format!("up {} / down {}", opt(s.loss_up_pct, "%"), opt(s.loss_down_pct, "%")));
            row("Jitter", opt(s.jitter_ms, "ms"));
            ui.separator();
            let size = [340.0, 44.0];
            let graph = |id: &str, values: Vec<f32>, overlay: String| {
                if values.len() > 1 {
                    ui.plot_lines(id, &values).graph_size(size).overlay_text(overlay).scale_min(0.0).build();
                }
            };
            graph("##g_up", series(&|x| x.bitrate_up_bps as f32 / 1000.0), format!("upload {:.0} kbps", s.bitrate_up_bps as f32 / 1000.0));
            graph("##g_down", series(&|x| x.bitrate_down_bps as f32 / 1000.0), format!("download {:.0} kbps", s.bitrate_down_bps as f32 / 1000.0));
            graph(
                "##g_rtt",
                series(&|x| x.rtc_rtt_ms.or(x.ws_rtt_ms.map(|v| v as f32)).unwrap_or(0.0)),
                format!("latency {}", opt(s.rtc_rtt_ms.or(s.ws_rtt_ms.map(|v| v as f32)), "ms")),
            );
            graph("##g_loss", series(&|x| x.loss_down_pct.unwrap_or(0.0)), format!("loss (down) {}", opt(s.loss_down_pct, "%")));
            ui.text_colored(muted, "Last 2 minutes, one sample per second.");
        });
    state.show_voice_info = open;
}

#[allow(clippy::too_many_arguments)]
fn draw_embed(
    ui: &Ui,
    fonts: &Fonts,
    theme: &crate::theme::Theme,
    embed: &rustcord_client::models::Embed,
    id_seed: &str,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    lightbox: &mut Option<String>,
    revealed_spoilers: &mut std::collections::HashSet<usize>,
    known_users: &HashMap<String, rustcord_client::models::User>,
    profiles_requested: &mut std::collections::HashSet<String>,
) {
    let color = embed
        .color
        .map(|c| [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0])
        .unwrap_or([0.30, 0.31, 0.34, 1.0]);
    let kind = embed.kind.as_deref().unwrap_or("rich");
    let is_video = embed.video.is_some() || kind == "gifv" || kind == "video";
    let thumb_is_large = embed.image.is_none() && (is_video || kind == "image");
    let large = if thumb_is_large { embed.thumbnail.as_ref() } else { embed.image.as_ref() };
    let small = if thumb_is_large { None } else { embed.thumbnail.as_ref() };
    let open_url = embed.url.clone().or_else(|| embed.video.as_ref().map(|v| v.url.clone()));

    ui.dummy([0.0, 4.0]);
    let start = ui.cursor_screen_pos();
    ui.group(|| {
        ui.indent_by(12.0);
        if let Some(provider) = embed.provider.as_ref().filter(|p| !p.text.is_empty()) {
            ui.text_colored(theme.text_muted, &provider.text);
        }
        if let Some(author) = embed.author.as_ref().filter(|a| !a.text.is_empty()) {
            ui.text(&author.text);
        }
        if let Some(title) = embed.title.as_deref().filter(|t| !t.is_empty()) {
            if let Some(url) = embed.url.as_deref() {
                if clickable_text(ui, &format!("embed_title_{id_seed}"), title, theme.accent) {
                    let _ = open::that(url);
                }
            } else {
                let font = ui.push_font(fonts.bold);
                ui.text_wrapped(title);
                font.pop();
            }
        }
        if let Some(description) = embed.description.as_deref().filter(|d| !d.is_empty()) {
            draw_markdown(ui, fonts, theme, description, revealed_spoilers, &format!("embed_{id_seed}"), textures, cmd_tx, known_users, profiles_requested);
        }
        for field in &embed.fields {
            let font = ui.push_font(fonts.bold);
            ui.text_wrapped(&field.name);
            font.pop();
            ui.text_wrapped(&field.value);
        }
        let mut draw_media = |media: &rustcord_client::models::EmbedMedia, max: [f32; 2], open_on_click: bool| {
            let url = media.best_url();
            if url.is_empty() {
                return;
            }
            if let Some((texture_id, natural)) = textures.get(url) {
                let scale = (max[0] / natural[0].max(1.0)).min(max[1] / natural[1].max(1.0)).min(1.0);
                imgui::Image::new(texture_id, [natural[0] * scale, natural[1] * scale]).build(ui);
                if ui.is_item_clicked() {
                    match (&open_url, open_on_click) {
                        (Some(u), true) => {
                            let _ = open::that(u);
                        }
                        _ => *lightbox = Some(url.to_string()),
                    }
                }
            } else if textures.is_failed(url) {
                ui.text_disabled("[image failed to load]");
            } else {
                textures.request(url, cmd_tx);
                ui.text_disabled("[loading...]");
            }
        };
        if let Some(thumb) = small {
            draw_media(thumb, [80.0, 80.0], false);
        }
        if let Some(media) = large {
            draw_media(media, [MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT], is_video);
            if is_video {
                ui.text_disabled("Click to open the video");
            }
        }
        let time = embed
            .timestamp
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string());
        let footer = embed.footer.as_ref().map(|f| f.text.clone()).filter(|t| !t.is_empty());
        let footer_line = match (footer, time) {
            (Some(f), Some(t)) => Some(format!("{f} | {t}")),
            (Some(f), None) => Some(f),
            (None, Some(t)) => Some(t),
            _ => None,
        };
        if let Some(line) = footer_line {
            ui.set_window_font_scale(0.85);
            ui.text_colored(theme.text_muted, line);
            ui.set_window_font_scale(1.0);
        }
        ui.unindent_by(12.0);
    });
    let end = ui.item_rect_max();
    ui.get_window_draw_list().add_rect(start, [start[0] + 4.0, end[1]], color).filled(true).rounding(2.0).build();
    ui.dummy([0.0, 2.0]);
}

fn draw_incoming_calls(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some(channel_id) = state.incoming_calls.first().cloned() else { return };
    let who = state
        .dm_channels
        .iter()
        .find(|c| c.id == channel_id)
        .map(|dm| dm.recipients.iter().map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone())).collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| "Someone".into());
    let [w, _] = ui.io().display_size;
    let opaque_bg = [state.theme.background[0], state.theme.background[1], state.theme.background[2], 1.0];
    let _bg_token = ui.push_style_color(imgui::StyleColor::WindowBg, opaque_bg);
    ui.window("incoming_call")
        .position([w - 260.0, 16.0], imgui::Condition::Always)
        .size([244.0, 0.0], imgui::Condition::Always)
        .no_decoration()
        .always_auto_resize(true)
        .build(|| {
            ui.text(format!("{who} is calling..."));
            if ui.button("Accept") {
                let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: None, channel_id: channel_id.clone(), ring: false });
                state.selected_guild = None;
                state.selected_channel = Some(channel_id.clone());
                state.incoming_calls.retain(|c| *c != channel_id);
                state.sync_ringtone();
            }
            ui.same_line();
            if ui.button("Decline") {
                let _ = cmd_tx.send(UiCommand::DeclineCall { channel_id: channel_id.clone() });
                state.incoming_calls.retain(|c| *c != channel_id);
                state.sync_ringtone();
            }
        });
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

fn emoji_button(ui: &Ui, textures: &TextureCache, id: &str, ch: char, tooltip: &str, active: Option<[f32; 4]>) -> bool {
    const SIZE: f32 = 30.0;
    let p = ui.cursor_screen_pos();
    let clicked = ui.invisible_button(id, [SIZE, SIZE]);
    let hovered = ui.is_item_hovered();
    let dl = ui.get_window_draw_list();
    let fill = match (active, hovered) {
        (Some(c), _) => Some(c),
        (None, true) => Some(ui.style_color(imgui::StyleColor::ButtonHovered)),
        (None, false) => Some(ui.style_color(imgui::StyleColor::Button)),
    };
    if let Some(c) = fill {
        dl.add_rect(p, [p[0] + SIZE, p[1] + SIZE], c).filled(true).rounding(6.0).build();
    }
    if let EmojiTexture::Ready(tex) = emoji_texture(textures, ch) {
        let pad = 4.0;
        dl.add_image(tex, [p[0] + pad, p[1] + pad], [p[0] + SIZE - pad, p[1] + SIZE - pad]).build();
    }
    if hovered {
        ui.tooltip_text(tooltip);
    }
    clicked
}

fn dispatch_keybinds(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    if ui.io().want_text_input {
        return;
    }
    use crate::keybinds::Action;

    let pressed = |action: Action| state.keybinds.get(action).is_some_and(|key| ui.is_key_pressed(key));

    if pressed(Action::FocusCompose) {
        state.focus_compose = true;
    }

    if let Some((guild_id, channel_id)) = state.connected_voice_channel.clone() {
        if pressed(Action::ToggleMute) {
            state.self_muted = !state.self_muted;
            if state.self_muted {
                state.self_deafened = false;
            }
            let _ = cmd_tx.send(UiCommand::SetVoiceMuteDeaf {
                guild_id: guild_id.clone(),
                channel_id: channel_id.clone(),
                self_mute: state.self_muted,
                self_deaf: state.self_deafened,
            });
        }
        if pressed(Action::ToggleDeafen) {
            state.self_deafened = !state.self_deafened;
            if state.self_deafened {
                state.self_muted = true;
            }
            let _ = cmd_tx.send(UiCommand::SetVoiceMuteDeaf {
                guild_id: guild_id.clone(),
                channel_id: channel_id.clone(),
                self_mute: state.self_muted,
                self_deaf: state.self_deafened,
            });
        }
        if pressed(Action::LeaveVoiceChannel) {
            let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id });
            state.self_muted = false;
            state.self_deafened = false;
        }
    }
}

fn draw_voice_controls(ui: &Ui, state: &mut AppState, textures: &TextureCache, cmd_tx: &UnboundedSender<UiCommand>, guild_id: Option<String>, channel_id: &str, center: bool) {
    const BUTTON: f32 = 30.0;
    const GAP: f32 = 8.0;
    const RED: [f32; 4] = [0.75, 0.25, 0.25, 1.0];
    const GREEN: [f32; 4] = [0.20, 0.60, 0.35, 1.0];
    if center {
        let total = 5.0 * BUTTON + 4.0 * GAP;
        let y = ui.cursor_pos()[1];
        ui.set_cursor_pos([(ui.window_size()[0] - total) / 2.0, y]);
    }
    let send_mute_deaf = |state: &AppState| {
        let _ = cmd_tx.send(UiCommand::SetVoiceMuteDeaf {
            guild_id: guild_id.clone(),
            channel_id: channel_id.to_string(),
            self_mute: state.self_muted,
            self_deaf: state.self_deafened,
        });
    };

    let muted = state.self_muted;
    if emoji_button(ui, textures, "vc_mute", if muted { '\u{1F507}' } else { '\u{1F3A4}' }, if muted { "Unmute" } else { "Mute" }, muted.then_some(RED)) {
        state.self_muted = !state.self_muted;
        if state.self_muted {
            state.self_deafened = false;
        }
        send_mute_deaf(state);
    }
    ui.same_line_with_spacing(0.0, GAP);
    let deafened = state.self_deafened;
    if emoji_button(ui, textures, "vc_deafen", if deafened { '\u{1F515}' } else { '\u{1F3A7}' }, if deafened { "Undeafen" } else { "Deafen" }, deafened.then_some(RED)) {
        state.self_deafened = !state.self_deafened;
        if state.self_deafened {
            state.self_muted = true;
        }
        send_mute_deaf(state);
    }
    ui.same_line_with_spacing(0.0, GAP);
    let denoise = state.denoise;
    if emoji_button(ui, textures, "vc_rnn", '\u{1F92B}', if denoise { "Noise suppression: on" } else { "Noise suppression: off" }, denoise.then_some(GREEN)) {
        state.denoise = !state.denoise;
        crate::settings::save(&crate::settings::Settings {
            theme_file: current_theme_file(&state.theme),
            input_device: state.selected_input_device.clone(),
            output_device: state.selected_output_device.clone(),
            ..state.audio_settings()
        });
        let _ = cmd_tx.send(UiCommand::SetMicProcessing { gain: state.mic_gain, denoise: state.denoise });
    }
    ui.same_line_with_spacing(0.0, GAP);
    if emoji_button(ui, textures, "vc_info", '\u{2139}', "Connection info", state.show_voice_info.then_some([0.35, 0.40, 0.95, 1.0])) {
        state.show_voice_info = !state.show_voice_info;
    }
    ui.same_line_with_spacing(0.0, GAP);
    if emoji_button(ui, textures, "vc_disconnect", '\u{1F4F4}', "Disconnect", None) {
        let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id: guild_id.clone() });
        state.self_muted = false;
        state.self_deafened = false;
    }
}

fn draw_user_volume_controls(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, user_id: &str) {
    if state.current_user.as_ref().is_some_and(|u| u.id == user_id) {
        return;
    }
    let current = state.user_volumes.get(user_id).copied().unwrap_or(1.0);
    let mut new_volume = None;
    let mut muted = current == 0.0;
    if ui.checkbox("Mute for me", &mut muted) {
        new_volume = Some(if muted { 0.0 } else { 1.0 });
    }
    let mut percent = current * 100.0;
    ui.set_next_item_width(160.0);
    if ui.slider_config("##user_volume", 0.0, 200.0).display_format("%.0f%%").build(&mut percent) {
        new_volume = Some(percent / 100.0);
    }
    if let Some(volume) = new_volume {
        state.user_volumes.insert(user_id.to_string(), volume);
        crate::settings::save(&crate::settings::Settings {
            theme_file: current_theme_file(&state.theme),
            input_device: state.selected_input_device.clone(),
            output_device: state.selected_output_device.clone(),
            ..state.audio_settings()
        });
        let _ = cmd_tx.send(UiCommand::SetUserVolume { user_id: user_id.to_string(), volume });
    }
}

const CALL_PANEL_HEIGHT: f32 = 130.0;

fn draw_dm_call_panel(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, dm: &rustcord_client::models::Channel) {
    const SIZE: f32 = 56.0;
    const GAP: f32 = 16.0;
    let channel_id = dm.id.clone();
    let members: Vec<(rustcord_client::models::User, bool)> = state
        .members_in_voice_channel(&channel_id, &channel_id)
        .into_iter()
        .filter_map(|vs| {
            let user = state
                .known_users
                .get(&vs.user_id)
                .or_else(|| dm.recipients.iter().find(|u| u.id == vs.user_id))
                .or_else(|| state.current_user.as_ref().filter(|u| u.id == vs.user_id))?;
            Some((user.clone(), state.voice_speaking_users.contains(&vs.user_id)))
        })
        .collect();
    let in_call = state.connected_voice_channel.as_ref().is_some_and(|(_, cid)| *cid == channel_id);
    ui.child_window("dm_call_panel").size([0.0, CALL_PANEL_HEIGHT]).build(|| {
        let width = ui.window_size()[0];
        let row_w = members.len() as f32 * SIZE + members.len().saturating_sub(1) as f32 * GAP;
        ui.set_cursor_pos([(width - row_w) / 2.0, 12.0]);
        for (user, speaking) in &members {
            let p = ui.cursor_screen_pos();
            let center = [p[0] + SIZE / 2.0, p[1] + SIZE / 2.0];
            let url = rustcord_client::cdn::avatar_url(user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
            match textures.get(&url) {
                Some((texture_id, _)) => draw_circular_image(ui, texture_id, [SIZE, SIZE]),
                None => {
                    textures.request(&url, cmd_tx);
                    ui.dummy([SIZE, SIZE]);
                }
            }
            if *speaking {
                ui.get_window_draw_list().add_circle(center, SIZE / 2.0 + 2.0, [0.25, 0.8, 0.4, 1.0]).thickness(3.0).num_segments(48).build();
            }
            if ui.is_item_hovered() {
                ui.tooltip_text(user.global_name.clone().unwrap_or_else(|| user.username.clone()));
                if ui.is_mouse_clicked(imgui::MouseButton::Right) {
                    state.context_vc_member = Some(user.id.clone());
                    ui.open_popup("dm_call_member_ctx");
                }
            }
            ui.same_line_with_spacing(0.0, GAP);
        }
        ui.popup("dm_call_member_ctx", || {
            let Some(member_id) = state.context_vc_member.clone() else { return };
            let known = state.known_users.get(&member_id).cloned().or_else(|| dm.recipients.iter().find(|u| u.id == member_id).cloned());
            if let Some(u) = &known {
                ui.text_disabled(u.global_name.clone().unwrap_or_else(|| u.username.clone()));
                ui.separator();
            }
            if ui.selectable("Profile") {
                state.profile_popout = known;
            }
            draw_user_volume_controls(ui, state, cmd_tx, &member_id);
        });
        if in_call {
            ui.set_cursor_pos([0.0, 12.0 + SIZE + 18.0]);
            draw_voice_controls(ui, state, textures, cmd_tx, None, &channel_id, true);
        }
    });
}

fn draw_dm_header(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, dm: &rustcord_client::models::Channel) {
    let name = AppState::dm_display_name(dm);
    let channel_id = dm.id.clone();
    let is_connected = state.connected_voice_channel.as_ref().is_some_and(|(_, cid)| *cid == channel_id);
    let is_connecting = state.voice_connecting && state.pending_voice_channel.as_ref().is_some_and(|(_, cid)| *cid == channel_id);

    ui.child_window("dm_header").size([0.0, HEADER_HEIGHT]).build(|| {
        ui.set_cursor_pos([12.0, (HEADER_HEIGHT - 20.0) / 2.0]);
        ui.text(&name);
        ui.same_line();
        let right_edge = ui.window_size()[0] - 16.0;
        let spacing = ui.clone_style().item_spacing[0];
        let frame_padding_x = ui.clone_style().frame_padding[0];
        let call_width = ui.calc_text_size("Call")[0] + frame_padding_x * 2.0;
        let video_width = ui.calc_text_size("Video")[0] + frame_padding_x * 2.0;
        let row_width = call_width + spacing + video_width;
        let button_height = ui.text_line_height() + ui.clone_style().frame_padding[1] * 2.0;
        ui.set_cursor_pos([right_edge - row_width, (HEADER_HEIGHT - button_height) / 2.0]);

        if is_connected {
            let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.accent);
            if ui.button("Call") {
                let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id: None });
            }
            token.pop();
            if ui.is_item_hovered() {
                ui.tooltip_text("Hang up");
            }
        } else if is_connecting {
            ui.text_disabled("Call");
        } else if ui.button("Call") {
            let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: None, channel_id: channel_id.clone(), ring: true });
            state.pending_voice_channel = Some((None, channel_id.clone()));
        }
        if !is_connected && !is_connecting && ui.is_item_hovered() {
            ui.tooltip_text("Start Voice Call");
        }

        ui.same_line();
        let _disabled = ui.begin_disabled(true);
        ui.button("Video");
        drop(_disabled);
        if ui.is_item_hovered() {
            ui.tooltip_text("Video calls aren't supported yet");
        }
    });
    ui.separator();
}

fn draw_members_panel(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(guild_id) = state.selected_guild.clone() else { return };
    let Some(members) = state.guild_members.get(&guild_id).cloned() else {
        ui.text_disabled("Loading members...");
        return;
    };

    let roles = state.guild_roles.get(&guild_id).cloned().unwrap_or_default();
    let mut roles_by_position = roles.clone();
    roles_by_position.sort_by_key(|r| std::cmp::Reverse(r.position));

    let highest_role_name = |member: &rustcord_client::models::GuildMember| -> String {
        roles_by_position
            .iter()
            .find(|r| member.roles.contains(&r.id))
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "Members".to_string())
    };

    let mut groups: Vec<(String, Vec<&rustcord_client::models::GuildMember>)> = Vec::new();
    for member in &members {
        let group_name = highest_role_name(member);
        match groups.iter_mut().find(|(name, _)| *name == group_name) {
            Some((_, list)) => list.push(member),
            None => groups.push((group_name, vec![member])),
        }
    }
    groups.sort_by_key(|(name, _)| {
        roles_by_position
            .iter()
            .position(|r| &r.name == name)
            .map(|p| p as i32)
            .unwrap_or(i32::MAX)
    });

    for (group_name, group_members) in &groups {
        let online: Vec<&&rustcord_client::models::GuildMember> = group_members
            .iter()
            .filter(|m| state.presence_for(&m.user.id) != rustcord_client::models::PresenceStatus::Offline)
            .collect();
        let offline: Vec<&&rustcord_client::models::GuildMember> = group_members
            .iter()
            .filter(|m| state.presence_for(&m.user.id) == rustcord_client::models::PresenceStatus::Offline)
            .collect();

        let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
        ui.text(format!("{} — {}", group_name.to_uppercase(), online.len()));
        token.pop();

        for member in &online {
            draw_member_row(ui, state, cmd_tx, textures, member);
        }
        for member in &offline {
            draw_member_row(ui, state, cmd_tx, textures, member);
        }
        ui.spacing();
    }
}

fn draw_member_row(
    ui: &Ui,
    state: &mut AppState,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    member: &rustcord_client::models::GuildMember,
) {
    let name = member.nick.clone().unwrap_or_else(|| member.user.global_name.clone().unwrap_or_else(|| member.user.username.clone()));
    let status = state.presence_for(&member.user.id);
    let platform = state.platform_for(&member.user.id).map(str::to_string);
    let activity = state.activity_for(&member.user.id);
    let offline = status == rustcord_client::models::PresenceStatus::Offline;
    let _dim = offline.then(|| ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted));
    let result = draw_user_row(ui, cmd_tx, textures, &member.user, &name, false, status, platform.as_deref(), activity.as_deref(), state.theme.panel);
    if result.avatar_clicked {
        state.profile_popout = Some(member.user.clone());
    } else if result.row_clicked {
        let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: member.user.id.clone() });
    } else if result.right_clicked {
        state.context_vc_member = Some(member.user.id.clone());
        ui.open_popup("member_panel_ctx");
    }

    ui.popup("member_panel_ctx", || {
        let Some(member_id) = state.context_vc_member.clone() else { return };
        let Some(guild_id) = state.selected_guild.clone() else { return };
        if let Some(user) = state.known_users.get(&member_id).cloned() {
            ui.text_disabled(user.global_name.clone().unwrap_or_else(|| user.username.clone()));
            ui.separator();
            if ui.selectable("Profile") {
                state.profile_popout = Some(user);
            }
        }
        if ui.selectable("Message") {
            let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: member_id.clone() });
        }
        let permissions = state.my_base_permissions(&guild_id);
        let can_kick = rustcord_client::permissions::has_permission(permissions, rustcord_client::permissions::KICK_MEMBERS);
        let can_ban = rustcord_client::permissions::has_permission(permissions, rustcord_client::permissions::BAN_MEMBERS);
        if can_kick || can_ban {
            ui.separator();
        }
        if can_kick && ui.selectable("Kick") {
            let _ = cmd_tx.send(UiCommand::KickMember { guild_id: guild_id.clone(), user_id: member_id.clone() });
        }
        if can_ban && ui.selectable("Ban") {
            let _ = cmd_tx.send(UiCommand::BanMember { guild_id, user_id: member_id });
        }
    });
}

fn draw_dm_sidebar(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let plus_width = 28.0;
    ui.set_next_item_width(ui.content_region_avail()[0] - plus_width - ui.clone_style().item_spacing[0]);
    ui.input_text("##search", &mut state.dm_search)
        .hint("Search friends or DMs")
        .build();
    ui.same_line();
    if ui.button("+##new_group") {
        ui.open_popup("new_group");
    }
    ui.popup("new_group", || {
        ui.text("New DM group");
        ui.input_text("##group_name", &mut state.new_group_name).build();
        let name = state.new_group_name.trim().to_string();
        if ui.button("Create") && !name.is_empty() {
            state.groups.push(crate::app_state::DmGroup { name, channels: Vec::new() });
            state.sidebar_tab = state.groups.len() + 1;
            state.new_group_name.clear();
            state.save_groups();
            ui.close_current_popup();
        }
    });

    if let Some(_bar) = ui.tab_bar("sidebar_tabs") {
        let labels: Vec<String> =
            ["DMs".to_string(), "Friends".to_string()].into_iter().chain(state.groups.iter().map(|g| g.name.clone())).collect();
        for (i, label) in labels.iter().enumerate() {
            if ui.tab_item(format!("{label}###tab{i}")).is_some() {
                state.sidebar_tab = i;
            }
        }
    }
    if state.sidebar_tab >= state.groups.len() + 2 {
        state.sidebar_tab = 0;
    }

    if state.sidebar_tab == 1 {
        let friends: Vec<rustcord_client::models::User> = state.friends().into_iter().map(|r| r.user.clone()).collect();
        if friends.is_empty() {
            ui.text_disabled("No friends found.");
        }
        for user in &friends {
            let name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
            let status = state.presence_for(&user.id);
            let platform = state.platform_for(&user.id).map(str::to_string);
            let activity = state.activity_for(&user.id);
            let result = draw_user_row(ui, cmd_tx, textures, user, &name, false, status, platform.as_deref(), activity.as_deref(), state.theme.panel);
            if result.avatar_clicked {
                state.profile_popout = Some(user.clone());
            } else if result.row_clicked {
                let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: user.id.clone() });
            }
        }
        return;
    }

    let group = state.sidebar_tab.checked_sub(2);
    if let Some(g) = group {
        if ui.small_button("Delete group") {
            state.groups.remove(g);
            state.sidebar_tab = 0;
            state.save_groups();
            return;
        }
    }
    let dms: Vec<(String, String, Option<rustcord_client::models::User>, Option<String>)> = state
        .filtered_dm_channels()
        .into_iter()
        .filter(|dm| group.is_none_or(|g| state.groups[g].channels.contains(&dm.id)))
        .map(|dm| (dm.id.clone(), AppState::dm_display_name(dm), dm.recipients.first().cloned(), dm.last_message_id.clone()))
        .collect();
    if dms.is_empty() {
        ui.text_disabled(if group.is_some() { "Empty group. Right-click a DM to add it." } else { "No direct messages." });
    }
    for (channel_id, name, recipient, last_message_id) in &dms {
        let is_selected = state.selected_channel.as_deref() == Some(channel_id.as_str());
        let row_start = ui.cursor_screen_pos();
        let (row_clicked, avatar_clicked, right_clicked) = if let Some(user) = recipient {
            let status = state.presence_for(&user.id);
            let platform = state.platform_for(&user.id).map(str::to_string);
            let activity = state.activity_for(&user.id);
            let r = draw_user_row(ui, cmd_tx, textures, user, name, is_selected, status, platform.as_deref(), activity.as_deref(), state.theme.panel);
            (r.row_clicked, r.avatar_clicked, r.right_clicked)
        } else {
            let clicked = ui.selectable_config(name).selected(is_selected).build();
            (clicked, false, ui.is_item_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Right))
        };
        if state.channel_is_unread(channel_id, last_message_id.as_deref()) {
            draw_unread_dot(ui, row_start);
        }
        if right_clicked {
            state.context_dm = Some(channel_id.clone());
            ui.open_popup("dm_context");
        }
        if avatar_clicked {
            if let Some(user) = recipient {
                state.profile_popout = Some(user.clone());
            }
        } else if row_clicked {
            let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
            state.selected_channel = Some(channel_id.clone());
            if let Some(last_id) = last_message_id {
                if state.mark_channel_read(channel_id, last_id) {
                    let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: channel_id.clone(), message_id: last_id.clone() });
                }
            }
        }
    }

    ui.popup("dm_context", || {
        let Some(channel_id) = state.context_dm.clone() else { return };
        if state.groups.is_empty() {
            ui.text_disabled("Create a group with + first");
        }
        let mut changed = false;
        for g in &mut state.groups {
            let mut member = g.channels.contains(&channel_id);
            if ui.checkbox(format!("{}##ctx", g.name), &mut member) {
                if member {
                    g.channels.push(channel_id.clone());
                } else {
                    g.channels.retain(|c| c != &channel_id);
                }
                changed = true;
            }
        }
        if changed {
            state.save_groups();
        }
    });
}

fn snowflake_date(id: &str) -> String {
    let ms = (id.parse::<u64>().unwrap_or(0) >> 22) as i64 + 1_420_070_400_000;
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%b %d, %Y").to_string()).unwrap_or_default()
}

fn draw_profile_popout(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(user) = state.profile_popout.clone() else { return };
    if state.profiles_requested.insert(user.id.clone()) {
        let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: user.id.clone() });
    }
    let profile = state.profiles.get(&user.id).cloned().unwrap_or_default();
    let [display_w, display_h] = ui.io().display_size;
    let mut open = true;
    let mut message_clicked = false;
    let opaque_bg = [state.theme.background[0], state.theme.background[1], state.theme.background[2], 1.0];
    let _bg_token = ui.push_style_color(imgui::StyleColor::WindowBg, opaque_bg);
    ui.window(format!("Profile##{}", user.id))
        .opened(&mut open)
        .position([display_w / 2.0 - 260.0, display_h / 2.0 - 260.0], imgui::Condition::Appearing)
        .size([520.0, 520.0], imgui::Condition::Appearing)
        .build(|| {
            let width = ui.content_region_avail()[0];
            let banner_size = [width, width * 0.3];
            let banner_pos = ui.cursor_pos();
            match profile.banner_url.as_deref().and_then(|u| textures.get(u).map(|t| (u, t))) {
                Some((_, (texture_id, _))) => imgui::Image::new(texture_id, banner_size).build(ui),
                None => {
                    if let Some(url) = &profile.banner_url {
                        textures.request(url, cmd_tx);
                    }
                    let p = ui.cursor_screen_pos();
                    ui.get_window_draw_list()
                        .add_rect(p, [p[0] + banner_size[0], p[1] + banner_size[1]], state.theme.accent)
                        .filled(true)
                        .build();
                    ui.dummy(banner_size);
                }
            }
            ui.set_cursor_pos([banner_pos[0] + 12.0, banner_pos[1] + banner_size[1] - 36.0]);
            let avatar_url =
                rustcord_client::cdn::avatar_url(&user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
            let avatar_screen_pos = ui.cursor_screen_pos();
            ui.get_window_draw_list()
                .add_circle([avatar_screen_pos[0] + 40.0, avatar_screen_pos[1] + 40.0], 44.0, state.theme.panel)
                .filled(true)
                .build();
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
            if user.bot {
                ui.same_line();
                ui.text_disabled("BOT");
            }
            ui.text_disabled(format!("@{}", user.username));
            for badge in &profile.badges {
                ui.same_line();
                if let Some((texture_id, _)) = textures.get(&badge.icon_url) {
                    imgui::Image::new(texture_id, [18.0, 18.0]).build(ui);
                    if ui.is_item_hovered() {
                        ui.tooltip_text(&badge.description);
                    }
                } else {
                    textures.request(&badge.icon_url, cmd_tx);
                    ui.dummy([18.0, 18.0]);
                }
            }
            if ui.button("Message") {
                let _ = cmd_tx.send(UiCommand::OpenDmWithUser { user_id: user.id.clone() });
                message_clicked = true;
            }
            if let Some(presence) = state.presences.get(&user.id) {
                let muted = state.theme.text_muted;
                for activity in presence.activities.iter().filter(|a| a.kind != 4) {
                    let image = activity.large_image_url();
                    let lines = [activity.details.as_deref().unwrap_or(""), activity.state.as_deref().unwrap_or("")];
                    draw_activity_card(ui, cmd_tx, textures, image.as_deref(), &activity.summary_title(), &lines, muted);
                }
                if let Some(custom) = presence.activities.iter().find(|a| a.kind == 4) {
                    ui.text_wrapped(custom.summary());
                }
            }

            if let Some(_bar) = ui.tab_bar("profile_tabs") {
                if ui.tab_item("About").is_some() {
                    state.profile_tab = 0;
                    if let Some(bio) = &profile.bio {
                        ui.text_wrapped(bio);
                        ui.spacing();
                    }
                    ui.text_disabled("Member Since");
                    ui.text(snowflake_date(&user.id));
                    if let Some(since) = state.relationships.iter().find(|r| r.user.id == user.id).and_then(|r| r.since.as_deref()) {
                        ui.text_disabled("Friends Since");
                        ui.text(since.get(..10).unwrap_or(since));
                    }
                    ui.spacing();
                    ui.text_disabled("Note (only visible to you)");
                    let mut note = state.notes.get(&user.id).cloned().unwrap_or_default();
                    if ui.input_text_multiline("##note", &mut note, [-1.0, 60.0]).build() {
                        if note.is_empty() {
                            state.notes.remove(&user.id);
                        } else {
                            state.notes.insert(user.id.clone(), note);
                        }
                        crate::app_state::save_notes(&state.notes);
                    }
                }
                if ui.tab_item(format!("{} Mutual Friends###mf", profile.mutual_friends.len())).is_some() {
                    state.profile_tab = 1;
                    for friend in &profile.mutual_friends {
                        let name = friend.global_name.clone().unwrap_or_else(|| friend.username.clone());
                        let avatar =
                            rustcord_client::cdn::avatar_url(friend, 32).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(friend));
                        if let Some((texture_id, _)) = textures.get(&avatar) {
                            draw_circular_image(ui, texture_id, [24.0, 24.0]);
                        } else {
                            textures.request(&avatar, cmd_tx);
                            ui.dummy([24.0, 24.0]);
                        }
                        ui.same_line();
                        ui.text(name);
                    }
                }
                if ui.tab_item(format!("{} Mutual Servers###ms", profile.mutual_guild_ids.len())).is_some() {
                    state.profile_tab = 2;
                    for id in &profile.mutual_guild_ids {
                        let name = state.guilds.iter().find(|g| &g.id == id).map(|g| g.name.as_str()).unwrap_or("Unknown server");
                        ui.text(name);
                    }
                }
            }
        });
    if !open || message_clicked {
        state.profile_popout = None;
    }
}

fn draw_activity_card(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    image_url: Option<&str>,
    title: &str,
    lines: &[&str],
    muted: [f32; 4],
) {
    const IMG: [f32; 2] = [48.0, 48.0];
    let top = ui.cursor_screen_pos();
    match image_url.and_then(|u| textures.get(u)) {
        Some((texture_id, _)) => {
            ui.get_window_draw_list().add_image_rounded(texture_id, top, [top[0] + IMG[0], top[1] + IMG[1]], 6.0).build();
        }
        None => {
            if let Some(u) = image_url {
                textures.request(u, cmd_tx);
            }
            ui.get_window_draw_list().add_rect(top, [top[0] + IMG[0], top[1] + IMG[1]], [1.0, 1.0, 1.0, 0.06]).filled(true).rounding(6.0).build();
        }
    }
    ui.dummy(IMG);
    ui.same_line();
    ui.group(|| {
        ui.text(title);
        for line in lines.iter().filter(|l| !l.is_empty()) {
            let token = ui.push_style_color(imgui::StyleColor::Text, muted);
            ui.text(line);
            token.pop();
        }
    });
}

fn draw_settings_dialog(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    if !state.show_settings {
        return;
    }
    let [display_w, display_h] = ui.io().display_size;
    let mut open = state.show_settings;
    let opaque_bg = [state.theme.background[0], state.theme.background[1], state.theme.background[2], 1.0];
    let _bg_token = ui.push_style_color(imgui::StyleColor::WindowBg, opaque_bg);
    ui.window("Settings")
        .opened(&mut open)
        .position([display_w / 2.0 - 280.0, display_h / 2.0 - 260.0], imgui::Condition::Appearing)
        .size([560.0, 520.0], imgui::Condition::Appearing)
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
            ui.text("Notifications");
            ui.separator();
            ui.checkbox("Desktop notifications for DMs and mentions", &mut state.notifications_enabled);

            ui.spacing();
            ui.separator();
            ui.text("Activity");
            ui.separator();
            let muted = state.theme.text_muted;
            ui.text_colored(muted, "Forced activity (shown as Playing)");
            let [name, details, st] = &mut state.manual_activity;
            ui.set_next_item_width(-1.0);
            ui.input_text("##act_name", name).hint("Name").build();
            ui.set_next_item_width(-1.0);
            ui.input_text("##act_details", details).hint("Details").build();
            ui.set_next_item_width(-1.0);
            ui.input_text("##act_state", st).hint("State").build();
            if ui.button("Apply##activity") {
                let _ = cmd_tx.send(UiCommand::SetManualActivity { name: name.clone(), details: details.clone(), state: st.clone() });
            }
            ui.same_line();
            if ui.button("Remove##activity") {
                name.clear();
                details.clear();
                st.clear();
                let _ = cmd_tx.send(UiCommand::SetManualActivity { name: String::new(), details: String::new(), state: String::new() });
            }

            ui.spacing();
            ui.text_colored(muted, "Apps sharing activity via RPC");
            if state.rpc_apps.is_empty() {
                ui.text_disabled("No apps connected. Quit the official client first so apps connect here.");
            }
            let apps = state.rpc_apps.clone();
            for app in &apps {
                let image = app.image_url.as_deref().or(app.icon_url.as_deref());
                draw_activity_card(ui, cmd_tx, textures, image, &app.name, &[&app.detail], muted);
                let mut enabled = app.enabled;
                if ui.checkbox(format!("Share##rpc{}", app.conn_id), &mut enabled) {
                    let _ = cmd_tx.send(UiCommand::SetRpcEnabled { conn_id: app.conn_id, enabled });
                }
                ui.spacing();
            }

            ui.spacing();
            ui.separator();
            ui.text("Voice & Video");
            ui.separator();
            if state.voice_input_devices.is_empty() && state.voice_output_devices.is_empty() {
                if ui.button("Refresh device list") {
                    let _ = cmd_tx.send(UiCommand::LoadVoiceDevices);
                }
            } else {
                ui.text_colored(muted, "Input Device");
                ui.set_next_item_width(-1.0);
                let current_input = state.selected_input_device.clone().unwrap_or_else(|| "Default".to_string());
                if let Some(_token) = ui.begin_combo("##input_device", &current_input) {
                    if ui.selectable_config("Default").selected(state.selected_input_device.is_none()).build() {
                        state.selected_input_device = None;
                        crate::settings::save(&crate::settings::Settings { theme_file: current_theme_file(&state.theme), input_device: None, output_device: state.selected_output_device.clone(), ..state.audio_settings() });
                        let _ = cmd_tx.send(UiCommand::SetVoiceInputDevice { device: None });
                    }
                    let devices = state.voice_input_devices.clone();
                    for device in &devices {
                        let is_selected = state.selected_input_device.as_deref() == Some(device.as_str());
                        if ui.selectable_config(device).selected(is_selected).build() {
                            state.selected_input_device = Some(device.clone());
                            crate::settings::save(&crate::settings::Settings { theme_file: current_theme_file(&state.theme), input_device: Some(device.clone()), output_device: state.selected_output_device.clone(), ..state.audio_settings() });
                            let _ = cmd_tx.send(UiCommand::SetVoiceInputDevice { device: Some(device.clone()) });
                        }
                    }
                }

                ui.spacing();
                ui.text_colored(muted, "Output Device");
                ui.set_next_item_width(-1.0);
                let current_output = state.selected_output_device.clone().unwrap_or_else(|| "Default".to_string());
                if let Some(_token) = ui.begin_combo("##output_device", &current_output) {
                    if ui.selectable_config("Default").selected(state.selected_output_device.is_none()).build() {
                        state.selected_output_device = None;
                        crate::settings::save(&crate::settings::Settings { theme_file: current_theme_file(&state.theme), input_device: state.selected_input_device.clone(), output_device: None, ..state.audio_settings() });
                        let _ = cmd_tx.send(UiCommand::SetVoiceOutputDevice { device: None });
                    }
                    let devices = state.voice_output_devices.clone();
                    for device in &devices {
                        let is_selected = state.selected_output_device.as_deref() == Some(device.as_str());
                        if ui.selectable_config(device).selected(is_selected).build() {
                            state.selected_output_device = Some(device.clone());
                            crate::settings::save(&crate::settings::Settings { theme_file: current_theme_file(&state.theme), input_device: state.selected_input_device.clone(), output_device: Some(device.clone()), ..state.audio_settings() });
                            let _ = cmd_tx.send(UiCommand::SetVoiceOutputDevice { device: Some(device.clone()) });
                        }
                    }
                }
            }

            ui.spacing();
            ui.text_colored(muted, "Microphone");
            let mut changed = false;
            let mut gain_percent = state.mic_gain * 100.0;
            ui.set_next_item_width(-1.0);
            if ui.slider_config("##mic_gain", 0.0, 300.0).display_format("Gain %.0f%%").build(&mut gain_percent) {
                state.mic_gain = gain_percent / 100.0;
                changed = true;
            }
            changed |= ui.checkbox("Noise suppression (RNNoise)", &mut state.denoise);
            if changed {
                crate::settings::save(&crate::settings::Settings {
                    theme_file: current_theme_file(&state.theme),
                    input_device: state.selected_input_device.clone(),
                    output_device: state.selected_output_device.clone(),
                    ..state.audio_settings()
                });
                let _ = cmd_tx.send(UiCommand::SetMicProcessing { gain: state.mic_gain, denoise: state.denoise });
            }

            ui.spacing();
            ui.separator();
            ui.text("Keybinds");
            ui.separator();
            for action in crate::keybinds::Action::ALL {
                ui.text(action.label());
                ui.same_line();
                let is_rebinding = state.rebinding_action == Some(action);
                let button_label = if is_rebinding {
                    "Press a key...".to_string()
                } else {
                    state.keybinds.get(action).map(crate::keybinds::key_name).unwrap_or_else(|| "Unbound".to_string())
                };
                if ui.button(format!("{button_label}##bind_{}", action.label())) {
                    state.rebinding_action = Some(action);
                }
                if is_rebinding {
                    if ui.is_key_pressed(imgui::Key::Escape) {
                        state.rebinding_action = None;
                    } else if let Some(key) = imgui::Key::VARIANTS.into_iter().find(|k| ui.is_key_pressed(*k)) {
                        state.keybinds.set(action, Some(key));
                        state.rebinding_action = None;
                        crate::settings::save(&crate::settings::Settings {
                            theme_file: current_theme_file(&state.theme),
                            input_device: state.selected_input_device.clone(),
                            output_device: state.selected_output_device.clone(),
                            ..state.audio_settings()
                        });
                    }
                }
                ui.same_line();
                if ui.small_button(format!("Clear##unbind_{}", action.label())) {
                    state.keybinds.set(action, None);
                    state.rebinding_action = None;
                    crate::settings::save(&crate::settings::Settings {
                        theme_file: current_theme_file(&state.theme),
                        input_device: state.selected_input_device.clone(),
                        output_device: state.selected_output_device.clone(),
                        ..state.audio_settings()
                    });
                }
            }

            ui.spacing();
            ui.separator();
            ui.text("Theme");
            ui.separator();

            let theme_paths = crate::theme::Theme::list_available();
            if theme_paths.is_empty() {
                ui.text_disabled("No themes are available.");
            }
            for path in &theme_paths {
                let label = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let is_current = label == state.theme.name;
                if ui.selectable_config(&label).selected(is_current).build() {
                    if let Ok(theme) = crate::theme::Theme::load_from_file(path) {
                        state.theme = theme;
                        let theme_file = path.file_name().map(|name| name.to_string_lossy().into_owned());
                        crate::settings::save(&crate::settings::Settings { theme_file, input_device: state.selected_input_device.clone(), output_device: state.selected_output_device.clone(), ..state.audio_settings() });
                    }
                }
            }
        });
    state.show_settings = open;
}

fn current_theme_file(theme: &crate::theme::Theme) -> Option<String> {
    crate::theme::Theme::themes_dir().and_then(|dir| {
        crate::theme::Theme::list_available().into_iter().find(|path| {
            crate::theme::Theme::load_from_file(path).is_ok_and(|candidate| candidate.name == theme.name)
        }).and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .or_else(|| Some(dir.join(format!("{}.toml", theme.name)).file_name()?.to_string_lossy().into_owned()))
    })
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
