use crate::app_state::{AppState, GuildFolder, RailItem, SettingsCategory};
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

fn timestamp_text(ui: &Ui, color: [f32; 4], label: &str, full: &str, scale: f32) {
    let token = ui.push_style_color(imgui::StyleColor::Text, color);
    ui.set_window_font_scale(scale);
    ui.text(label);
    ui.set_window_font_scale(1.0);
    token.pop();
    if ui.is_item_hovered() {
        ui.tooltip_text(full);
    }
}

fn draw_day_divider(ui: &Ui, theme: &crate::theme::Theme, label: &str) {
    ui.dummy([0.0, 4.0]);
    let start = ui.cursor_screen_pos();
    let width = ui.content_region_avail()[0];
    ui.set_window_font_scale(0.8);
    let text_size = ui.calc_text_size(label);
    let line_y = start[1] + text_size[1] / 2.0;
    let text_x = start[0] + (width - text_size[0]) / 2.0;
    let dl = ui.get_window_draw_list();
    dl.add_line([start[0] + 16.0, line_y], [text_x - 8.0, line_y], theme.border).thickness(1.0).build();
    dl.add_line([text_x + text_size[0] + 8.0, line_y], [start[0] + width - 8.0, line_y], theme.border).thickness(1.0).build();
    dl.add_text([text_x, start[1]], theme.text_muted, label);
    ui.set_window_font_scale(1.0);
    ui.dummy([width, text_size[1] + 8.0]);
}

#[allow(clippy::too_many_arguments)]
pub struct MentionCtx<'a> {
    pub known_users: &'a HashMap<String, rustcord_client::models::User>,
    pub profiles_requested: &'a mut std::collections::HashSet<String>,
    pub channel_names: &'a HashMap<String, String>,
    pub role_names: &'a HashMap<String, (String, Option<[f32; 4]>)>,
}

const QUOTE_BAR_INDENT: f32 = 10.0;
const LIST_INDENT: f32 = 14.0;

fn bullet_glyph(depth: u8) -> char {
    match depth % 3 {
        0 => '\u{2022}',
        1 => '\u{25E6}',
        _ => '\u{25AA}',
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_markdown_table(
    ui: &Ui,
    fonts: &Fonts,
    theme: &crate::theme::Theme,
    table: &markdown::Table,
    revealed_spoilers: &mut std::collections::HashSet<usize>,
    id_seed: &str,
    block_idx: usize,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    mentions: &mut MentionCtx,
) {
    use imgui::{TableColumnSetup, TableFlags};

    let columns = table.header.len();
    if columns == 0 {
        return;
    }
    let flags = TableFlags::BORDERS | TableFlags::ROW_BG | TableFlags::SIZING_STRETCH_PROP;
    let setup: Vec<TableColumnSetup<String>> =
        (0..columns).map(|i| TableColumnSetup::new(format!("##col_{id_seed}_{block_idx}_{i}"))).collect();
    let Some(_token) = ui.begin_table_with_flags(format!("table_{id_seed}_{block_idx}"), columns, flags) else {
        return;
    };
    for column in setup {
        ui.table_setup_column(column.name);
    }

    ui.table_next_row();
    for (col, cell) in table.header.iter().enumerate() {
        ui.table_set_column_index(col);
        let bold = ui.push_font(fonts.bold);
        align_table_cell(ui, table, col, cell, fonts, theme);
        draw_spans(ui, fonts, theme, cell, revealed_spoilers, id_seed, block_idx * 100 + col, textures, cmd_tx, mentions);
        bold.pop();
    }

    for (row_idx, row) in table.rows.iter().enumerate() {
        ui.table_next_row();
        for (col, cell) in row.iter().enumerate() {
            ui.table_set_column_index(col);
            align_table_cell(ui, table, col, cell, fonts, theme);
            let seed = block_idx * 10_000 + (row_idx + 1) * 100 + col;
            draw_spans(ui, fonts, theme, cell, revealed_spoilers, id_seed, seed, textures, cmd_tx, mentions);
        }
    }
}

fn align_table_cell(
    ui: &Ui,
    table: &markdown::Table,
    col: usize,
    cell: &[markdown::Span],
    _fonts: &Fonts,
    _theme: &crate::theme::Theme,
) {
    let Some(alignment) = table.alignments.get(col).copied() else { return };
    if alignment == markdown::Alignment::Left {
        return;
    }
    let text: String = cell
        .iter()
        .map(|span| match span {
            markdown::Span::Text { text, .. } => text.as_str(),
            markdown::Span::Link { label, .. } => label.as_str(),
            _ => "",
        })
        .collect();
    let width = ui.calc_text_size(&text)[0];
    let available = ui.content_region_avail()[0];
    if width >= available {
        return;
    }
    let shift = match alignment {
        markdown::Alignment::Center => (available - width) / 2.0,
        markdown::Alignment::Right => available - width,
        markdown::Alignment::Left => 0.0,
    };
    let pos = ui.cursor_pos();
    ui.set_cursor_pos([pos[0] + shift, pos[1]]);
}

fn draw_markdown(
    ui: &Ui,
    fonts: &Fonts,
    theme: &crate::theme::Theme,
    content: &str,
    revealed_spoilers: &mut std::collections::HashSet<usize>,
    id_seed: &str,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    mentions: &mut MentionCtx,
) {
    let blocks = markdown::parse(content);
    for (block_idx, block) in blocks.iter().enumerate() {
        match block {
            Block::Paragraph(spans) => draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, mentions),
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
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, mentions);
                ui.set_window_font_scale(1.0);
                token.pop();
            }
            Block::Subtext(spans) => {
                let color = ui.push_style_color(imgui::StyleColor::Text, theme.text_muted);
                ui.set_window_font_scale(0.85);
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, mentions);
                ui.set_window_font_scale(1.0);
                color.pop();
            }
            Block::Blockquote { depth, spans } => {
                let start = ui.cursor_screen_pos();
                let indent = QUOTE_BAR_INDENT * *depth as f32;
                ui.set_cursor_screen_pos([start[0] + indent, start[1]]);
                let token = ui.push_style_color(imgui::StyleColor::Text, theme.text_muted);
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, mentions);
                token.pop();
                let bottom = ui.item_rect_max()[1].max(start[1] + ui.text_line_height());
                for level in 0..*depth {
                    let x = start[0] + QUOTE_BAR_INDENT * level as f32 + 2.0;
                    ui.get_window_draw_list()
                        .add_rect([x, start[1]], [x + 3.0, bottom], theme.border)
                        .filled(true)
                        .rounding(1.5)
                        .build();
                }
            }
            Block::ListItem { depth, marker, spans } => {
                let start = ui.cursor_screen_pos();
                let indent = LIST_INDENT * (*depth as f32 + 1.0);
                ui.set_cursor_screen_pos([start[0] + indent, start[1]]);
                let bullet = match marker {
                    markdown::ListMarker::Ordered(n) => format!("{n}."),
                    markdown::ListMarker::Bullet => bullet_glyph(*depth).to_string(),
                };
                let token = ui.push_style_color(imgui::StyleColor::Text, theme.text_muted);
                ui.text(&bullet);
                token.pop();
                ui.same_line_with_spacing(0.0, 6.0);
                draw_spans(ui, fonts, theme, spans, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, mentions);
            }
            Block::Blank => ui.dummy([0.0, ui.text_line_height() - ui.clone_style().item_spacing[1]]),
            Block::Table(table) => {
                draw_markdown_table(ui, fonts, theme, table, revealed_spoilers, id_seed, block_idx, textures, cmd_tx, mentions);
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
    mentions: &mut MentionCtx,
) {
    let line_start = ui.cursor_pos()[0];
    for (span_idx, span) in spans.iter().enumerate() {
        if span_idx > 0 {
            ui.same_line_with_spacing(0.0, 0.0);
        }
        match span {
            Span::Text { text, style } => {
                if style.spoiler {
                    let key = spoiler_key(id_seed, block_idx, span_idx);
                    if revealed_spoilers.contains(&key) {
                        draw_styled_text(ui, fonts, theme, text, *style, line_start);
                    } else {
                        flow_wrap(ui, ui.calc_text_size("spoiler")[0] + ui.clone_style().frame_padding[0] * 2.0, line_start);
                        let bg = ui.push_style_color(imgui::StyleColor::Button, theme.text_muted);
                        if ui.button(format!("spoiler##{key}")) {
                            revealed_spoilers.insert(key);
                        }
                        bg.pop();
                    }
                    continue;
                }
                draw_styled_text(ui, fonts, theme, text, *style, line_start);
            }
            Span::Link { label, url } => {
                let shown = if label == url && !crate::invites::is_invite_url(url) { elide_url(label) } else { label.clone() };
                if clickable_text_wrapping(ui, &format!("link_{id_seed}_{block_idx}_{span_idx}"), &shown, theme.accent, line_start) {
                    let _ = open::that(url);
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(elide_url(url));
                }
            }
            Span::CustomEmoji { name, id, animated } => {
                flow_wrap(ui, EMOJI_SIZE, line_start);
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
                flow_wrap(ui, EMOJI_SIZE, line_start);
                if !draw_unicode_emoji(ui, textures, *ch, [EMOJI_SIZE, EMOJI_SIZE]) {
                    ui.dummy([EMOJI_SIZE, EMOJI_SIZE]);
                }
            }
            Span::Mention { user_id } => {
                let name = mentions.known_users.get(user_id).map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()));
                if name.is_none() && mentions.profiles_requested.insert(user_id.clone()) {
                    let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: user_id.clone() });
                }
                mention_pill(ui, fonts, theme, &format!("@{}", name.as_deref().unwrap_or(user_id)), line_start);
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!("@{}", mentions.known_users.get(user_id).map(|u| u.username.as_str()).unwrap_or(user_id)));
                }
            }
            Span::ChannelMention { channel_id } => {
                let label = match mentions.channel_names.get(channel_id) {
                    Some(name) => format!("#{name}"),
                    None => format!("#{channel_id}"),
                };
                mention_pill(ui, fonts, theme, &label, line_start);
            }
            Span::RoleMention { role_id } => {
                let (label, color) = match mentions.role_names.get(role_id) {
                    Some((name, color)) => (format!("@{name}"), color.unwrap_or(theme.accent)),
                    None => ("@unknown-role".to_string(), theme.accent),
                };
                mention_pill_colored(ui, fonts, &label, color, line_start);
            }
            Span::EveryoneMention { here } => {
                mention_pill(ui, fonts, theme, if *here { "@here" } else { "@everyone" }, line_start);
            }
            Span::Timestamp { unix, style } => {
                let label = format_discord_timestamp(*unix, *style);
                flow_wrap(ui, ui.calc_text_size(&label)[0] + ui.clone_style().frame_padding[0] * 2.0, line_start);
                let token = ui.push_style_color(imgui::StyleColor::Button, theme.panel);
                ui.small_button(label);
                token.pop();
                if ui.is_item_hovered() {
                    ui.tooltip_text(format_discord_timestamp(*unix, markdown::TimestampStyle::LongDateTime));
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

const REACTION_EMOJI_SIZE: f32 = 20.0;
const REACTION_PILL_PAD_X: f32 = 6.0;
const REACTION_PILL_PAD_Y: f32 = 2.0;
const REACTION_PILL_GAP: f32 = 4.0;

fn reaction_pill_width(count_width: f32) -> f32 {
    REACTION_PILL_PAD_X * 2.0 + REACTION_EMOJI_SIZE + REACTION_PILL_GAP + count_width
}

fn draw_reaction_row(
    ui: &Ui,
    theme: &crate::theme::Theme,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    msg: &rustcord_client::models::Message,
) {
    if msg.reactions.is_empty() {
        return;
    }
    ui.dummy([0.0, 2.0]);
    let max_x = ui.window_content_region_max()[0];
    for (idx, reaction) in msg.reactions.iter().enumerate() {
        let count = reaction.count.to_string();
        let pill_width = reaction_pill_width(ui.calc_text_size(&count)[0]);
        let pill_height = REACTION_EMOJI_SIZE + REACTION_PILL_PAD_Y * 2.0;
        if idx > 0 {
            ui.same_line();
            if ui.cursor_pos()[0] + pill_width > max_x {
                ui.new_line();
            }
        }
        let bg = if reaction.me {
            [theme.accent[0], theme.accent[1], theme.accent[2], 0.35]
        } else {
            [theme.text_muted[0], theme.text_muted[1], theme.text_muted[2], 0.15]
        };
        let bg_token = ui.push_style_color(imgui::StyleColor::ChildBg, bg);
        let id = format!("reaction_{}_{idx}", msg.id);
        let rounding = ui.push_style_var(imgui::StyleVar::ChildRounding(pill_height / 2.0));
        let padding = ui.push_style_var(imgui::StyleVar::WindowPadding([REACTION_PILL_PAD_X, REACTION_PILL_PAD_Y]));
        let spacing = ui.push_style_var(imgui::StyleVar::ItemSpacing([REACTION_PILL_GAP, 0.0]));
        ui.child_window(&id)
            .size([pill_width, pill_height])
            .always_use_window_padding(true)
            .build(|| {
                let row_top = ui.cursor_pos()[1];
                match (&reaction.emoji.id, &reaction.emoji.name) {
                    (Some(emoji_id), _) => {
                        let ext = if reaction.emoji.animated { "gif" } else { "png" };
                        let url = format!("https://cdn.discordapp.com/emojis/{emoji_id}.{ext}?size=32");
                        if let Some((texture_id, _)) = textures.get(&url) {
                            imgui::Image::new(texture_id, [REACTION_EMOJI_SIZE, REACTION_EMOJI_SIZE]).build(ui);
                        } else {
                            textures.request(&url, cmd_tx);
                            ui.dummy([REACTION_EMOJI_SIZE, REACTION_EMOJI_SIZE]);
                        }
                    }
                    (None, Some(name)) => {
                        let ch = name.chars().next().unwrap_or('?');
                        if !draw_unicode_emoji(ui, textures, ch, [REACTION_EMOJI_SIZE, REACTION_EMOJI_SIZE]) {
                            ui.text(name);
                        }
                    }
                    (None, None) => ui.dummy([REACTION_EMOJI_SIZE, REACTION_EMOJI_SIZE]),
                }
                ui.same_line();
                let text_y = row_top + (REACTION_EMOJI_SIZE - ui.text_line_height()) / 2.0;
                ui.set_cursor_pos([ui.cursor_pos()[0], text_y]);
                ui.text_colored(if reaction.me { theme.accent } else { theme.text_muted }, &count);
            });
        spacing.pop();
        padding.pop();
        rounding.pop();
        bg_token.pop();
        if ui.is_item_clicked() {
            let _ = cmd_tx.send(UiCommand::ToggleReaction {
                channel_id: msg.channel_id.clone(),
                message_id: msg.id.clone(),
                emoji: reaction.emoji.reaction_path(),
                add: !reaction.me,
            });
        }
        if ui.is_item_hovered() {
            let label = reaction.emoji.name.clone().unwrap_or_default();
            ui.tooltip_text(format!(":{label}: \u{00d7} {}", reaction.count));
        }
    }
}

fn last_fitting(candidates: &[usize], fits: impl Fn(usize) -> bool) -> Option<usize> {
    let count = candidates.partition_point(|&end| fits(end));
    count.checked_sub(1).map(|index| candidates[index])
}

fn line_break(text: &str, avail: f32, at_line_start: bool, measure: impl Fn(&str) -> f32) -> Option<usize> {
    if measure(text) <= avail {
        return Some(text.len());
    }
    let fits = |end: usize| measure(&text[..end]) <= avail;
    let spaces: Vec<usize> = text.char_indices().filter(|&(idx, ch)| idx > 0 && ch.is_whitespace()).map(|(idx, _)| idx).collect();
    let best = last_fitting(&spaces, &fits);
    if best.is_some() || !at_line_start {
        return best;
    }
    let boundaries: Vec<usize> = text.char_indices().skip(1).map(|(idx, _)| idx).collect();
    Some(last_fitting(&boundaries, &fits).unwrap_or_else(|| text.chars().next().map_or(text.len(), char::len_utf8)))
}

fn flow_wrap(ui: &Ui, width: f32, line_start: f32) {
    let cursor = ui.cursor_pos()[0];
    if cursor > line_start && cursor + width > ui.content_region_max()[0] {
        ui.new_line();
        ui.set_cursor_pos([line_start, ui.cursor_pos()[1]]);
    }
}

fn draw_styled_text(ui: &Ui, fonts: &Fonts, theme: &crate::theme::Theme, text: &str, style: Style, line_start: f32) {
    let font_token = ui.push_font(if style.code { fonts.mono } else { fonts.pick(style) });
    let color_token = style.code.then(|| ui.push_style_color(imgui::StyleColor::Text, theme.accent));
    let max_x = ui.content_region_max()[0];
    let mut rest = text;
    loop {
        let at_line_start = ui.cursor_pos()[0] <= line_start + 0.5;
        let avail = (max_x - ui.cursor_pos()[0]).max(0.0);
        let Some(end) = line_break(rest, avail, at_line_start, |t| ui.calc_text_size(t)[0]) else {
            ui.new_line();
            ui.set_cursor_pos([line_start, ui.cursor_pos()[1]]);
            rest = rest.trim_start();
            continue;
        };
        ui.text(rest[..end].trim_end());
        if style.strikethrough || style.underline {
            let [x, y] = ui.item_rect_min();
            let [x2, y2] = ui.item_rect_max();
            let line_y = if style.strikethrough { (y + y2) / 2.0 } else { y2 };
            ui.get_window_draw_list().add_line([x, line_y], [x2, line_y], theme.text).build();
        }
        rest = rest[end..].trim_start();
        if rest.is_empty() {
            break;
        }
        ui.set_cursor_pos([line_start, ui.cursor_pos()[1]]);
    }
    if let Some(t) = color_token {
        t.pop();
    }
    font_token.pop();
}

fn spoiler_key(id_seed: &str, block_idx: usize, span_idx: usize) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id_seed.hash(&mut hasher);
    block_idx.hash(&mut hasher);
    span_idx.hash(&mut hasher);
    hasher.finish() as usize
}

fn is_video_attachment(content_type: Option<&str>, filename: &str) -> bool {
    if let Some(ct) = content_type {
        if ct.starts_with("video/") {
            return true;
        }
    }
    let ext = filename.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    matches!(ext.as_str(), "mp4" | "webm" | "mov" | "mkv")
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
const MESSAGE_GAP: f32 = 10.0;
const CHANNEL_ROW_HEIGHT: f32 = 26.0;
const RAIL_WIDTH: f32 = 72.0;
const RAIL_ICON_SIZE: [f32; 2] = [48.0, 48.0];
const RAIL_PADDING_TOP: f32 = 12.0;
const USER_BAR_HEIGHT: f32 = 44.0;
const VOICE_PANEL_ROW: f32 = 38.0;
const VOICE_PANEL_BUTTON: f32 = 32.0;
const VOICE_STATUS_BAR_HEIGHT: f32 = if cfg!(feature = "screenshare") { VOICE_PANEL_ROW + 8.0 + VOICE_PANEL_BUTTON + 6.0 } else { VOICE_PANEL_ROW + 6.0 };
const HEADER_HEIGHT: f32 = 48.0;
const SIDEBAR_MIN_WIDTH: f32 = 160.0;
const SIDEBAR_MAX_WIDTH: f32 = 480.0;
const SPLITTER_WIDTH: f32 = 6.0;
const MEMBERS_PANEL_WIDTH: f32 = 240.0;
const PROFILE_PANEL_WIDTH: f32 = 300.0;
const SEARCH_PANEL_WIDTH: f32 = 340.0;

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

fn truncate_measured(text: &str, max_width: f32, measure: impl Fn(&str) -> f32) -> Option<String> {
    if measure(text) <= max_width {
        return None;
    }
    let ellipsis = '\u{2026}';
    let budget = max_width - measure(&ellipsis.to_string());
    if budget <= 0.0 {
        return Some(ellipsis.to_string());
    }
    let mut out = String::new();
    for ch in text.chars() {
        out.push(ch);
        if measure(&out) > budget {
            out.pop();
            break;
        }
    }
    out.push(ellipsis);
    Some(out)
}

fn truncate_to_width(ui: &Ui, text: &str, max_width: f32) -> Option<String> {
    truncate_measured(text, max_width, |s| ui.calc_text_size(s)[0])
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

fn clickable_text_wrapping(ui: &Ui, id: &str, text: &str, color: [f32; 4], line_start: f32) -> bool {
    flow_wrap(ui, ui.calc_text_size(text)[0], line_start);
    clickable_text(ui, id, text, color)
}

fn mention_pill(ui: &Ui, fonts: &Fonts, theme: &crate::theme::Theme, label: &str, line_start: f32) {
    mention_pill_colored(ui, fonts, label, theme.accent, line_start);
}

fn mention_pill_colored(ui: &Ui, fonts: &Fonts, label: &str, color: [f32; 4], line_start: f32) {
    flow_wrap(ui, ui.calc_text_size(label)[0] + ui.clone_style().frame_padding[0] * 2.0, line_start);
    let bg = ui.push_style_color(imgui::StyleColor::Button, [color[0], color[1], color[2], 0.25]);
    let text = ui.push_style_color(imgui::StyleColor::Text, color);
    let font = ui.push_font(fonts.bold);
    ui.small_button(label);
    font.pop();
    text.pop();
    bg.pop();
}

fn format_discord_timestamp(unix: i64, style: markdown::TimestampStyle) -> String {
    use markdown::TimestampStyle as T;
    let Some(dt) = chrono::DateTime::from_timestamp(unix, 0) else {
        return format!("<invalid timestamp {unix}>");
    };
    let local = dt.with_timezone(&chrono::Local);
    match style {
        T::ShortTime => local.format("%H:%M").to_string(),
        T::LongTime => local.format("%H:%M:%S").to_string(),
        T::ShortDate => local.format("%d/%m/%Y").to_string(),
        T::LongDate => local.format("%-d %B %Y").to_string(),
        T::ShortDateTime => local.format("%-d %B %Y %H:%M").to_string(),
        T::LongDateTime => local.format("%A, %-d %B %Y %H:%M").to_string(),
        T::Relative => format_relative(unix),
    }
}

fn format_relative(unix: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let delta = unix - now;
    let secs = delta.unsigned_abs();
    let (n, unit) = match secs {
        0..=44 => (secs.max(1), "second"),
        45..=2699 => ((secs + 30) / 60, "minute"),
        2700..=86_399 => ((secs + 1800) / 3600, "hour"),
        86_400..=2_591_999 => ((secs + 43_200) / 86_400, "day"),
        2_592_000..=31_535_999 => ((secs + 1_296_000) / 2_592_000, "month"),
        _ => ((secs + 15_768_000) / 31_536_000, "year"),
    };
    let n = n.max(1);
    let plural = if n == 1 { "" } else { "s" };
    if delta <= 0 {
        format!("{n} {unit}{plural} ago")
    } else {
        format!("in {n} {unit}{plural}")
    }
}

fn elide_url(url: &str) -> String {
    const MAX: usize = 72;
    let trimmed = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX - 20).collect();
    let tail: String = {
        let chars: Vec<char> = trimmed.chars().collect();
        chars[chars.len() - 16..].iter().collect()
    };
    format!("{head}\u{2026}{tail}")
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
    activity_emoji: Option<&rustcord_client::models::ActivityEmoji>,
    row_bg_color: [f32; 4],
    name_color: Option<[f32; 4]>,
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
    let has_subtitle = activity.is_some_and(|a| !a.is_empty()) || activity_emoji.is_some();
    let text_y = if has_subtitle {
        row_start[1] + 3.0
    } else {
        row_start[1] + row_height / 2.0 - ui.text_line_height() / 2.0
    };
    ui.set_cursor_pos([text_x, text_y]);
    let name_clicked = clickable_text(
        ui,
        &format!("name_row_{}", user.id),
        name,
        name_color.unwrap_or_else(|| ui.style_color(imgui::StyleColor::Text)),
    );
    if let Some(tag) = user.server_tag() {
        ui.same_line_with_spacing(0.0, 6.0);
        draw_server_tag(ui, textures, cmd_tx, tag, tag.badge_url(32).as_deref());
    }
    if let Some(platform) = platform {
        ui.same_line();
        let token = ui.push_style_color(imgui::StyleColor::Text, [0.6, 0.62, 0.66, 1.0]);
        ui.set_window_font_scale(0.8);
        ui.text(platform);
        ui.set_window_font_scale(1.0);
        token.pop();
    }

    if has_subtitle {
        ui.set_cursor_pos([text_x, text_y + ui.text_line_height()]);
        let subtitle_size = ui.text_line_height() * 0.8;
        if let Some(emoji) = activity_emoji {
            draw_status_emoji(ui, textures, cmd_tx, emoji, subtitle_size);
            ui.same_line_with_spacing(0.0, 4.0);
        }
        let token = ui.push_style_color(imgui::StyleColor::Text, [0.6, 0.62, 0.66, 1.0]);
        ui.set_window_font_scale(0.8);
        let text = activity.unwrap_or_default();
        let room = ui.content_region_avail()[0] - 4.0;
        ui.text(truncate_to_width(ui, text, room).as_deref().unwrap_or(text));
        ui.set_window_font_scale(1.0);
        token.pop();
    }

    ui.set_cursor_pos([row_start[0], row_start[1] + row_height + ui.clone_style().item_spacing[1]]);
    UserRowResult { row_clicked, avatar_clicked: avatar_clicked || name_clicked, right_clicked }
}

fn draw_server_tag(
    ui: &Ui,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    tag: &rustcord_client::models::PrimaryGuild,
    badge_url: Option<&str>,
) {
    const BADGE: f32 = 12.0;
    let Some(label) = tag.visible_tag() else { return };
    let line = ui.text_line_height();
    ui.set_window_font_scale(0.75);
    let text_size = ui.calc_text_size(label);
    let badge_room = if badge_url.is_some() { BADGE + 3.0 } else { 0.0 };
    let pad = 4.0;
    let size = [pad * 2.0 + badge_room + text_size[0], text_size[1] + 2.0];
    let min = ui.cursor_screen_pos();
    let min = [min[0], min[1] + (line - size[1]) / 2.0];
    let muted = ui.style_color(imgui::StyleColor::TextDisabled);
    let badge = badge_url.and_then(|url| match textures.get(url) {
        Some((texture, _)) => Some(texture),
        None => {
            textures.request(url, cmd_tx);
            None
        }
    });
    let dl = ui.get_window_draw_list();
    dl.add_rect(min, [min[0] + size[0], min[1] + size[1]], [muted[0], muted[1], muted[2], 0.2]).filled(true).rounding(4.0).build();
    if let Some(texture) = badge {
        let y = min[1] + (size[1] - BADGE) / 2.0;
        dl.add_image(texture, [min[0] + pad, y], [min[0] + pad + BADGE, y + BADGE]).build();
    }
    dl.add_text([min[0] + pad + badge_room, min[1] + 1.0], ui.style_color(imgui::StyleColor::Text), label);
    ui.set_window_font_scale(1.0);
    ui.dummy([size[0], line]);
}

fn draw_status_emoji(
    ui: &Ui,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    emoji: &rustcord_client::models::ActivityEmoji,
    size: f32,
) {
    match (&emoji.id, &emoji.name) {
        (Some(id), _) => {
            let url = format!("https://cdn.discordapp.com/emojis/{id}.{}?size=32", if emoji.animated { "gif" } else { "png" });
            match textures.get(&url) {
                Some((texture, _)) => imgui::Image::new(texture, [size, size]).build(ui),
                None => {
                    textures.request(&url, cmd_tx);
                    ui.dummy([size, size]);
                }
            }
        }
        (None, Some(name)) => match name.chars().next().filter(|c| crate::emoji::is_emoji(*c)) {
            Some(ch) => {
                if !draw_unicode_emoji(ui, textures, ch, [size, size]) {
                    ui.dummy([size, size]);
                }
            }
            None => ui.dummy([size, size]),
        },
        (None, None) => ui.dummy([size, size]),
    }
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

fn draw_rail_mention_badge(ui: &Ui, color: [f32; 4], icon_min: [f32; 2], icon_size: [f32; 2], count: u32) {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    let text_size = ui.calc_text_size(&text);
    let height = 16.0;
    let width = (text_size[0] + 8.0).max(height);
    let min = [icon_min[0] + icon_size[0] - width + 2.0, icon_min[1] + icon_size[1] - height + 2.0];
    let max = [min[0] + width, min[1] + height];
    let draw_list = ui.get_window_draw_list();
    draw_list
        .add_rect([min[0] - 2.0, min[1] - 2.0], [max[0] + 2.0, max[1] + 2.0], [0.13, 0.14, 0.17, 1.0])
        .filled(true)
        .rounding(height / 2.0 + 2.0)
        .build();
    draw_list.add_rect(min, max, color).filled(true).rounding(height / 2.0).build();
    draw_list.add_text(
        [min[0] + (width - text_size[0]) / 2.0, min[1] + (height - text_size[1]) / 2.0],
        [1.0, 1.0, 1.0, 1.0],
        &text,
    );
}

fn draw_mention_badge(ui: &Ui, color: [f32; 4], row_screen_pos: [f32; 2], row_height: f32, count: u32) {
    let text = if count > 99 { "99+".to_string() } else { count.to_string() };
    let text_size = ui.calc_text_size(&text);
    let height = (ui.text_line_height() + 2.0).max(16.0);
    let width = (text_size[0] + 10.0).max(height);
    let right = row_screen_pos[0] + ui.content_region_avail()[0];
    let min = [right - width, row_screen_pos[1] + (row_height - height) / 2.0];
    let max = [right, min[1] + height];
    ui.get_window_draw_list().add_rect(min, max, color).filled(true).rounding(height / 2.0).build();
    ui.get_window_draw_list().add_text(
        [min[0] + (width - text_size[0]) / 2.0, min[1] + (height - text_size[1]) / 2.0],
        [1.0, 1.0, 1.0, 1.0],
        &text,
    );
}

fn jump_highlight(accent: [f32; 4]) -> [f32; 4] {
    [accent[0], accent[1], accent[2], 0.22]
}

fn draw_unread_dot(ui: &Ui, row_screen_pos: [f32; 2], row_height: f32) {
    let center = [row_screen_pos[0] - 8.0, row_screen_pos[1] + row_height / 2.0];
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
    folder_guild_context: &mut Option<(usize, String)>,
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
        let mentions = state.guild_mention_count(guild_id);
        if mentions > 0 {
            draw_rail_mention_badge(ui, state.theme.mention_badge(), icon_min, FOLDER_ITEM_SIZE, mentions);
        } else if state.guild_has_unread(guild_id) {
            draw_unread_badge(ui, icon_min);
        }
        if clicked {
            *select_guild = Some(guild_id.clone());
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(&guild.name);
            if ui.is_mouse_clicked(imgui::MouseButton::Right) {
                *folder_guild_context = Some((folder_idx, guild_id.clone()));
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
    let mut folder_guild_context: Option<(usize, String)> = None;
    let mut open_guild_context: Option<(String, Option<usize>)> = None;
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
                let mentions = state.guild_mention_count(&guild.id);
                if mentions > 0 {
                    draw_rail_mention_badge(ui, state.theme.mention_badge(), icon_min, RAIL_ICON_SIZE, mentions);
                } else if state.guild_has_unread(&guild.id) {
                    draw_unread_badge(ui, icon_min);
                }
                if clicked {
                    select_guild = Some(guild.id.clone());
                }
                if ui.is_item_hovered() {
                    ui.tooltip_text(&guild.name);
                    if ui.is_mouse_clicked(imgui::MouseButton::Right) {
                        open_guild_context = Some((guild.id.clone(), None));
                    }
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
                        &mut folder_guild_context,
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
    if let Some((folder_idx, guild_id)) = folder_guild_context {
        open_guild_context = Some((guild_id, Some(folder_idx)));
    }
    if let Some(context) = open_guild_context {
        state.guild_context = Some(context);
        ui.open_popup("guild_ctx");
    }
    ui.popup("guild_ctx", || {
        let Some((guild_id, folder_idx)) = state.guild_context.clone() else { return };
        if let Some(guild) = state.guilds.iter().find(|g| g.id == guild_id) {
            ui.text_disabled(&guild.name);
            ui.separator();
        }
        if let Some(mute) = mute_menu(ui, "server", state.is_guild_muted(&guild_id, chrono::Utc::now())) {
            let patch = crate::app_state::mute_patch(mute.is_some(), mute.unwrap_or(-1), chrono::Utc::now());
            let _ = cmd_tx.send(UiCommand::UpdateGuildSettings { guild_key: guild_id.clone(), patch });
        }
        if let Some(level) = notification_level_menu(ui, state.guild_notification_level(&guild_id), false) {
            let patch = serde_json::json!({ "message_notifications": level });
            let _ = cmd_tx.send(UiCommand::UpdateGuildSettings { guild_key: guild_id.clone(), patch });
        }
        if let Some(folder_idx) = folder_idx {
            ui.separator();
            if ui.selectable("Remove from folder") {
                remove_from_folder = Some((folder_idx, guild_id));
            }
        }
    });
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
const FREQUENT_TAB: usize = 0;
const CUSTOM_TAB: usize = 1;
const FIRST_CATEGORY_TAB: usize = 2;

fn custom_emoji_cell(ui: &Ui, emoji: &rustcord_client::models::Emoji, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) -> Option<String> {
    let url = rustcord_client::cdn::custom_emoji_url(emoji, 32);
    let mut insert = None;
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
    insert
}

fn unicode_emoji_cell(ui: &Ui, textures: &TextureCache, emoji: &'static emojis::Emoji) -> Option<String> {
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
    if ui.is_item_hovered() {
        ui.tooltip_text(format!(":{shortcode}:"));
    }
    picked.then(|| emoji.as_str().to_string())
}

fn draw_custom_emoji_grid(
    ui: &Ui,
    theme: &crate::theme::Theme,
    custom: &[(&rustcord_client::models::Guild, &rustcord_client::models::Emoji)],
    filter: &str,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
) -> Option<String> {
    let mut insert = None;
    let mut col = 0;
    let mut current_guild: Option<&str> = None;
    for (guild, emoji) in custom {
        if !filter.is_empty() && !emoji.name.to_lowercase().contains(filter) {
            continue;
        }
        if filter.is_empty() && current_guild != Some(guild.id.as_str()) {
            if current_guild.is_some() {
                ui.spacing();
            }
            current_guild = Some(guild.id.as_str());
            ui.text_colored(theme.text_muted, &guild.name);
            col = 0;
        }
        if col > 0 {
            ui.same_line();
        }
        if let Some(picked) = custom_emoji_cell(ui, emoji, textures, cmd_tx) {
            insert = Some(picked);
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
        if let Some(picked) = unicode_emoji_cell(ui, textures, emoji) {
            insert = Some(picked);
        }
        col = (col + 1) % EMOJI_GRID_COLUMNS;
    }
    insert
}

fn draw_frequent_emoji_grid(ui: &Ui, state: &AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) -> Option<String> {
    let mut insert = None;
    for (idx, emoji) in state.frequent_emojis().iter().enumerate() {
        if idx % EMOJI_GRID_COLUMNS != 0 {
            ui.same_line();
        }
        let picked = match emoji {
            crate::app_state::FrequentEmoji::Custom(emoji) => custom_emoji_cell(ui, emoji, textures, cmd_tx),
            crate::app_state::FrequentEmoji::Unicode(emoji) => unicode_emoji_cell(ui, textures, emoji),
        };
        if picked.is_some() {
            insert = picked;
        }
    }
    insert
}

fn sidebar_tab(ui: &Ui, theme: &crate::theme::Theme, label: &str, id: &str, width: f32, selected: bool) -> bool {
    let token = selected.then(|| ui.push_style_color(imgui::StyleColor::Button, theme.accent));
    let clicked = ui.button_with_size(format!("{label}##{id}"), [width, 0.0]);
    drop(token);
    if ui.is_item_hovered() && ui.calc_text_size(label)[0] > width - 8.0 {
        ui.tooltip_text(label);
    }
    clicked
}

fn draw_emoji_picker(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let mut insert: Option<String> = None;
    let mut open = false;
    ui.popup("emoji_picker", || {
        open = true;
        ui.set_next_item_width(EMOJI_PICKER_WIDTH);
        ui.input_text("##emoji_search", &mut state.emoji_picker_filter).hint("Search emoji...").build();
        let filter = state.emoji_picker_filter.to_lowercase();
        ui.separator();

        let has_frequent = !state.frequently_used.emojis.is_empty();
        let has_custom = !state.guild_emojis.values().all(Vec::is_empty);
        if filter.is_empty() {
            if state.emoji_picker_category == FREQUENT_TAB && !has_frequent {
                state.emoji_picker_category = CUSTOM_TAB;
            }
            if state.emoji_picker_category == CUSTOM_TAB && !has_custom {
                state.emoji_picker_category = FIRST_CATEGORY_TAB;
            }
            let tab_width = EMOJI_SIDEBAR_WIDTH - 16.0;
            let theme = state.theme.clone();
            ui.child_window("emoji_sidebar").size([EMOJI_SIDEBAR_WIDTH, EMOJI_PICKER_HEIGHT]).build(|| {
                if has_frequent && sidebar_tab(ui, &theme, "Frequent", "emoji_tab", tab_width, state.emoji_picker_category == FREQUENT_TAB) {
                    state.emoji_picker_category = FREQUENT_TAB;
                }
                if has_custom && sidebar_tab(ui, &theme, "Custom", "emoji_tab", tab_width, state.emoji_picker_category == CUSTOM_TAB) {
                    state.emoji_picker_category = CUSTOM_TAB;
                }
                if has_frequent || has_custom {
                    ui.separator();
                }
                for (idx, (label, _)) in EMOJI_CATEGORIES.iter().enumerate() {
                    let tab = idx + FIRST_CATEGORY_TAB;
                    if sidebar_tab(ui, &theme, label, "emoji_tab", tab_width, state.emoji_picker_category == tab) {
                        state.emoji_picker_category = tab;
                    }
                }
            });
            ui.same_line();
        }

        ui.child_window("emoji_grid")
            .size([EMOJI_PICKER_WIDTH - if filter.is_empty() { EMOJI_SIDEBAR_WIDTH + 8.0 } else { 0.0 }, EMOJI_PICKER_HEIGHT])
            .build(|| {
                if filter.is_empty() && state.emoji_picker_category == FREQUENT_TAB {
                    insert = draw_frequent_emoji_grid(ui, state, textures, cmd_tx);
                    return;
                }

                let custom = state.available_emojis();
                if filter.is_empty() && state.emoji_picker_category == CUSTOM_TAB {
                    insert = draw_custom_emoji_grid(ui, &state.theme, &custom, &filter, textures, cmd_tx);
                    return;
                }

                if !filter.is_empty() && !custom.is_empty() {
                    if let Some(picked) = draw_custom_emoji_grid(ui, &state.theme, &custom, &filter, textures, cmd_tx) {
                        insert = Some(picked);
                    }
                    if custom.iter().any(|(_, e)| e.name.to_lowercase().contains(&filter)) {
                        ui.separator();
                    }
                }

                let group = filter.is_empty().then(|| {
                    EMOJI_CATEGORIES[state.emoji_picker_category.saturating_sub(FIRST_CATEGORY_TAB).min(EMOJI_CATEGORIES.len() - 1)].1
                });
                if let Some(picked) = draw_unicode_emoji_grid(ui, textures, group, &filter) {
                    insert = Some(picked);
                }
            });
    });
    if !open && insert.is_none() {
        state.emoji_picker_target = None;
    }
    if let Some(glyph) = insert {
        state.record_emoji_use(&glyph);
        match state.emoji_picker_target.take() {
            Some((channel_id, message_id)) => {
                let _ = cmd_tx.send(UiCommand::ToggleReaction {
                    channel_id,
                    message_id,
                    emoji: reaction_emoji_path(&glyph),
                    add: true,
                });
            }
            None => {
                state.compose_buf.push_str(&glyph);
                state.focus_compose = true;
            }
        }
    }
}

const GIF_PICKER_WIDTH: f32 = 420.0;
const GIF_PICKER_HEIGHT: f32 = 380.0;
const GIF_CELL_HEIGHT: f32 = 120.0;
const STICKER_CELL: f32 = 80.0;
const STICKER_GRID_COLUMNS: usize = 4;
const STICKER_SIDEBAR_WIDTH: f32 = 120.0;
const MESSAGE_STICKER_SIZE: f32 = 160.0;

fn draw_image_fitted(ui: &Ui, texture_id: imgui::TextureId, size: [f32; 2], cell_min: [f32; 2], cell: [f32; 2]) {
    let scale = (cell[0] / size[0].max(1.0)).min(cell[1] / size[1].max(1.0));
    let drawn = [size[0] * scale, size[1] * scale];
    let p_min = [cell_min[0] + (cell[0] - drawn[0]) / 2.0, cell_min[1] + (cell[1] - drawn[1]) / 2.0];
    ui.get_window_draw_list().add_image(texture_id, p_min, [p_min[0] + drawn[0], p_min[1] + drawn[1]]).build();
}

fn draw_sticker_items(
    ui: &Ui,
    theme: &crate::theme::Theme,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    stickers: &[rustcord_client::models::StickerItem],
) {
    for sticker in stickers {
        match rustcord_client::cdn::sticker_url(&sticker.id, sticker.format_type, MESSAGE_STICKER_SIZE as u32) {
            Some(url) => match textures.get(&url) {
                Some((texture_id, size)) => {
                    ui.dummy([MESSAGE_STICKER_SIZE, MESSAGE_STICKER_SIZE]);
                    draw_image_fitted(ui, texture_id, size, ui.item_rect_min(), [MESSAGE_STICKER_SIZE, MESSAGE_STICKER_SIZE]);
                    if ui.is_item_hovered() {
                        ui.tooltip_text(&sticker.name);
                    }
                }
                None => {
                    textures.request(&url, cmd_tx);
                    ui.dummy([MESSAGE_STICKER_SIZE, MESSAGE_STICKER_SIZE]);
                }
            },
            None => ui.text_colored(theme.text_muted, format!("[sticker: {}]", sticker.name)),
        }
    }
}

fn draw_gif_picker(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let mut picked: Option<String> = None;
    ui.popup("gif_picker", || {
        if ui.is_window_appearing() {
            ui.set_keyboard_focus_here();
        }
        ui.set_next_item_width(GIF_PICKER_WIDTH);
        if ui.input_text("##gif_search", &mut state.gif_picker.query).hint("Search GIFs").build() {
            state.gif_picker.edited_at = Some(std::time::Instant::now());
        }
        if let Some(query) = state.gif_picker.due_query(std::time::Instant::now()) {
            state.gif_picker.requested = Some(query.clone());
            let _ = cmd_tx.send(UiCommand::SearchGifs { query });
        }
        ui.separator();
        ui.child_window("gif_grid").size([GIF_PICKER_WIDTH, GIF_PICKER_HEIGHT]).build(|| {
            let picker = &state.gif_picker;
            if let Some(error) = &picker.error {
                ui.text_colored(state.theme.text_muted, format!("Couldn't load GIFs: {error}"));
                return;
            }
            if picker.results.is_empty() {
                ui.text_disabled(if picker.loading() { "Loading..." } else { "No GIFs found." });
                return;
            }
            let spacing = ui.clone_style().item_spacing[0];
            let cell = [((ui.content_region_avail()[0] - spacing) / 2.0).max(40.0), GIF_CELL_HEIGHT];
            for (idx, gif) in picker.results.iter().enumerate() {
                if idx % 2 == 1 {
                    ui.same_line();
                }
                if ui.invisible_button(format!("##gif_{idx}"), cell) {
                    picked = Some(gif.url.clone());
                }
                let cell_min = ui.item_rect_min();
                let hovered = ui.is_item_hovered();
                let draw_list = ui.get_window_draw_list();
                draw_list.add_rect(cell_min, [cell_min[0] + cell[0], cell_min[1] + cell[1]], state.theme.panel).filled(true).rounding(4.0).build();
                drop(draw_list);
                match textures.get(gif.thumbnail()) {
                    Some((texture_id, size)) => draw_image_fitted(ui, texture_id, size, cell_min, cell),
                    None => textures.request(gif.thumbnail(), cmd_tx),
                }
                if hovered {
                    ui.get_window_draw_list()
                        .add_rect(cell_min, [cell_min[0] + cell[0], cell_min[1] + cell[1]], state.theme.accent)
                        .rounding(4.0)
                        .thickness(2.0)
                        .build();
                }
            }
        });
        if picked.is_some() {
            ui.close_current_popup();
        }
    });
    if let (Some(content), Some(channel_id)) = (picked, state.selected_channel.clone()) {
        let reply_to = state.reply_target.take().map(|m| m.id);
        let _ = cmd_tx.send(UiCommand::SendMessage { channel_id, content, reply_to });
        state.focus_compose = true;
    }
}

fn sticker_cell(ui: &Ui, sticker: &rustcord_client::models::Sticker, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) -> bool {
    let cell = [STICKER_CELL, STICKER_CELL];
    let url = rustcord_client::cdn::sticker_url(&sticker.id, sticker.format_type, STICKER_CELL as u32);
    let clicked = match url.as_deref().map(|url| (url, textures.get(url))) {
        Some((_, Some((texture_id, size)))) => {
            let clicked = ui.invisible_button(format!("##sticker_{}", sticker.id), cell);
            draw_image_fitted(ui, texture_id, size, ui.item_rect_min(), cell);
            clicked
        }
        Some((url, None)) => {
            textures.request(url, cmd_tx);
            ui.invisible_button(format!("##sticker_{}", sticker.id), cell)
        }
        None => ui.button_with_size(format!("{}##sticker_{}", sticker.name, sticker.id), cell),
    };
    if ui.is_item_hovered() {
        ui.tooltip_text(&sticker.name);
    }
    clicked
}

fn draw_sticker_picker(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let mut picked: Option<String> = None;
    ui.popup("sticker_picker", || {
        if ui.is_window_appearing() {
            ui.set_keyboard_focus_here();
        }
        let grid_width = STICKER_GRID_COLUMNS as f32 * (STICKER_CELL + ui.clone_style().item_spacing[0]) + 16.0;
        let filter = state.sticker_picker_filter.trim().to_string();
        let sidebar = if filter.is_empty() { STICKER_SIDEBAR_WIDTH + 8.0 } else { 0.0 };
        ui.set_next_item_width(grid_width + sidebar);
        ui.input_text("##sticker_search", &mut state.sticker_picker_filter).hint("Search stickers").build();
        ui.separator();
        let sections = state.sticker_sections(&filter);
        if sections.is_empty() {
            let loading = state.sticker_packs.is_empty() && state.sticker_packs_requested;
            ui.text_disabled(if loading { "Loading..." } else { "No stickers found." });
            return;
        }
        let selected = state
            .sticker_picker_section
            .as_deref()
            .and_then(|key| sections.iter().position(|s| s.key == key))
            .unwrap_or(0);
        let mut chosen_section = None;
        if filter.is_empty() {
            ui.child_window("sticker_sidebar").size([STICKER_SIDEBAR_WIDTH, GIF_PICKER_HEIGHT]).build(|| {
                for (idx, section) in sections.iter().enumerate() {
                    if idx > 0 && sections[idx - 1].key == crate::app_state::FREQUENT_SECTION {
                        ui.separator();
                    }
                    if sidebar_tab(ui, &state.theme, section.name, &format!("sticker_tab_{}", section.key), STICKER_SIDEBAR_WIDTH - 16.0, idx == selected) {
                        chosen_section = Some(section.key.clone());
                    }
                }
            });
            ui.same_line();
        }
        ui.child_window("sticker_grid").size([grid_width, GIF_PICKER_HEIGHT]).build(|| {
            let shown: &[crate::app_state::StickerSection] = if filter.is_empty() { &sections[selected..=selected] } else { &sections };
            for section in shown {
                ui.text_colored(state.theme.text_muted, section.name);
                for (idx, sticker) in section.stickers.iter().enumerate() {
                    if idx % STICKER_GRID_COLUMNS != 0 {
                        ui.same_line();
                    }
                    if sticker_cell(ui, sticker, textures, cmd_tx) {
                        picked = Some(sticker.id.clone());
                    }
                }
                ui.spacing();
            }
        });
        drop(sections);
        if chosen_section.is_some() {
            state.sticker_picker_section = chosen_section;
        }
        if picked.is_some() {
            ui.close_current_popup();
        }
    });
    if let (Some(sticker_id), Some(channel_id)) = (picked, state.selected_channel.clone()) {
        state.record_sticker_use(&sticker_id);
        let reply_to = state.reply_target.take().map(|m| m.id);
        let _ = cmd_tx.send(UiCommand::SendSticker { channel_id, sticker_id, reply_to });
        state.focus_compose = true;
    }
}

fn reaction_emoji_path(picked: &str) -> String {
    let Some(inner) = picked.strip_prefix('<').and_then(|r| r.strip_suffix('>')) else {
        return picked.to_string();
    };
    let body = inner.strip_prefix("a:").or_else(|| inner.strip_prefix(':'));
    match body.and_then(|rest| rest.split_once(':')) {
        Some((name, id)) if !name.is_empty() && !id.is_empty() => format!("{name}:{id}"),
        _ => picked.to_string(),
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
    let is_video = is_video_attachment(attachment.content_type.as_deref(), &attachment.filename);
    let is_audio = content_type.starts_with("audio/") || matches!(ext.as_str(), "mp3" | "ogg" | "wav" | "flac" | "m4a" | "opus");
    let (icon, open_label) = if is_video {
        (crate::icons::Icon::Film, "Play")
    } else if is_audio {
        (crate::icons::Icon::Music, "Play")
    } else if matches!(ext.as_str(), "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst") {
        (crate::icons::Icon::Package, "Open")
    } else if matches!(ext.as_str(), "txt" | "md" | "log" | "pdf" | "doc" | "docx") {
        (crate::icons::Icon::FileText, "Open")
    } else {
        (crate::icons::Icon::Folder, "Open")
    };
    ui.group(|| {
        draw_icon(ui, textures, icon, 28.0, ui.style_color(imgui::StyleColor::TextDisabled));
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

#[cfg(feature = "video")]
const VIDEO_CONTROL_ICONS: [crate::icons::Icon; 6] = [
    crate::icons::Icon::Play,
    crate::icons::Icon::Pause,
    crate::icons::Icon::Replay,
    crate::icons::Icon::Volume,
    crate::icons::Icon::VolumeOff,
    crate::icons::Icon::Maximize,
];

const CHROME_ICONS: [crate::icons::Icon; 2] = [crate::icons::Icon::Close, crate::icons::Icon::MessageCircle];

const HEADER_ICONS: [crate::icons::Icon; 4] =
    [crate::icons::Icon::Phone, crate::icons::Icon::Video, crate::icons::Icon::Users, crate::icons::Icon::Search];
const SCREENSHARE_ICON: crate::icons::Icon = crate::icons::Icon::MonitorUp;

#[cfg(feature = "video")]
fn draw_play_overlay(ui: &Ui, origin: [f32; 2], size: [f32; 2], hovered: bool) {
    let center = [origin[0] + size[0] / 2.0, origin[1] + size[1] / 2.0];
    let radius = (size[0].min(size[1]) * 0.12).clamp(18.0, 34.0);
    let draw_list = ui.get_window_draw_list();
    let shade = if hovered { 0.85 } else { 0.65 };
    draw_list.add_circle(center, radius, [0.0, 0.0, 0.0, shade]).filled(true).build();
    draw_list.add_circle(center, radius, [1.0, 1.0, 1.0, 0.9]).thickness(2.0).build();

    let tip = radius * 0.55;
    let half = radius * 0.45;
    draw_list
        .add_triangle(
            [center[0] - half * 0.6, center[1] - half],
            [center[0] - half * 0.6, center[1] + half],
            [center[0] + tip, center[1]],
            [1.0, 1.0, 1.0, 0.95],
        )
        .filled(true)
        .build();
}

#[cfg(feature = "video")]
fn format_timecode(secs: f64) -> String {
    let secs = if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 };
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(feature = "video")]
fn draw_video_controls(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    url: &str,
    id: &str,
    status: &crate::images::VideoStatus,
    width: f32,
) {
    if status.buffering {
        ui.text_disabled("Buffering...");
        ui.same_line();
    }
    const CONTROL_GAP: f32 = 6.0;
    const EMOJI_BUTTON_SIZE: f32 = 30.0;
    const VOLUME_SLIDER_WIDTH: f32 = 60.0;
    let row_top = ui.cursor_pos()[1];
    let centre = |ui: &Ui, height: f32| {
        let x = ui.cursor_pos()[0];
        ui.set_cursor_pos([x, row_top + (EMOJI_BUTTON_SIZE - height) / 2.0]);
    };
    let (play_icon, play_tip) = if status.ended {
        (VIDEO_CONTROL_ICONS[2], "Replay")
    } else if status.playing {
        (VIDEO_CONTROL_ICONS[1], "Pause")
    } else {
        (VIDEO_CONTROL_ICONS[0], "Play")
    };
    if icon_button(ui, textures, &format!("play_{id}"), play_icon, play_tip, None) {
        if status.ended {
            textures.request_video_seek(url, 0.0, cmd_tx);
        } else {
            textures.toggle_video_playback(url);
        }
    }
    ui.same_line_with_spacing(0.0, CONTROL_GAP);
    centre(ui, ui.text_line_height());
    ui.text(format!("{} / {}", format_timecode(status.position_secs), format_timecode(status.duration_secs)));
    ui.same_line_with_spacing(0.0, CONTROL_GAP);
    centre(ui, ui.frame_height());

    let fullscreen_width = EMOJI_BUTTON_SIZE + CONTROL_GAP;
    let audio_width = if status.has_audio {
        EMOJI_BUTTON_SIZE + CONTROL_GAP + VOLUME_SLIDER_WIDTH + CONTROL_GAP
    } else {
        0.0
    };
    let trailing = fullscreen_width + audio_width + CONTROL_GAP;
    let scrub_width = (width - ui.cursor_pos()[0] - trailing).max(60.0);
    let mut position = status.position_secs as f32;
    let duration = status.duration_secs.max(0.001) as f32;
    ui.set_next_item_width(scrub_width);
    let grab = ui.push_style_var(imgui::StyleVar::GrabMinSize(12.0));
    let rounding = ui.push_style_var(imgui::StyleVar::GrabRounding(6.0));
    if ui.slider_config(format!("##scrub_{id}"), 0.0f32, duration).display_format("").build(&mut position) {
        textures.request_video_seek(url, position as f64, cmd_tx);
    }
    rounding.pop();
    grab.pop();

    if status.has_audio {
        ui.same_line_with_spacing(0.0, CONTROL_GAP);
        centre(ui, EMOJI_BUTTON_SIZE);
        let (icon, tip) = if status.muted { (VIDEO_CONTROL_ICONS[4], "Unmute") } else { (VIDEO_CONTROL_ICONS[3], "Mute") };
        if icon_button(ui, textures, &format!("mute_{id}"), icon, tip, None) {
            textures.toggle_video_mute(url);
        }
        ui.same_line_with_spacing(0.0, CONTROL_GAP);
        centre(ui, ui.frame_height());
        let mut volume = status.volume;
        ui.set_next_item_width(VOLUME_SLIDER_WIDTH);
        let grab = ui.push_style_var(imgui::StyleVar::GrabMinSize(12.0));
        let rounding = ui.push_style_var(imgui::StyleVar::GrabRounding(6.0));
        if ui.slider_config(format!("##vol_{id}"), 0.0f32, 1.0f32).display_format("").build(&mut volume) {
            textures.set_video_volume(url, volume);
        }
        rounding.pop();
        grab.pop();
    }

    ui.same_line_with_spacing(0.0, CONTROL_GAP);
    centre(ui, EMOJI_BUTTON_SIZE);
    let fs_tip = if status.fullscreen { "Exit fullscreen" } else { "Fullscreen" };
    if icon_button(ui, textures, &format!("fs_{id}"), VIDEO_CONTROL_ICONS[5], fs_tip, None) {
        textures.toggle_video_fullscreen(url);
    }
}

#[cfg(feature = "video")]
fn draw_video_attachment(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    attachment: &rustcord_client::models::Attachment,
) {
    let url = &attachment.url;
    if !textures.is_video(url) {
        let natural = match (attachment.width, attachment.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => [w as f32, h as f32],
            _ => [MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT],
        };
        let poster = textures.poster(url, cmd_tx);
        ui.group(|| {
            match poster {
                crate::images::PosterState::Ready { texture_id, size } => {
                    let draw_size = scaled_image_size(size);
                    let origin = ui.cursor_screen_pos();
                    imgui::Image::new(texture_id, draw_size).build(ui);
                    let hovered = ui.is_item_hovered();
                    draw_play_overlay(ui, origin, draw_size, hovered);
                    if ui.is_item_clicked() {
                        textures.start_video(url, size, cmd_tx);
                    }
                }
                crate::images::PosterState::Loading => {
                    let draw_size = scaled_image_size(natural);
                    let origin = ui.cursor_screen_pos();
                    ui.dummy(draw_size);
                    ui.get_window_draw_list()
                        .add_rect(origin, [origin[0] + draw_size[0], origin[1] + draw_size[1]], [1.0, 1.0, 1.0, 0.04])
                        .filled(true)
                        .rounding(4.0)
                        .build();
                }
                crate::images::PosterState::Unavailable => {
                    ui.text(&attachment.filename);
                    if ui.small_button(format!("Play##play_{}", attachment.id)) {
                        textures.start_video(url, natural, cmd_tx);
                    }
                }
            }
            ui.text_disabled(human_size(attachment.size));
            ui.same_line();
            if ui.small_button(format!("Save##save_{}", attachment.id)) {
                let _ = cmd_tx.send(UiCommand::SaveAttachment { url: url.clone(), filename: attachment.filename.clone() });
            }
            ui.same_line();
            if ui.small_button(format!("Open##open_{}", attachment.id)) {
                let _ = open::that(url);
            }
        });
        return;
    }

    let Some(status) = textures.video_status(url) else { return };
    if status.fullscreen {
        ui.text_disabled(format!("{} (fullscreen)", attachment.filename));
        return;
    }

    ui.text(&attachment.filename);
    let size = match textures.get(url) {
        Some((texture_id, natural_size)) => {
            let size = scaled_image_size(natural_size);
            imgui::Image::new(texture_id, size).build(ui);
            if ui.is_item_clicked() {
                textures.toggle_video_playback(url);
            }
            size
        }
        None => {
            let size = scaled_image_size([MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT]);
            ui.dummy(size);
            ui.text_disabled("Buffering...");
            size
        }
    };
    draw_video_controls(ui, cmd_tx, textures, url, &attachment.id, &status, size[0]);
    if ui.small_button(format!("Close##stopvid_{}", attachment.id)) {
        let _ = cmd_tx.send(UiCommand::CloseVideo { url: url.clone() });
        textures.mark_video_stopped(url);
    }
}

const ATTACHMENT_TRAY_HEIGHT: f32 = 168.0;
const ATTACHMENT_CARD_WIDTH: f32 = 160.0;
const ATTACHMENT_THUMB_HEIGHT: f32 = 104.0;

fn draw_attachment_tray(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, channel_id: &str) {
    let Some(queue) = state.pending_attachments.get(channel_id).cloned() else { return };
    let mut removed = None;
    let bg = ui.push_style_color(imgui::StyleColor::ChildBg, state.theme.panel);
    ui.child_window("attachment_tray")
        .size([0.0, ATTACHMENT_TRAY_HEIGHT])
        .horizontal_scrollbar(true)
        .always_use_window_padding(true)
        .build(|| {
            for (index, attachment) in queue.iter().enumerate() {
                if index > 0 {
                    ui.same_line_with_spacing(0.0, 10.0);
                }
                let card_min = ui.cursor_screen_pos();
                let card_max = [card_min[0] + ATTACHMENT_CARD_WIDTH, card_min[1] + ATTACHMENT_TRAY_HEIGHT - 20.0];
                let thumb_min = [card_min[0] + 8.0, card_min[1] + 8.0];
                let thumb_max = [card_max[0] - 8.0, thumb_min[1] + ATTACHMENT_THUMB_HEIGHT];
                let thumbnail = if attachment.is_image() {
                    let key = attachment.preview_key();
                    if state.attachment_previews_requested.insert(attachment.id) {
                        let _ = cmd_tx.send(UiCommand::PreviewAttachment { key: key.clone(), source: attachment.source.clone() });
                    }
                    textures.get(&key)
                } else {
                    None
                };
                {
                    let dl = ui.get_window_draw_list();
                    dl.add_rect(card_min, card_max, state.theme.background).filled(true).rounding(8.0).build();
                    dl.add_rect(card_min, card_max, state.theme.border).rounding(8.0).build();
                    match thumbnail {
                        Some((texture_id, size)) => {
                            let (uv_min, uv_max) = cover_uv(size, [thumb_max[0] - thumb_min[0], thumb_max[1] - thumb_min[1]]);
                            dl.add_image_rounded(texture_id, thumb_min, thumb_max, 6.0).uv_min(uv_min).uv_max(uv_max).build();
                        }
                        None => {
                            dl.add_rect(thumb_min, thumb_max, state.theme.panel).filled(true).rounding(6.0).build();
                        }
                    }
                }
                if thumbnail.is_none() {
                    let icon = if attachment.is_image() { crate::icons::Icon::Image } else { crate::icons::Icon::FileText };
                    let glyph = 32.0;
                    draw_icon_at(
                        ui,
                        textures,
                        icon,
                        [(thumb_min[0] + thumb_max[0] - glyph) / 2.0, (thumb_min[1] + thumb_max[1] - glyph) / 2.0],
                        glyph,
                        state.theme.text_muted,
                    );
                }
                let name = truncate_to_width(ui, &attachment.filename, ATTACHMENT_CARD_WIDTH - 16.0).unwrap_or_else(|| attachment.filename.clone());
                ui.get_window_draw_list().add_text([thumb_min[0], thumb_max[1] + 8.0], state.theme.text, &name);
                ui.set_cursor_screen_pos([card_max[0] - 34.0, card_min[1] + 4.0]);
                if icon_button(ui, textures, &format!("remove_attachment_{}", attachment.id), crate::icons::Icon::Close, "Remove attachment", None) {
                    removed = Some(attachment.id);
                }
                let remove_hovered = ui.is_item_hovered();
                ui.set_cursor_screen_pos(card_min);
                ui.dummy([ATTACHMENT_CARD_WIDTH, card_max[1] - card_min[1]]);
                if ui.is_item_hovered() && !remove_hovered {
                    ui.tooltip_text(&attachment.filename);
                }
            }
        });
    bg.pop();
    if let Some(id) = removed {
        state.remove_attachment(channel_id, id);
    }
}

const POLL_MAX_WIDTH: f32 = 440.0;
const POLL_PADDING: f32 = 12.0;
const POLL_ROW_HEIGHT: f32 = 38.0;

fn poll_time_left(expires_at: Option<chrono::DateTime<chrono::Utc>>, closed: bool, now: chrono::DateTime<chrono::Utc>) -> String {
    let Some(expires_at) = expires_at.filter(|_| !closed) else { return "Poll closed".into() };
    let minutes = (expires_at - now).num_minutes().max(0);
    match (minutes / (60 * 24), minutes / 60 % 24, minutes % 60) {
        (0, 0, 0) => "Less than a minute left".into(),
        (0, 0, m) => format!("{m}m left"),
        (0, h, _) => format!("{h}h left"),
        (d, 0, _) => format!("{d}d left"),
        (d, h, _) => format!("{d}d {h}h left"),
    }
}

fn poll_percent(count: u32, total: u32) -> u32 {
    if total == 0 { 0 } else { ((count as f64 / total as f64) * 100.0).round() as u32 }
}

fn draw_poll_emoji(ui: &Ui, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, emoji: &rustcord_client::models::ReactionEmoji, at: [f32; 2], size: f32) -> bool {
    let texture = match (&emoji.id, &emoji.name) {
        (Some(id), _) => {
            let url = format!("https://cdn.discordapp.com/emojis/{id}.{}?size=32", if emoji.animated { "gif" } else { "png" });
            let texture = textures.get(&url).map(|(t, _)| t);
            if texture.is_none() {
                textures.request(&url, cmd_tx);
            }
            texture
        }
        (None, Some(name)) => match name.chars().next().map(|ch| emoji_texture(textures, ch)) {
            Some(EmojiTexture::Ready(texture)) => Some(texture),
            _ => None,
        },
        (None, None) => return false,
    };
    if let Some(texture) = texture {
        ui.get_window_draw_list().add_image(texture, at, [at[0] + size, at[1] + size]).build();
    }
    true
}

fn draw_poll_creator(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some(draft) = state.poll_draft.as_mut() else { return };
    let mut open = true;
    let mut post = false;
    let display = ui.io().display_size;
    ui.window("Create a poll")
        .opened(&mut open)
        .size([420.0, 0.0], imgui::Condition::Always)
        .position([display[0] / 2.0, display[1] / 2.0], imgui::Condition::Appearing)
        .position_pivot([0.5, 0.5])
        .collapsible(false)
        .resizable(false)
        .build(|| {
            ui.text_colored(state.theme.text_muted, "Question");
            ui.set_next_item_width(-1.0);
            if ui.is_window_appearing() {
                ui.set_keyboard_focus_here();
            }
            ui.input_text("##poll_question", &mut draft.question).hint("What would you like to ask?").build();
            ui.spacing();
            ui.text_colored(state.theme.text_muted, "Answers");
            let mut removed = None;
            let can_remove = draft.answers.len() > 1;
            for (index, answer) in draft.answers.iter_mut().enumerate() {
                ui.set_next_item_width(if can_remove { -36.0 } else { -1.0 });
                ui.input_text(format!("##poll_answer_{index}"), answer).hint("Type your answer").build();
                if can_remove {
                    ui.same_line();
                    if ui.button(format!("x##poll_remove_{index}")) {
                        removed = Some(index);
                    }
                }
            }
            if let Some(index) = removed {
                draft.answers.remove(index);
            }
            if draft.answers.len() < crate::app_state::MAX_POLL_ANSWERS && ui.button("+ Add another answer") {
                draft.answers.push(String::new());
            }
            ui.spacing();
            ui.text_colored(state.theme.text_muted, "Duration");
            ui.set_next_item_width(160.0);
            let durations = rustcord_client::models::POLL_DURATIONS_HOURS;
            if let Some(_combo) = ui.begin_combo("##poll_duration", durations[draft.duration_index].1) {
                for (index, (_, label)) in durations.iter().enumerate() {
                    if ui.selectable_config(*label).selected(index == draft.duration_index).build() {
                        draft.duration_index = index;
                    }
                }
            }
            ui.same_line();
            ui.checkbox("Allow multiple answers", &mut draft.multiselect);
            ui.spacing();
            let disabled = ui.begin_disabled(!draft.can_post());
            if ui.button("Post") {
                post = true;
            }
            drop(disabled);
        });
    if post {
        let draft = state.poll_draft.take().expect("draft present");
        let hours = rustcord_client::models::POLL_DURATIONS_HOURS[draft.duration_index].0;
        let poll = rustcord_client::models::poll_create_payload(&draft.question, &draft.answers, hours, draft.multiselect);
        let _ = cmd_tx.send(UiCommand::SendPoll { channel_id: draft.channel_id, poll });
        state.focus_compose = true;
    } else if !open {
        state.poll_draft = None;
    }
}

fn draw_poll(ui: &Ui, fonts: &Fonts, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, msg: &rustcord_client::models::Message) {
    let Some(poll) = msg.poll.as_ref() else { return };
    let now = chrono::Utc::now();
    let closed = poll.is_closed(now);
    let my_votes = poll.my_votes();
    let mut view = state.poll_views.get(&msg.id).cloned().unwrap_or_default();
    let results = closed || !my_votes.is_empty() || view.show_results;
    let total = poll.total_votes();
    let is_author = state.current_user.as_ref().is_some_and(|u| u.id == msg.author.id);
    let theme = state.theme.clone();
    let width = ui.content_region_avail()[0].min(POLL_MAX_WIDTH).max(200.0);
    let inner = width - POLL_PADDING * 2.0;
    let start = ui.cursor_screen_pos();
    let cached_height = state.poll_card_heights.get(&msg.id).copied().unwrap_or(0.0);
    ui.get_window_draw_list()
        .add_rect(start, [start[0] + width, start[1] + cached_height], theme.panel)
        .filled(true)
        .rounding(10.0)
        .build();
    let local = ui.cursor_pos();
    ui.set_cursor_pos([local[0] + POLL_PADDING, local[1] + POLL_PADDING]);
    let mut vote: Option<Vec<u32>> = None;
    let mut end = false;
    ui.group(|| {
        let wrap = ui.push_text_wrap_pos_with_pos(local[0] + width - POLL_PADDING);
        let font = ui.push_font(fonts.bold);
        ui.text_wrapped(poll.question.text.as_deref().unwrap_or("Poll"));
        font.pop();
        let hint = match (closed, poll.allow_multiselect) {
            (true, _) => "Final results",
            (false, true) => "Select one or more answers",
            (false, false) => "Select one answer",
        };
        ui.text_colored(theme.text_muted, hint);
        wrap.end();
        ui.dummy([0.0, 2.0]);
        for answer in &poll.answers {
            let count = poll.count(answer.answer_id);
            let selected = view.selected.contains(&answer.answer_id);
            let clicked = ui.invisible_button(format!("##poll_{}_{}", msg.id, answer.answer_id), [inner, POLL_ROW_HEIGHT]);
            let hovered = ui.is_item_hovered();
            let (p_min, p_max) = (ui.item_rect_min(), ui.item_rect_max());
            if clicked && !results && !closed {
                view.toggle(answer.answer_id, poll.allow_multiselect);
            }
            let highlight = if results { count.me_voted } else { selected };
            {
                let dl = ui.get_window_draw_list();
                let base = if hovered && !results { theme.accent_hover } else { theme.background };
                dl.add_rect(p_min, p_max, [base[0], base[1], base[2], if hovered && !results { 0.35 } else { 1.0 }]).filled(true).rounding(8.0).build();
                if results && total > 0 {
                    let fraction = count.count as f32 / total as f32;
                    let alpha = if count.me_voted { 0.55 } else { 0.25 };
                    let fill = [theme.accent[0], theme.accent[1], theme.accent[2], alpha];
                    dl.add_rect(p_min, [p_min[0] + inner * fraction, p_max[1]], fill).filled(true).rounding(8.0).build();
                }
                if highlight {
                    dl.add_rect(p_min, p_max, theme.accent).rounding(8.0).thickness(2.0).build();
                }
            }
            let mut text_x = p_min[0] + 12.0;
            if let Some(emoji) = &answer.poll_media.emoji {
                let size = 20.0;
                if draw_poll_emoji(ui, textures, cmd_tx, emoji, [text_x, p_min[1] + (POLL_ROW_HEIGHT - size) / 2.0], size) {
                    text_x += size + 8.0;
                }
            }
            let text_y = p_min[1] + (POLL_ROW_HEIGHT - ui.text_line_height()) / 2.0;
            let right = if results {
                format!("{} {} \u{b7} {}%", count.count, if count.count == 1 { "vote" } else { "votes" }, poll_percent(count.count, total))
            } else {
                String::new()
            };
            let right_width = ui.calc_text_size(&right)[0];
            let label = answer.poll_media.text.clone().unwrap_or_default();
            let label = truncate_to_width(ui, &label, (p_max[0] - 12.0 - right_width - 12.0 - text_x).max(20.0)).unwrap_or(label);
            let dl = ui.get_window_draw_list();
            dl.add_text([text_x, text_y], theme.text, &label);
            if results {
                dl.add_text([p_max[0] - 12.0 - right_width, text_y], if count.me_voted { theme.text } else { theme.text_muted }, &right);
            } else {
                let center = [p_max[0] - 22.0, p_min[1] + POLL_ROW_HEIGHT / 2.0];
                if poll.allow_multiselect {
                    let (a, b) = ([center[0] - 8.0, center[1] - 8.0], [center[0] + 8.0, center[1] + 8.0]);
                    if selected {
                        dl.add_rect(a, b, theme.accent).filled(true).rounding(3.0).build();
                    } else {
                        dl.add_rect(a, b, theme.text_muted).rounding(3.0).thickness(1.5).build();
                    }
                } else if selected {
                    dl.add_circle(center, 8.0, theme.accent).filled(true).build();
                    dl.add_circle(center, 3.5, theme.background).filled(true).build();
                } else {
                    dl.add_circle(center, 8.0, theme.text_muted).thickness(1.5).build();
                }
            }
        }
        ui.dummy([0.0, 2.0]);
        let summary = format!(
            "{total} {} \u{b7} {}",
            if total == 1 { "vote" } else { "votes" },
            poll_time_left(poll.expires_at(), closed, now)
        );
        ui.text_colored(theme.text_muted, summary);
        if !closed {
            if my_votes.is_empty() {
                ui.same_line();
                let toggle = if view.show_results { "Back to voting" } else { "Show results" };
                if ui.small_button(format!("{toggle}##poll_results_{}", msg.id)) {
                    view.show_results = !view.show_results;
                }
                if !view.show_results {
                    ui.same_line();
                    let disabled = ui.begin_disabled(view.selected.is_empty());
                    if ui.small_button(format!("Vote##poll_vote_{}", msg.id)) {
                        vote = Some(view.selected.clone());
                    }
                    drop(disabled);
                }
            } else {
                ui.same_line();
                if ui.small_button(format!("Remove vote##poll_unvote_{}", msg.id)) {
                    vote = Some(Vec::new());
                }
            }
            if is_author {
                ui.same_line();
                if ui.small_button(format!("End poll##poll_end_{}", msg.id)) {
                    end = true;
                }
            }
        }
    });
    let bottom = ui.item_rect_max()[1] + POLL_PADDING;
    state.poll_card_heights.insert(msg.id.clone(), bottom - start[1]);
    ui.set_cursor_screen_pos([start[0], bottom]);
    ui.dummy([width, 0.0]);
    if let Some(answer_ids) = vote {
        let _ = cmd_tx.send(UiCommand::VotePoll { channel_id: msg.channel_id.clone(), message_id: msg.id.clone(), answer_ids });
    }
    if end {
        let _ = cmd_tx.send(UiCommand::EndPoll { channel_id: msg.channel_id.clone(), message_id: msg.id.clone() });
    }
    if view.selected.is_empty() && !view.show_results {
        state.poll_views.remove(&msg.id);
    } else {
        state.poll_views.insert(msg.id.clone(), view);
    }
}

const MOSAIC_MAX_WIDTH: f32 = 520.0;
const MOSAIC_GAP: f32 = 4.0;

fn mosaic_rows(count: usize) -> Vec<usize> {
    match count {
        0 => Vec::new(),
        1 | 2 => vec![count],
        3 => vec![1, 2],
        4 => vec![2, 2],
        _ => {
            let lead = count % 3;
            let lead = if lead == 0 { None } else { Some(lead) };
            lead.into_iter().chain(std::iter::repeat_n(3, count / 3)).collect()
        }
    }
}

fn draw_attachments(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    attachments: &[rustcord_client::models::Attachment],
    lightbox: &mut Option<String>,
) {
    let (images, others): (Vec<_>, Vec<_>) =
        attachments.iter().partition(|a| is_image_attachment(a.content_type.as_deref(), &a.filename));
    if images.len() >= 2 {
        draw_image_mosaic(ui, cmd_tx, textures, &images, lightbox);
    } else {
        for attachment in images {
            draw_attachment(ui, cmd_tx, textures, attachment, lightbox);
        }
    }
    for attachment in others {
        draw_attachment(ui, cmd_tx, textures, attachment, lightbox);
    }
}

const VOICE_CARD_MAX_WIDTH: f32 = 400.0;
const VOICE_CARD_HEIGHT: f32 = 52.0;
const VOICE_PLAY_SIZE: f32 = 34.0;
const WAVEFORM_BAR: f32 = 3.0;
const WAVEFORM_GAP: f32 = 2.0;
const RECORDING_RED: [f32; 4] = [0.93, 0.26, 0.27, 1.0];

fn format_clip_time(secs: f64) -> String {
    let secs = secs.max(0.0).round() as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn waveform_bar_count(width: f32) -> usize {
    ((width + WAVEFORM_GAP) / (WAVEFORM_BAR + WAVEFORM_GAP)).floor().max(0.0) as usize
}

fn draw_waveform_bars(dl: &imgui::DrawListMut, min: [f32; 2], height: f32, levels: &[f32], played: f32, played_color: [f32; 4], rest_color: [f32; 4]) {
    let mid = min[1] + height / 2.0;
    let count = levels.len().max(1) as f32;
    for (index, level) in levels.iter().enumerate() {
        let x = min[0] + index as f32 * (WAVEFORM_BAR + WAVEFORM_GAP);
        let half = (level.clamp(0.0, 1.0) * height / 2.0).max(1.5);
        let color = if (index as f32 + 0.5) / count <= played { played_color } else { rest_color };
        dl.add_rect([x, mid - half], [x + WAVEFORM_BAR, mid + half], color).filled(true).rounding(WAVEFORM_BAR / 2.0).build();
    }
}

fn draw_voice_messages(
    ui: &Ui,
    state: &mut AppState,
    textures: &TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    attachments: &[rustcord_client::models::Attachment],
) {
    for attachment in attachments {
        draw_voice_message(ui, state, textures, cmd_tx, attachment);
    }
}

fn draw_voice_message(
    ui: &Ui,
    state: &mut AppState,
    textures: &TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    attachment: &rustcord_client::models::Attachment,
) {
    use crate::app_state::VoiceClip;
    let url = attachment.url.as_str();
    let _id = ui.push_id(url);
    let theme = state.theme.clone();
    let width = ui.content_region_avail()[0].min(VOICE_CARD_MAX_WIDTH).max(220.0);
    let origin = ui.cursor_screen_pos();
    let max = [origin[0] + width, origin[1] + VOICE_CARD_HEIGHT];
    ui.get_window_draw_list().add_rect(origin, max, theme.panel).filled(true).rounding(10.0).build();

    let progress = state.voice_clip_progress(url);
    let playing = progress.is_some_and(|(_, playing)| playing);
    let fraction = progress.map_or(0.0, |(fraction, _)| fraction);
    let (loading, failed, loaded_secs) = match state.voice_clips.get(url) {
        Some(VoiceClip::Loading) => (true, None, None),
        Some(VoiceClip::Failed(e)) => (false, Some(e.clone()), None),
        Some(VoiceClip::Ready(samples)) => (false, None, Some(rustcord_client::voice_message::duration_secs(samples))),
        None => (false, None, None),
    };
    let duration = attachment.duration_secs.or(loaded_secs).unwrap_or(0.0);

    let pad = (VOICE_CARD_HEIGHT - VOICE_PLAY_SIZE) / 2.0;
    let button_min = [origin[0] + pad, origin[1] + pad];
    ui.set_cursor_screen_pos(button_min);
    let play_clicked = ui.invisible_button("voice_play", [VOICE_PLAY_SIZE, VOICE_PLAY_SIZE]);
    let play_hovered = ui.is_item_hovered();
    let center = [button_min[0] + VOICE_PLAY_SIZE / 2.0, button_min[1] + VOICE_PLAY_SIZE / 2.0];
    let fill = if play_hovered { theme.accent_hover } else { theme.accent };
    let dl = ui.get_window_draw_list();
    dl.add_circle(center, VOICE_PLAY_SIZE / 2.0, fill).filled(true).build();
    let icon = if playing {
        crate::icons::Icon::Pause
    } else if progress.is_some_and(|(fraction, _)| fraction >= 1.0) {
        crate::icons::Icon::Replay
    } else {
        crate::icons::Icon::Play
    };
    let icon_size = VOICE_PLAY_SIZE * 0.5;
    let icon_alpha = if loading { 0.45 + 0.35 * (ui.time() as f32 * 4.0).sin().abs() } else { 1.0 };
    draw_icon_at(ui, textures, icon, [center[0] - icon_size / 2.0, center[1] - icon_size / 2.0], icon_size, [1.0, 1.0, 1.0, icon_alpha]);
    if play_hovered {
        ui.tooltip_text(match (&failed, playing) {
            (Some(_), _) => "Retry",
            (None, true) => "Pause",
            (None, false) => "Play",
        });
    }

    let label = match (&failed, progress) {
        (Some(_), _) => "Failed".to_string(),
        (None, Some((fraction, _))) if fraction > 0.0 && fraction < 1.0 => format_clip_time(fraction as f64 * duration),
        _ => format_clip_time(duration),
    };
    let label_size = ui.calc_text_size(&label);
    let label_pos = [max[0] - pad - label_size[0], origin[1] + (VOICE_CARD_HEIGHT - label_size[1]) / 2.0];
    dl.add_text(label_pos, if failed.is_some() { RECORDING_RED } else { theme.text_muted }, &label);

    let wave_min = [button_min[0] + VOICE_PLAY_SIZE + 12.0, origin[1] + 12.0];
    let wave_width = (label_pos[0] - 12.0 - wave_min[0]).max(0.0);
    let wave_height = VOICE_CARD_HEIGHT - 24.0;
    let source = attachment.waveform.as_deref().map(rustcord_client::voice_message::decode_waveform).unwrap_or_default();
    let bars = waveform_bar_count(wave_width);
    let levels = if source.is_empty() { vec![0.25; bars] } else { rustcord_client::voice_message::resample_waveform(&source, bars) };
    let rest = [theme.text_muted[0], theme.text_muted[1], theme.text_muted[2], theme.text_muted[3] * 0.55];
    draw_waveform_bars(&dl, wave_min, wave_height, &levels, fraction, theme.accent, rest);

    ui.set_cursor_screen_pos(wave_min);
    let seek_clicked = ui.invisible_button("voice_seek", [wave_width.max(1.0), wave_height]);
    if ui.is_item_hovered() {
        ui.set_mouse_cursor(Some(imgui::MouseCursor::Hand));
        if let Some(e) = &failed {
            ui.tooltip_text(e);
        }
    }
    if seek_clicked {
        let fraction = ((ui.io().mouse_pos[0] - wave_min[0]) / wave_width.max(1.0)).clamp(0.0, 1.0);
        if matches!(state.voice_clips.get(url), Some(VoiceClip::Ready(_))) {
            state.seek_voice_clip(url, fraction);
        } else {
            state.toggle_voice_clip(url, cmd_tx);
        }
    } else if play_clicked {
        state.toggle_voice_clip(url, cmd_tx);
    }

    ui.set_cursor_screen_pos(origin);
    ui.dummy([width, VOICE_CARD_HEIGHT]);
}

#[allow(clippy::too_many_arguments)]
fn draw_forwarded(
    ui: &Ui,
    fonts: &Fonts,
    state: &mut AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    msg: &rustcord_client::models::Message,
    snapshot: &rustcord_client::models::SnapshotMessage,
    channel_names: &HashMap<String, String>,
    role_names: &HashMap<String, (String, Option<[f32; 4]>)>,
) {
    let start = ui.cursor_screen_pos();
    ui.indent_by(FORWARD_INDENT);
    ui.group(|| {
        draw_icon(ui, textures, crate::icons::Icon::Forward, 14.0, state.theme.text_muted);
        ui.same_line_with_spacing(0.0, 4.0);
        timestamp_text(ui, state.theme.text_muted, "Forwarded", "Forwarded message", 0.85);
        if !snapshot.content.is_empty() {
            let seed = format!("{}_fwd", msg.id);
            draw_markdown(ui, fonts, &state.theme, &snapshot.content, &mut state.revealed_spoilers, &seed, textures, cmd_tx, &mut MentionCtx { known_users: &state.known_users, profiles_requested: &mut state.profiles_requested, channel_names, role_names });
        }
        for (embed_idx, embed) in snapshot.embeds.iter().enumerate() {
            draw_embed(ui, fonts, &state.theme, embed, &format!("{}_fwd_{embed_idx}", msg.id), textures, cmd_tx, &mut state.image_lightbox, &mut state.revealed_spoilers, &mut MentionCtx { known_users: &state.known_users, profiles_requested: &mut state.profiles_requested, channel_names, role_names });
        }
        if snapshot.is_voice_message() {
            draw_voice_messages(ui, state, textures, cmd_tx, &snapshot.attachments);
        } else {
            draw_attachments(ui, cmd_tx, textures, &snapshot.attachments, &mut state.image_lightbox);
        }
        draw_sticker_items(ui, &state.theme, textures, cmd_tx, &snapshot.sticker_items);
        let reference = msg.message_reference.as_ref();
        let source_channel = reference.and_then(|r| r.channel_id.clone());
        let when = chrono::DateTime::parse_from_rfc3339(&snapshot.timestamp)
            .map(|dt| crate::timefmt::TimeFormat::resolve(state.clock_24h).short_datetime(&dt.with_timezone(&chrono::Local)))
            .unwrap_or_default();
        let footer = match source_channel.as_ref().and_then(|id| channel_names.get(id)) {
            Some(name) if when.is_empty() => format!("#{name}"),
            Some(name) => format!("#{name}  \u{2022}  {when}"),
            None => when,
        };
        if !footer.is_empty() {
            ui.set_window_font_scale(0.8);
            let jump = reference.and_then(|r| Some((r.channel_id.clone()?, r.message_id.clone()?)));
            let clicked = clickable_text(ui, &format!("fwd_src_{}", msg.id), &footer, state.theme.text_muted);
            ui.set_window_font_scale(1.0);
            if let (true, Some((channel_id, message_id))) = (clicked, jump) {
                let _ = cmd_tx.send(UiCommand::JumpToMessage { channel_id, message_id });
            }
        }
    });
    ui.unindent_by(FORWARD_INDENT);
    let end = ui.item_rect_max()[1];
    ui.get_window_draw_list()
        .add_rect([start[0], start[1]], [start[0] + 3.0, end], state.theme.border)
        .filled(true)
        .rounding(1.5)
        .build();
}

const FORWARD_INDENT: f32 = 12.0;

#[allow(clippy::too_many_arguments)]
fn draw_message_media(
    ui: &Ui,
    fonts: &Fonts,
    state: &mut AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    msg: &rustcord_client::models::Message,
    channel_names: &HashMap<String, String>,
    role_names: &HashMap<String, (String, Option<[f32; 4]>)>,
) {
    if let Some(snapshot) = msg.forwarded() {
        draw_forwarded(ui, fonts, state, textures, cmd_tx, msg, snapshot, channel_names, role_names);
    }
    if msg.poll.is_some() {
        draw_poll(ui, fonts, state, textures, cmd_tx, msg);
    }
    if msg.is_voice_message() {
        draw_voice_messages(ui, state, textures, cmd_tx, &msg.attachments);
    } else {
        draw_attachments(ui, cmd_tx, textures, &msg.attachments, &mut state.image_lightbox);
    }
    draw_sticker_items(ui, &state.theme, textures, cmd_tx, &msg.sticker_items);
}

fn draw_recording_bar(ui: &Ui, state: &mut AppState, textures: &TextureCache, cmd_tx: &UnboundedSender<UiCommand>, start: [f32; 2], width: f32) {
    let height = ui.frame_height();
    let spacing = ui.clone_style().item_spacing[0];
    let theme = state.theme.clone();
    let Some(recording) = state.voice_recording.as_mut() else { return };
    recording.sample_level();
    let elapsed = recording.recorder.duration_secs();
    let levels: Vec<f32> = recording.levels.iter().map(|l| l.sqrt()).collect();

    let discard = icon_button_sized(ui, textures, "voice_discard", crate::icons::Icon::Trash, "Discard (Esc)", Some(RECORDING_RED), height);
    ui.same_line();
    let x0 = ui.cursor_screen_pos()[0];
    let mid = start[1] + height / 2.0;
    let dl = ui.get_window_draw_list();
    let pulse = 0.55 + 0.45 * (ui.time() as f32 * 3.0).sin().abs();
    dl.add_circle([x0 + 6.0, mid], 5.0, [RECORDING_RED[0], RECORDING_RED[1], RECORDING_RED[2], pulse]).filled(true).build();
    let time = format_clip_time(elapsed);
    let time_size = ui.calc_text_size(&time);
    dl.add_text([x0 + 18.0, mid - time_size[1] / 2.0], theme.text, &time);

    let send_x = start[0] + width - height;
    let bars_min = x0 + 18.0 + time_size[0] + 14.0;
    let bars_width = (send_x - spacing - 8.0 - bars_min).max(0.0);
    let count = waveform_bar_count(bars_width).min(levels.len());
    let shown = &levels[levels.len() - count..];
    let bars_x = bars_min + bars_width - count as f32 * (WAVEFORM_BAR + WAVEFORM_GAP);
    draw_waveform_bars(&dl, [bars_x, start[1] + 8.0], height - 16.0, shown, 1.0, theme.accent, theme.accent);
    if count == 0 {
        let hint = "Recording... Enter to send, Esc to discard";
        dl.add_text([bars_min, mid - ui.text_line_height() / 2.0], theme.text_muted, hint);
    }

    ui.set_cursor_screen_pos([send_x, start[1]]);
    let send = icon_button_sized(ui, textures, "voice_send", crate::icons::Icon::Send, "Send (Enter)", Some(theme.accent), height);
    let enter = ui.is_key_pressed(imgui::Key::Enter) || ui.is_key_pressed(imgui::Key::KeypadEnter);
    let limit = elapsed >= rustcord_client::voice_message::MAX_RECORDING_SECS;
    if send || enter || limit {
        state.finish_voice_recording(true, cmd_tx);
    } else if discard || ui.is_key_pressed(imgui::Key::Escape) {
        state.finish_voice_recording(false, cmd_tx);
    }
}

fn draw_forward_dialog(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some(query) = state.forward_dialog.as_ref().map(|d| d.query.clone()) else { return };
    let targets = state.forward_targets(&query);
    let theme = state.theme.clone();
    let Some(dialog) = state.forward_dialog.as_mut() else { return };
    let mut open = true;
    let mut send = false;
    let display = ui.io().display_size;
    ui.window("Forward To")
        .opened(&mut open)
        .size([440.0, 0.0], imgui::Condition::Always)
        .position([display[0] / 2.0, display[1] / 2.0], imgui::Condition::Appearing)
        .position_pivot([0.5, 0.5])
        .collapsible(false)
        .resizable(false)
        .build(|| {
            ui.text_colored(theme.text_muted, format!("Select where you want to share this message (up to {}).", crate::app_state::MAX_FORWARD_TARGETS));
            ui.set_next_item_width(-1.0);
            if ui.is_window_appearing() {
                ui.set_keyboard_focus_here();
            }
            ui.input_text("##forward_query", &mut dialog.query).hint("Search").build();
            ui.child_window("forward_targets").size([0.0, 260.0]).border(true).build(|| {
                if targets.is_empty() {
                    ui.text_disabled("No matching conversations");
                }
                for target in &targets {
                    let selected = dialog.selected.contains(&target.channel_id);
                    let full = !selected && dialog.selected.len() >= crate::app_state::MAX_FORWARD_TARGETS;
                    let disabled = ui.begin_disabled(full);
                    if ui.selectable_config(format!("{}##fwd_{}", target.label, target.channel_id)).selected(selected).build() {
                        dialog.toggle(&target.channel_id);
                    }
                    drop(disabled);
                    if let Some(detail) = &target.detail {
                        ui.same_line();
                        let width = ui.calc_text_size(detail)[0];
                        let right = ui.content_region_max()[0];
                        ui.set_cursor_pos([(right - width).max(ui.cursor_pos()[0]), ui.cursor_pos()[1]]);
                        ui.text_colored(theme.text_muted, detail);
                    }
                }
            });
            ui.spacing();
            ui.text_colored(theme.text_muted, "Message");
            ui.text_wrapped(&dialog.preview);
            ui.spacing();
            ui.set_next_item_width(-1.0);
            ui.input_text("##forward_note", &mut dialog.note).hint("Add an optional message...").build();
            ui.spacing();
            let disabled = ui.begin_disabled(dialog.selected.is_empty());
            let label = match dialog.selected.len() {
                0 | 1 => "Send".to_string(),
                n => format!("Send to {n}"),
            };
            if ui.button(label) {
                send = true;
            }
            drop(disabled);
        });
    if send {
        let dialog = state.forward_dialog.take().expect("dialog present");
        let _ = cmd_tx.send(UiCommand::ForwardMessage { source: dialog.source, targets: dialog.selected, note: dialog.note });
    } else if !open {
        state.forward_dialog = None;
    }
}

fn draw_image_mosaic(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    images: &[&rustcord_client::models::Attachment],
    lightbox: &mut Option<String>,
) {
    let width = ui.content_region_avail()[0].min(MOSAIC_MAX_WIDTH).max(120.0);
    let _spacing = ui.push_style_var(imgui::StyleVar::ItemSpacing([MOSAIC_GAP, MOSAIC_GAP]));
    let mut remaining = images.iter();
    for row in mosaic_rows(images.len()) {
        let cell_width = (width - MOSAIC_GAP * (row as f32 - 1.0)) / row as f32;
        let cell = [cell_width, if row == 1 { width * 0.5 } else { cell_width }];
        for (column, attachment) in remaining.by_ref().take(row).enumerate() {
            if column > 0 {
                ui.same_line();
            }
            let clicked = ui.invisible_button(format!("##mosaic_{}", attachment.id), cell);
            let (p_min, p_max) = (ui.item_rect_min(), ui.item_rect_max());
            let hovered = ui.is_item_hovered();
            let dl = ui.get_window_draw_list();
            match textures.get(&attachment.url) {
                Some((texture_id, size)) => {
                    let (uv_min, uv_max) = cover_uv(size, cell);
                    dl.add_image_rounded(texture_id, p_min, p_max, 6.0).uv_min(uv_min).uv_max(uv_max).build();
                }
                None => {
                    if !textures.is_failed(&attachment.url) {
                        textures.request(&attachment.url, cmd_tx);
                    }
                    dl.add_rect(p_min, p_max, [0.5, 0.5, 0.5, 0.15]).filled(true).rounding(6.0).build();
                }
            }
            if hovered {
                dl.add_rect(p_min, p_max, [1.0, 1.0, 1.0, 0.12]).filled(true).rounding(6.0).build();
                ui.tooltip_text(&attachment.filename);
            }
            if clicked {
                *lightbox = Some(attachment.url.clone());
            }
        }
    }
}

fn draw_attachment(
    ui: &Ui,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    attachment: &rustcord_client::models::Attachment,
    lightbox: &mut Option<String>,
) {
    if !is_image_attachment(attachment.content_type.as_deref(), &attachment.filename) {
        #[cfg(feature = "video")]
        if is_video_attachment(attachment.content_type.as_deref(), &attachment.filename) {
            draw_video_attachment(ui, cmd_tx, textures, attachment);
            return;
        }
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
    if let Some(gradient) = state.theme.global_gradient() {
        gradient.draw(&ui.get_background_draw_list(), [0.0, 0.0], ui.io().display_size);
    }
    if ui.io().key_ctrl && ui.io().key_shift && ui.is_key_pressed(imgui::Key::I) {
        state.devtools_requested = true;
    }
    if !state.logged_in {
        draw_login(ui, state, cmd_tx);
        let _ = textures;
        return;
    }

    if ui.io().key_ctrl && ui.is_key_pressed(imgui::Key::K) {
        state.switcher_open = true;
        state.switcher_focus_input = true;
        state.switcher_query.clear();
        state.switcher_selected = 0;
    }
    dispatch_keybinds(ui, state, cmd_tx);
    if let Some((guild_id, channel_id)) = state.pending_invite_voice.clone() {
        if state.guilds.iter().any(|g| g.id == guild_id) && !state.voice_connecting {
            let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: Some(guild_id.clone()), channel_id: channel_id.clone(), ring: false });
            state.pending_voice_channel = Some((Some(guild_id), channel_id));
            state.pending_invite_voice = None;
        }
    }

    let [display_w, display_h] = ui.io().display_size;
    #[cfg(feature = "screenshare")]
    let server_call_fullscreen = state.connected_voice_channel.as_ref().is_some_and(|(guild_id, _)| guild_id.is_some())
        && !state.server_call_minimized;
    #[cfg(not(feature = "screenshare"))]
    let server_call_fullscreen = false;
    state.sidebar_width = state.sidebar_width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
    let sidebar_width = state.sidebar_width;
    let surface_bg = if state.theme.use_global_gradient { [0.0; 4] } else { state.theme.panel };
    let servers_bg = if state.theme.use_global_gradient { [0.0; 4] } else { state.theme.servers_bg() };

    let _server_bg = ui.push_style_color(imgui::StyleColor::WindowBg, servers_bg);
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
            let _child_bg = ui.push_style_color(imgui::StyleColor::ChildBg, servers_bg);
            ui.child_window("guilds_scroll").size([RAIL_WIDTH, display_h - after_separator_y]).build(|| {
                ui.set_cursor_pos([rail_center_x, 0.0]);
                draw_rail_items(ui, state, cmd_tx, textures, rail_center_x);
            });
            _child_bg.pop();
            _no_pad.pop();
        });
        _server_bg.pop(); // <- Non dimenticare il pop()!

    let _channels_bg = ui.push_style_color(imgui::StyleColor::WindowBg, surface_bg);
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
            let _channels_content_bg = ui.push_style_color(imgui::StyleColor::ChildBg, surface_bg);
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
                    state.channel_context = None;
                    ui.open_popup("channel_list_ctx");
                }
                ui.popup("channel_list_ctx", || {
                    draw_channel_notification_menu(ui, state, cmd_tx);
                    draw_channel_admin_menu(ui, state, cmd_tx);
                    let label = if state.show_hidden_channels { "Hide channels you can't access" } else { "Show channels you can't access" };
                    if ui.selectable(label) {
                        state.show_hidden_channels = !state.show_hidden_channels;
                    }
                });
                for (category_name, channels) in &groups {
                    if let Some(name) = category_name {
                        ui.dummy([0.0, 8.0]);
                        let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                        ui.set_window_font_scale(0.8);
                        ui.text(name.to_uppercase());
                        ui.set_window_font_scale(1.0);
                        token.pop();
                        ui.dummy([0.0, 2.0]);
                    }
                    for (channel_id, name, last_message_id, visible) in channels {
                        let is_unread = *visible && state.channel_shows_unread(state.selected_guild.as_deref(), channel_id, last_message_id.as_deref());
                        let mentions = state.mention_count(channel_id);
                        let row_start = ui.cursor_screen_pos();
                        let _dim = match (visible, is_unread) {
                            (false, _) => Some(ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted)),
                            (true, false) => Some(ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted)),
                            (true, true) => None,
                        };
                        let label = format!("# {name}");
                        let badge_room = if mentions > 0 { 34.0 } else { 0.0 };
                        let shown = truncate_to_width(ui, &label, ui.content_region_avail()[0] - badge_room);
                        if *visible {
                            let clicked = ui
                                .selectable_config(shown.as_deref().unwrap_or(&label))
                                .size([0.0, CHANNEL_ROW_HEIGHT])
                                .build();
                            if shown.is_some() && ui.is_item_hovered() {
                                ui.tooltip_text(&label);
                            }
                            if ui.is_item_clicked_with_button(imgui::MouseButton::Right) {
                                state.channel_context = Some(channel_id.clone());
                            }
                            if clicked {
                                let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
                                state.selected_channel = Some(channel_id.clone());
                                state.server_call_minimized = true;
                                if let Some(last_id) = last_message_id {
                                    if state.mark_channel_read(channel_id, last_id) {
                                        let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: channel_id.clone(), message_id: last_id.clone() });
                                    }
                                }
                            }
                        } else {
                            ui.text(shown.as_deref().unwrap_or(&label));
                            if ui.is_item_hovered() {
                                ui.tooltip_text("You don't have access to this channel");
                            }
                        }
                        let row_height = ui.item_rect_size()[1];
                        if is_unread {
                            draw_unread_dot(ui, row_start, row_height);
                        }
                        if mentions > 0 {
                            draw_mention_badge(ui, state.theme.mention_badge(), row_start, row_height, mentions);
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
                        let shown = truncate_to_width(ui, &label, ui.content_region_avail()[0]);
                        let clicked = ui.selectable(shown.as_deref().unwrap_or(&label));
                        if shown.is_some() && ui.is_item_hovered() {
                            ui.tooltip_text(&label);
                        }
                        if clicked {
                            if is_connected {
                                state.server_call_minimized = false;
                            } else if !state.voice_connecting {
                                let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: Some(guild_id.clone()), channel_id: channel_id.clone(), ring: false });
                                state.pending_voice_channel = Some((Some(guild_id.clone()), channel_id.clone()));
                                state.server_call_minimized = false;
                                state.server_call_focus_stream = false;
                            }
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
                            #[cfg(feature = "screenshare")]
                            if state.active_streams.contains_key(&member_id) {
                                let is_self = state.current_user.as_ref().is_some_and(|user| user.id == member_id);
                                if !is_self {
                                    ui.set_cursor_screen_pos([row_pos[0] + row_width - 60.0, row_pos[1] + 7.0]);
                                    ui.text_colored([0.95, 0.25, 0.35, 1.0], "LIVE");
                                    ui.set_cursor_screen_pos([row_pos[0], row_pos[1] + ROW_H]);
                                }
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
                                let status_icon = if member.self_deaf || member.deaf {
                                    Some(crate::icons::Icon::HeadphoneOff)
                                } else if member.self_mute || member.mute {
                                    Some(crate::icons::Icon::MicOff)
                                } else {
                                    None
                                };
                                if let Some(texture) = status_icon.and_then(|icon| textures.icon(icon, 16.0)) {
                                    let x = row_pos[0] + row_width - 22.0;
                                    dl.add_image(texture, [x, center[1] - 8.0], [x + 16.0, center[1] + 8.0]).col(state.theme.mention_badge()).build();
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
                        #[cfg(feature = "screenshare")]
                        if let Some(stream_key) = state.active_streams.get(&member_id).cloned() {
                            if state.watching_stream.as_deref() == Some(stream_key.as_str()) {
                                if ui.selectable("Open call view") {
                                    state.server_call_minimized = false;
                                }
                                if ui.selectable("Stop watching") {
                                    let _ = cmd_tx.send(UiCommand::StopWatchingStream { stream_key: stream_key.clone() });
                                    state.unwatching_stream = Some(stream_key.clone());
                                    state.watching_stream = None;
                                    state.server_call_minimized = false;
                                }
                            } else if ui.selectable("Watch screen share") {
                                let _ = cmd_tx.send(UiCommand::WatchStream { stream_key: stream_key.clone() });
                                state.watching_stream = Some(stream_key);
                                state.server_call_minimized = false;
                                state.server_call_focus_stream = false;
                            }
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
                    let can_open_call = state.connected_voice_channel.as_ref().is_some_and(|(guild_id, _)| guild_id.is_some());
                    draw_voice_panel(ui, state, textures, cmd_tx, !server_call_fullscreen && state.server_call_minimized && can_open_call);
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
    #[cfg(feature = "screenshare")]
    draw_screen_share_setup_dialog(ui, state, cmd_tx);
    draw_profile_popout(ui, state, cmd_tx, textures);

    let mut open_emoji_picker = false;
    let channel_names = state.channel_name_map();
    let role_names = state.role_name_map();
    let search_panel_open = state.search_open;
    let members_panel_open = state.show_members_panel && state.selected_guild.is_some() && !search_panel_open;
    let profile_panel_user = (state.show_profile_panel && !search_panel_open && state.selected_guild.is_none())
        .then(|| {
            let channel_id = state.selected_channel.as_deref()?;
            let dm = state.dm_channels.iter().find(|c| c.id == channel_id && c.kind == rustcord_client::models::ChannelType::Dm)?;
            dm.recipients.first().cloned()
        })
        .flatten();
    let right_panel_width = if search_panel_open {
        SEARCH_PANEL_WIDTH
    } else if profile_panel_user.is_some() {
        PROFILE_PANEL_WIDTH
    } else if members_panel_open {
        MEMBERS_PANEL_WIDTH
    } else {
        0.0
    };

    let chat_bg = if state.theme.use_global_gradient { [0.0; 4] } else { state.theme.background };
    let _chat_bg = ui.push_style_color(imgui::StyleColor::WindowBg, chat_bg);
    ui.window("chat")
        .position([RAIL_WIDTH + sidebar_width, 0.0], imgui::Condition::Always)
        .size([display_w - RAIL_WIDTH - sidebar_width - right_panel_width, display_h], imgui::Condition::Always)
        .no_decoration()
        .scrollable(false)
        .draw_background(true)
        .build(|| {
            #[cfg(feature = "screenshare")]
            if server_call_fullscreen {
                draw_server_call_fullscreen(ui, state, textures, cmd_tx);
                return;
            }
            state.sync_draft_channel();
            let show_reconnecting = state.logged_in && !state.gateway_connected;
            let status_height = if state.status_message.is_some() || show_reconnecting { 24.0 } else { 0.0 };
            let channel_key = state.selected_channel.clone().unwrap_or_default();
            let channel_rows = state.rows_for(&channel_key);
            let rows = &channel_rows.rows;
            let seen_count = state.last_seen_message_count.get(&channel_key).copied().unwrap_or(0);
            let has_new_messages = rows.len() > seen_count;

            let dm_channel = state.selected_channel.as_ref().and_then(|id| state.dm_channels.iter().find(|c| c.id == *id).cloned());
            let mut header_height = if state.selected_channel.is_some() { HEADER_HEIGHT } else { 0.0 };
            if state.selected_channel.is_some() {
                let _header_bg = ui.push_style_color(imgui::StyleColor::ChildBg, [0.0; 4]);
                draw_channel_header(ui, state, cmd_tx, textures, dm_channel.as_ref());
            }
            if let Some(dm) = &dm_channel {
                if state.connected_voice_channel.as_ref().is_some_and(|(_, cid)| *cid == dm.id) {
                    let _call_panel_bg = ui.push_style_color(imgui::StyleColor::ChildBg, surface_bg);
                    draw_dm_call_panel(ui, state, textures, cmd_tx, dm);
                    header_height += CALL_PANEL_HEIGHT;
                }
            }
            #[cfg(feature = "screenshare")]
            if dm_channel.is_none() && state.watching_stream.is_some() {
                header_height += draw_watched_stream_panel(ui, state, textures, cmd_tx);
            }

            let line_height = ui.text_line_height_with_spacing();
            let typing_visible = !channel_key.is_empty()
                && state
                    .typing_users_for(&channel_key)
                    .into_iter()
                    .any(|id| state.current_user.as_ref().is_none_or(|u| u.id != *id));
            let extra_banner_height = (typing_visible as u32 as f32
                + state.reply_target.is_some() as u32 as f32
                + state.editing.is_some() as u32 as f32
                + state.uploads.len() as f32
                + state.voice_messages_sending.contains(&channel_key) as u32 as f32)
                * line_height
                + if state.pending_attachments.contains_key(&channel_key) { ATTACHMENT_TRAY_HEIGHT + ui.clone_style().item_spacing[1] } else { 0.0 };

            let _history_bg = ui.push_style_color(imgui::StyleColor::ChildBg, [0.0; 4]);
            ui.child_window(format!("history_{channel_key}"))
                .size([0.0, display_h - 80.0 - status_height - header_height - extra_banner_height])
                .build(|| {
                    let was_at_bottom = ui.scroll_y() >= ui.scroll_max_y() - 40.0;
                    let at_start = state.reached_start.contains(&channel_key);
                    if at_start {
                        if let Some(dm) = &dm_channel {
                            draw_conversation_start(ui, state, cmd_tx, textures, fonts, dm);
                        }
                    }
                    if rows.is_empty() && !at_start {
                        ui.text_disabled("No messages loaded yet.");
                    }

                    if state.loading_older_messages {
                        ui.text_disabled("Loading older messages...");
                    }

                    let content_x_offset = AVATAR_SIZE[0] + 12.0;
                    let scroll_target = state.scroll_to_message.clone();
                    let mut scrolled_to_target = false;

                    for row in rows {
                        let msg = &row.message;
                        if let Some(label) = &row.divider {
                            draw_day_divider(ui, &state.theme, label);
                        }
                        let message_y = ui.cursor_pos()[1];
                        let is_history_anchor = state.history_scroll_anchor.as_ref().is_some_and(|(channel, id)| {
                            channel == &channel_key && id == &msg.id
                        });
                        let is_scroll_target = scroll_target.as_deref() == Some(msg.id.as_str());
                        let highlight_start = ui.cursor_screen_pos();
                        if let Some(system) = &row.system {
                            let in_call = state.connected_voice_channel.as_ref().is_some_and(|(_, id)| *id == msg.channel_id);
                            match draw_system_row(ui, state, textures, fonts, row, system, in_call) {
                                Some(crate::message_rows::SystemAction::JoinCall) if !state.voice_connecting => {
                                    let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: msg.guild_id.clone(), channel_id: msg.channel_id.clone(), ring: false });
                                    state.pending_voice_channel = Some((msg.guild_id.clone(), msg.channel_id.clone()));
                                }
                                Some(crate::message_rows::SystemAction::JumpTo { channel_id, message_id }) => {
                                    let loaded = state.messages.get(&channel_id).is_some_and(|list| list.iter().any(|m| m.id == message_id));
                                    if loaded && state.selected_channel.as_deref() == Some(channel_id.as_str()) {
                                        state.scroll_to_message = Some(message_id);
                                    } else {
                                        let _ = cmd_tx.send(UiCommand::JumpToMessage { channel_id, message_id });
                                    }
                                }
                                _ => {}
                            }
                            ui.dummy([0.0, if row.followed_by_system { 0.0 } else { MESSAGE_GAP }]);
                            if is_history_anchor {
                                ui.set_scroll_from_pos_y_with_ratio(message_y, 0.0);
                                state.history_scroll_anchor = None;
                            }
                            continue;
                        }
                        let starts_group = row.starts_group;

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
                                let author_color = state.selected_guild.as_deref().and_then(|guild_id| {
                                    msg.member.as_ref().and_then(|member| state.color_for_roles(guild_id, &member.roles))
                                        .or_else(|| state.member_color(guild_id, &msg.author.id))
                                }).unwrap_or(state.theme.accent);
                                if clickable_text(ui, &format!("name_{}", msg.id), &author, author_color) {
                                    state.profile_popout = Some(msg.author.clone());
                                }
                                if let Some(tag) = msg.author.server_tag() {
                                    ui.same_line_with_spacing(0.0, 6.0);
                                    draw_server_tag(ui, textures, cmd_tx, tag, row.tag_badge.as_deref());
                                }
                                ui.same_line();
                                timestamp_text(ui, state.theme.text_muted, &row.times.header, &row.times.full, 0.85);

                                if !msg.content.is_empty() && !msg.is_only_gif_link() {
                                    draw_markdown(ui, fonts, &state.theme, &msg.content, &mut state.revealed_spoilers, &msg.id, textures, cmd_tx, &mut MentionCtx { known_users: &state.known_users, profiles_requested: &mut state.profiles_requested, channel_names: &channel_names, role_names: &role_names });
                                    if msg.edited_timestamp.is_some() {
                                        let edited = row.times.edited_full.as_deref().unwrap_or(&row.times.full);
                                        timestamp_text(ui, state.theme.text_muted, "(edited)", edited, 0.85);
                                    }
                                }
                                for (embed_idx, embed) in msg.embeds.iter().enumerate() {
                                    draw_embed(ui, fonts, &state.theme, embed, &format!("{}_{embed_idx}", msg.id), textures, cmd_tx, &mut state.image_lightbox, &mut state.revealed_spoilers, &mut MentionCtx { known_users: &state.known_users, profiles_requested: &mut state.profiles_requested, channel_names: &channel_names, role_names: &role_names });
                                }
                                draw_message_media(ui, fonts, state, textures, cmd_tx, msg, &channel_names, &role_names);
                                for code in &row.invites {
                                    draw_invite_card(ui, state, textures, cmd_tx, code);
                                }
                                draw_reaction_row(ui, &state.theme, textures, cmd_tx, msg);
                            });

                            let group_height = AVATAR_SIZE[1].max(ui.item_rect_size()[1]);
                            ui.set_cursor_pos([row_start[0], content_top + group_height]);
                        } else {
                            ui.set_cursor_pos([row_start[0] + content_x_offset, content_top]);
                            ui.group(|| {
                                if !msg.content.is_empty() && !msg.is_only_gif_link() {
                                    draw_markdown(ui, fonts, &state.theme, &msg.content, &mut state.revealed_spoilers, &msg.id, textures, cmd_tx, &mut MentionCtx { known_users: &state.known_users, profiles_requested: &mut state.profiles_requested, channel_names: &channel_names, role_names: &role_names });
                                    if msg.edited_timestamp.is_some() {
                                        let edited = row.times.edited_full.as_deref().unwrap_or(&row.times.full);
                                        timestamp_text(ui, state.theme.text_muted, "(edited)", edited, 0.85);
                                    }
                                }
                                for (embed_idx, embed) in msg.embeds.iter().enumerate() {
                                    draw_embed(ui, fonts, &state.theme, embed, &format!("{}_{embed_idx}", msg.id), textures, cmd_tx, &mut state.image_lightbox, &mut state.revealed_spoilers, &mut MentionCtx { known_users: &state.known_users, profiles_requested: &mut state.profiles_requested, channel_names: &channel_names, role_names: &role_names });
                                }
                                draw_message_media(ui, fonts, state, textures, cmd_tx, msg, &channel_names, &role_names);
                                for code in &row.invites {
                                    draw_invite_card(ui, state, textures, cmd_tx, code);
                                }
                                draw_reaction_row(ui, &state.theme, textures, cmd_tx, msg);
                            });
                            let after_group = [row_start[0], content_top + ui.item_rect_size()[1]];
                            let mouse = ui.io().mouse_pos;
                            let group_min = ui.item_rect_min();
                            let group_max = ui.item_rect_max();
                            if ui.is_window_hovered() && mouse[1] >= group_min[1] && mouse[1] <= group_max[1] && mouse[0] >= row_screen_start[0] {
                                ui.set_cursor_pos([row_start[0], content_top + 2.0]);
                                timestamp_text(ui, state.theme.text_muted, &row.times.short, &row.times.full, 0.75);
                            }
                            ui.set_cursor_pos(after_group);
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
                                state.reply_target = Some((**msg).clone());
                            }
                            if ui.menu_item("Forward") {
                                state.forward_dialog = Some(crate::app_state::ForwardDialog::for_message(msg));
                            }
                            if ui.menu_item("Add Reaction") {
                                state.emoji_picker_target = Some((msg.channel_id.clone(), msg.id.clone()));
                                open_emoji_picker = true;
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

                        ui.dummy([0.0, if row.ends_group { MESSAGE_GAP } else { 2.0 }]);

                        if is_history_anchor {
                            ui.set_scroll_from_pos_y_with_ratio(message_y, 0.0);
                            state.history_scroll_anchor = None;
                        }

                        if is_scroll_target {
                            let bottom = ui.cursor_screen_pos()[1];
                            let width = ui.window_size()[0];
                            ui.get_window_draw_list()
                                .add_rect(
                                    [highlight_start[0] - 4.0, highlight_start[1] - 2.0],
                                    [highlight_start[0] + width, bottom],
                                    jump_highlight(state.theme.accent),
                                )
                                .filled(true)
                                .rounding(4.0)
                                .build();
                            ui.set_scroll_here_y_with_ratio(0.5);
                            scrolled_to_target = true;
                        }
                    }

                    if scrolled_to_target {
                        state.scroll_to_message = None;
                    }

                    let snapping_to_bottom =
                        scroll_target.is_none() && (was_at_bottom || has_new_messages && seen_count == 0);
                    if snapping_to_bottom {
                        ui.set_scroll_here_y_with_ratio(1.0);
                    }

                    let scroll_y = ui.scroll_y();
                    if !snapping_to_bottom && scroll_target.is_none() && scroll_y <= 0.0 && !rows.is_empty() && !state.loading_older_messages && !state.reached_start.contains(&channel_key) {
                        if let (Some(channel_id), Some(oldest)) = (state.selected_channel.clone(), rows.first().map(|r| &r.message)) {
                            state.loading_older_messages = true;
                            state.history_scroll_anchor = Some((channel_id.clone(), oldest.id.clone()));
                            let _ = cmd_tx.send(UiCommand::LoadOlderMessages {
                                channel_id,
                                before: oldest.id.clone(),
                            });
                        }
                    }
                });
            state.last_seen_message_count.insert(channel_key.clone(), rows.len());
            if state.window_focused && !channel_key.is_empty() && !state.viewing_jumped_history {
                if let Some(newest) = rows.last().map(|r| &r.message) {
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

            if let Some(channel_id) = state.selected_channel.clone() {
                draw_attachment_tray(ui, state, textures, cmd_tx, &channel_id);
            }
            if state.selected_channel.as_ref().is_some_and(|c| state.voice_messages_sending.contains(c)) {
                ui.text_colored(state.theme.text_muted, "Sending voice message...");
            }

            let mut cancelled_upload = None;
            for upload in &state.uploads {
                let _id = ui.push_id_usize(upload.id as usize);
                ui.text_colored(state.theme.text_muted, &upload.filename);
                ui.same_line();
                let cancel_width = ui.calc_text_size("Cancel")[0] + ui.clone_style().frame_padding[0] * 2.0;
                let bar_width = (ui.content_region_avail()[0] - cancel_width - 8.0).max(60.0);
                imgui::ProgressBar::new(upload.progress.fraction()).size([bar_width, ui.text_line_height()]).build(ui);
                ui.same_line();
                if ui.small_button("Cancel") {
                    cancelled_upload = Some(upload.id);
                }
            }
            if let Some(upload_id) = cancelled_upload {
                state.uploads.retain(|u| u.id != upload_id);
                let _ = cmd_tx.send(UiCommand::CancelUpload { upload_id });
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
                .rounding(COMPOSER_ROUNDING)
                .build();

            if state.voice_recording.as_ref().is_some_and(|r| state.selected_channel.as_ref() != Some(&r.channel_id)) {
                state.finish_voice_recording(false, cmd_tx);
            }
            if state.voice_recording.is_some() {
                draw_recording_bar(ui, state, textures, cmd_tx, bar_start, bar_width);
                return;
            }
            let composer_icon = ui.frame_height();
            let attach_tint = state.selected_channel.as_ref().is_some_and(|c| state.pending_attachments.contains_key(c)).then_some(state.theme.accent);
            if icon_button_sized(ui, textures, "compose_attach", crate::icons::Icon::CirclePlus, "Upload files or create a poll", attach_tint, composer_icon) {
                ui.open_popup("compose_plus_menu");
            }
            ui.popup("compose_plus_menu", || {
                if ui.selectable("Upload files") {
                    if let (Some(channel_id), Some(paths)) = (
                        state.selected_channel.clone(),
                        rfd::FileDialog::new()
                            .add_filter("All files", &["*"])
                            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
                            .pick_files(),
                    ) {
                        for path in paths {
                            if !state.queue_attachment_path(&channel_id, path) {
                                break;
                            }
                        }
                        state.focus_compose = true;
                    }
                }
                if ui.selectable("Create poll") {
                    if let Some(channel_id) = state.selected_channel.clone() {
                        state.poll_draft = Some(crate::app_state::PollDraft::new(channel_id));
                    }
                }
            });
            let plus_width = composer_icon;
            let right_pickers = [
                ("compose_gif", crate::icons::Icon::Gif, "GIFs"),
                ("compose_sticker", crate::icons::Icon::Sticker, "Stickers"),
                ("compose_emoji", crate::icons::Icon::Smile, "Emoji"),
                ("compose_voice", crate::icons::Icon::Mic, "Record a voice message"),
            ];
            let right_width = right_pickers.len() as f32 * (composer_icon + spacing);
            ui.same_line();

            if std::mem::take(&mut state.focus_compose) {
                ui.set_keyboard_focus_here();
            }
            let before_edit = state.compose_buf.clone();
            ui.set_next_item_width((bar_width - plus_width - right_width - spacing).max(40.0));
            let enter_pressed_raw = ui
                .input_text("##compose", &mut state.compose_buf)
                .hint(&state.compose_placeholder)
                .enter_returns_true(true)
                .build();
            let input_focused = ui.is_item_focused();
            let emoji_autocomplete_active = trailing_emoji_token(&state.compose_buf).is_some();
            let mention_autocomplete_active = !emoji_autocomplete_active && trailing_mention_token(&state.compose_buf).is_some();

            let mut enter_pressed = enter_pressed_raw;
            if input_focused && emoji_autocomplete_active {
                enter_pressed = false;
                if let Some((token_start, insert_text)) = draw_emoji_autocomplete(ui, state, textures, cmd_tx, bar_start) {
                    state.record_emoji_use(&insert_text);
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

            for (id, icon, tooltip) in right_pickers {
                ui.same_line();
                if icon_button_sized(ui, textures, id, icon, tooltip, None, composer_icon) {
                    match icon {
                        crate::icons::Icon::Gif => ui.open_popup("gif_picker"),
                        crate::icons::Icon::Sticker => {
                            state.request_stickers(cmd_tx);
                            ui.open_popup("sticker_picker");
                        }
                        crate::icons::Icon::Mic => {
                            if let Some(channel_id) = state.selected_channel.clone() {
                                state.start_voice_recording(channel_id);
                            }
                        }
                        _ => ui.open_popup("emoji_picker"),
                    }
                }
            }
            draw_gif_picker(ui, state, textures, cmd_tx);
            draw_sticker_picker(ui, state, textures, cmd_tx);
            if open_emoji_picker {
                ui.open_popup("emoji_picker");
            }
            draw_emoji_picker(ui, state, textures, cmd_tx);

            let compose_focused = input_focused;
            if compose_focused && ui.io().key_ctrl && ui.is_key_pressed(imgui::Key::V) {
                if let Some(channel_id) = state.selected_channel.clone() {
                    if let Some((filename, bytes)) = crate::clipboard::read_image_as_png() {
                        let source = crate::app_state::AttachmentSource::Bytes { bytes: std::sync::Arc::new(bytes), content_type: "image/png".to_string() };
                        state.queue_attachment(&channel_id, filename, source);
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
                    let files = state.take_attachments(&channel_id);
                    if !files.is_empty() {
                        let label = match files.as_slice() {
                            [only] => only.filename.clone(),
                            many => format!("{} files", many.len()),
                        };
                        let (upload_id, progress) = state.start_upload(label);
                        let _ = cmd_tx.send(UiCommand::SendAttachments { channel_id, content, files, reply_to, upload_id, progress });
                    } else if !content.trim().is_empty() {
                        let _ = cmd_tx.send(UiCommand::SendMessage { channel_id, content, reply_to });
                    }
                }
            }
        });

    if search_panel_open {
        let _search_bg = ui.push_style_color(imgui::StyleColor::WindowBg, state.theme.panel);
        ui.window("search_panel")
            .position([display_w - SEARCH_PANEL_WIDTH, 0.0], imgui::Condition::Always)
            .size([SEARCH_PANEL_WIDTH, display_h], imgui::Condition::Always)
            .no_decoration()
            .draw_background(true)
            .build(|| {
                draw_search_panel(ui, state, cmd_tx);
            });
        _search_bg.pop();
    }

    if let Some(user) = &profile_panel_user {
        let _profile_bg = ui.push_style_color(imgui::StyleColor::WindowBg, state.theme.panel);
        ui.window("profile_panel")
            .position([display_w - PROFILE_PANEL_WIDTH, 0.0], imgui::Condition::Always)
            .size([PROFILE_PANEL_WIDTH, display_h], imgui::Condition::Always)
            .no_decoration()
            .draw_background(true)
            .build(|| {
                draw_profile_panel(ui, state, cmd_tx, textures, fonts, user);
            });
        _profile_bg.pop();
    }

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
    #[cfg(feature = "video")]
    draw_video_fullscreen(ui, textures, cmd_tx);
    draw_incoming_calls(ui, state, cmd_tx);
    draw_voice_info(ui, state);
    draw_quick_switcher(ui, state, cmd_tx, textures, display_w, display_h);
    draw_channel_editor(ui, state, cmd_tx, display_w, display_h);
    draw_invite_manager(ui, state, cmd_tx, display_w, display_h);
    draw_poll_creator(ui, state, cmd_tx);
    draw_forward_dialog(ui, state, cmd_tx);
}

fn format_bitrate(bps: u32) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f32 / 1_000_000.0)
    } else {
        format!("{} kbps", bps / 1000)
    }
}

fn signal_quality(rtt_ms: Option<f32>) -> [f32; 4] {
    match rtt_ms {
        None => [0.55, 0.55, 0.60, 1.0],
        Some(ms) if ms <= 120.0 => [0.26, 0.83, 0.44, 1.0],
        Some(ms) if ms <= 250.0 => [0.96, 0.74, 0.30, 1.0],
        Some(_) => [0.94, 0.33, 0.36, 1.0],
    }
}

#[cfg(feature = "screenshare")]
fn voice_panel_button(ui: &Ui, textures: &TextureCache, id: &str, icon: crate::icons::Icon, tooltip: &str, active: bool, size: [f32; 2]) -> bool {
    let clicked = ui.invisible_button(id, size);
    let hovered = ui.is_item_hovered();
    let (p_min, p_max) = (ui.item_rect_min(), ui.item_rect_max());
    let base = ui.style_color(imgui::StyleColor::Button);
    let fill = match (active, hovered) {
        (true, _) => [0.18, 0.58, 0.34, 1.0],
        (false, true) => ui.style_color(imgui::StyleColor::ButtonHovered),
        (false, false) => base,
    };
    ui.get_window_draw_list().add_rect(p_min, p_max, fill).filled(true).rounding(8.0).build();
    let glyph = (size[1] * 0.55).round();
    draw_icon_at(
        ui,
        textures,
        icon,
        [p_min[0] + (size[0] - glyph) / 2.0, p_min[1] + (size[1] - glyph) / 2.0],
        glyph,
        ui.style_color(imgui::StyleColor::Text),
    );
    if hovered {
        ui.tooltip_text(tooltip);
    }
    clicked
}

fn draw_voice_panel(ui: &Ui, state: &mut AppState, textures: &TextureCache, cmd_tx: &UnboundedSender<UiCommand>, open_call_view: bool) {
    const GREEN: [f32; 4] = [0.26, 0.83, 0.44, 1.0];
    const RED: [f32; 4] = [0.94, 0.33, 0.36, 1.0];
    let Some((guild_id, channel_id)) = state.connected_voice_channel.clone() else { return };
    #[cfg(not(feature = "screenshare"))]
    let _ = &channel_id;
    let channel_name = state.connected_voice_channel_name().unwrap_or_else(|| channel_id.clone());
    let location = match state.connected_voice_server_name() {
        Some(server) => format!("{channel_name} / {server}"),
        None => channel_name,
    };
    let stats = &state.voice_stats;
    let rtt = stats.rtc_rtt_ms.or(stats.ws_rtt_ms.map(|ms| ms as f32));
    let signal_tip = format!(
        "{}\nup {} · down {}\nClick for connection info",
        rtt.map_or_else(|| "Connecting…".to_string(), |ms| format!("{ms:.0} ms ping")),
        format_bitrate(stats.bitrate_up_bps),
        format_bitrate(stats.bitrate_down_bps),
    );
    let width = ui.content_region_avail()[0];
    let top = ui.cursor_pos();
    let icon = 30.0;
    ui.set_cursor_pos([top[0], top[1] + (VOICE_PANEL_ROW - icon) / 2.0]);
    if icon_button_sized(ui, textures, "vc_signal", crate::icons::Icon::Signal, &signal_tip, Some(signal_quality(rtt)), icon) {
        state.show_voice_info = !state.show_voice_info;
    }

    let right_buttons = icon * 2.0 + 4.0;
    let text_x = top[0] + icon + 8.0;
    let text_width = (width - icon - 8.0 - right_buttons - 8.0).max(40.0);
    let line = ui.text_line_height();
    let text_top = top[1] + (VOICE_PANEL_ROW - line * 2.0 - 2.0) / 2.0;
    ui.set_cursor_pos([text_x, text_top]);
    let title_clicked = ui.invisible_button("##vc_title", [text_width, line * 2.0 + 2.0]);
    let title_hovered = ui.is_item_hovered();
    let title_min = ui.item_rect_min();
    {
        let dl = ui.get_window_draw_list();
        let title = if title_hovered && open_call_view { "Open call" } else { "Voice Connected" };
        dl.add_text(title_min, GREEN, title);
        let shown = truncate_to_width(ui, &location, text_width).unwrap_or_else(|| location.clone());
        dl.add_text([title_min[0], title_min[1] + line + 2.0], ui.style_color(imgui::StyleColor::TextDisabled), shown);
    }
    if title_hovered && !location.is_empty() {
        ui.tooltip_text(&location);
    }
    if title_clicked && open_call_view {
        state.server_call_minimized = false;
    }

    ui.set_cursor_pos([top[0] + width - right_buttons, top[1] + (VOICE_PANEL_ROW - icon) / 2.0]);
    let denoise = state.denoise;
    let denoise_tip = if denoise { "Noise suppression: on" } else { "Noise suppression: off" };
    if icon_button_sized(ui, textures, "vc_rnn", crate::icons::Icon::AudioWaveform, denoise_tip, denoise.then_some(GREEN), icon) {
        state.denoise = !state.denoise;
        persist_settings(state);
        let _ = cmd_tx.send(UiCommand::SetMicProcessing { gain: state.mic_gain, denoise: state.denoise });
    }
    ui.same_line_with_spacing(0.0, 4.0);
    if icon_button_sized(ui, textures, "vc_disconnect", crate::icons::Icon::PhoneOff, "Disconnect", Some(RED), icon) {
        let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id: guild_id.clone() });
    }

    #[cfg(feature = "screenshare")]
    {
        let gap = 8.0;
        let button = [(width - gap) / 2.0, VOICE_PANEL_BUTTON];
        ui.set_cursor_pos([top[0], top[1] + VOICE_PANEL_ROW + 8.0]);
        if voice_panel_button(ui, textures, "vc_camera", crate::icons::Icon::Video, camera_tooltip(state.camera_on), state.camera_on, button) {
            toggle_camera(state, cmd_tx);
        }
        ui.same_line_with_spacing(0.0, gap);
        let sharing = state.screen_sharing;
        if voice_panel_button(ui, textures, "vc_screenshare", SCREENSHARE_ICON, if sharing { "Stop screen sharing" } else { "Share your screen" }, sharing, button) {
            if sharing {
                state.screen_sharing = false;
                let _ = cmd_tx.send(UiCommand::SetScreenSharing { enabled: false, include_system_audio: state.share_system_audio, audio_source_pid: state.selected_screen_audio_pid, max_height: state.screen_share_max_height, framerate: state.screen_share_fps, guild_id: guild_id.clone(), channel_id: channel_id.clone() });
            } else {
                state.show_screen_share_setup = true;
            }
        }
    }
}

fn draw_user_bar(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    let Some(user) = state.current_user.clone() else { return };
    const PANEL_ICON: f32 = 30.0;
    const CHEVRON_WIDTH: f32 = 14.0;
    const CONTROLS_WIDTH: f32 = PANEL_ICON * 3.0 + CHEVRON_WIDTH * 2.0 + 8.0;
    let avatar_url =
        rustcord_client::cdn::avatar_url(&user, 64).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
    let bar_start = ui.cursor_pos();
    let bar_screen = ui.cursor_screen_pos();
    let width = ui.content_region_avail()[0];

    let clicked = ui
        .selectable_config("##user_bar")
        .size([(width - CONTROLS_WIDTH).max(40.0), USER_BAR_HEIGHT])
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
    let name_room = (width - CONTROLS_WIDTH - 46.0).max(20.0);
    ui.set_cursor_pos([bar_start[0] + 42.0, bar_start[1] + 3.0]);
    ui.text(truncate_to_width(ui, &display_name, name_room).as_deref().unwrap_or(&display_name));
    ui.set_cursor_pos([bar_start[0] + 42.0, bar_start[1] + 3.0 + ui.text_line_height()]);
    let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
    ui.set_window_font_scale(0.85);
    ui.text(truncate_to_width(ui, &status_label, name_room).as_deref().unwrap_or(&status_label));
    ui.set_window_font_scale(1.0);
    token.pop();

    let red = state.theme.mention_badge();
    ui.set_cursor_pos([bar_start[0] + width - CONTROLS_WIDTH + 4.0, bar_start[1] + (USER_BAR_HEIGHT - PANEL_ICON) / 2.0]);
    let (mic_icon, mic_tip) = if state.self_muted { (crate::icons::Icon::MicOff, "Unmute") } else { (crate::icons::Icon::Mic, "Mute") };
    if icon_button_sized(ui, textures, "user_mute", mic_icon, mic_tip, state.self_muted.then_some(red), PANEL_ICON) {
        toggle_voice(state, cmd_tx, VoiceToggle::Mute);
    }
    ui.same_line_with_spacing(0.0, 0.0);
    if chevron_button(ui, textures, "user_input_menu", "Input device", CHEVRON_WIDTH, PANEL_ICON) {
        if state.voice_input_devices.is_empty() {
            let _ = cmd_tx.send(UiCommand::LoadVoiceDevices);
        }
        ui.open_popup("input_device_menu");
    }
    ui.same_line_with_spacing(0.0, 0.0);
    let (deaf_icon, deaf_tip) = if state.self_deafened { (crate::icons::Icon::HeadphoneOff, "Undeafen") } else { (crate::icons::Icon::Headphones, "Deafen") };
    if icon_button_sized(ui, textures, "user_deafen", deaf_icon, deaf_tip, state.self_deafened.then_some(red), PANEL_ICON) {
        toggle_voice(state, cmd_tx, VoiceToggle::Deafen);
    }
    ui.same_line_with_spacing(0.0, 0.0);
    if chevron_button(ui, textures, "user_output_menu", "Output device", CHEVRON_WIDTH, PANEL_ICON) {
        if state.voice_output_devices.is_empty() {
            let _ = cmd_tx.send(UiCommand::LoadVoiceDevices);
        }
        ui.open_popup("output_device_menu");
    }
    ui.popup("input_device_menu", || draw_device_menu(ui, state, cmd_tx, DeviceKind::Input));
    ui.popup("output_device_menu", || draw_device_menu(ui, state, cmd_tx, DeviceKind::Output));
    ui.same_line_with_spacing(0.0, 0.0);
    if icon_button_sized(ui, textures, "user_settings", crate::icons::Icon::Settings, "User Settings", state.show_settings.then_some(state.theme.accent), PANEL_ICON) {
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

const SYSTEM_ICON_SIZE: f32 = 16.0;
const START_AVATAR_SIZE: f32 = 80.0;
const START_MUTUAL_ICON: f32 = 20.0;

fn draw_bold_sentence(ui: &Ui, fonts: &Fonts, muted: [f32; 4], strong: [f32; 4], before: &str, name: &str, after: &str) {
    ui.text_colored(muted, before);
    ui.same_line_with_spacing(0.0, 0.0);
    let bold = ui.push_font(fonts.bold);
    ui.text_colored(strong, name);
    bold.pop();
    ui.same_line_with_spacing(0.0, 0.0);
    ui.text_colored(muted, after);
}

fn draw_start_avatar(ui: &Ui, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, url: Option<&str>, fallback: [f32; 4]) {
    match url.and_then(|url| textures.get(url)) {
        Some((texture, _)) => draw_circular_image(ui, texture, [START_AVATAR_SIZE; 2]),
        None => {
            if let Some(url) = url {
                textures.request(url, cmd_tx);
            }
            let min = ui.cursor_screen_pos();
            let radius = START_AVATAR_SIZE / 2.0;
            ui.get_window_draw_list()
                .add_circle([min[0] + radius, min[1] + radius], radius, fallback)
                .filled(true)
                .num_segments(48)
                .build();
            let icon = 36.0;
            let inset = (START_AVATAR_SIZE - icon) / 2.0;
            draw_icon_at(ui, textures, crate::icons::Icon::Users, [min[0] + inset, min[1] + inset], icon, [1.0, 1.0, 1.0, 1.0]);
            ui.dummy([START_AVATAR_SIZE; 2]);
        }
    }
}

fn draw_conversation_start(
    ui: &Ui,
    state: &mut AppState,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    fonts: &Fonts,
    dm: &rustcord_client::models::Channel,
) {
    use crate::backend::RelationshipAction;
    use rustcord_client::models::{ChannelType, RelationshipType};

    let muted = state.theme.text_muted;
    let strong = state.theme.text;
    let accent = state.theme.accent;
    let left = ui.cursor_pos()[0] + 24.0;
    ui.dummy([0.0, 16.0]);
    let indent = |ui: &Ui| ui.set_cursor_pos([left, ui.cursor_pos()[1]]);

    if dm.kind == ChannelType::GroupDm {
        let name = AppState::dm_display_name(dm);
        let icon_url = dm.icon.as_deref().map(|hash| rustcord_client::cdn::channel_icon_url(&dm.id, hash, 128));
        indent(ui);
        draw_start_avatar(ui, textures, cmd_tx, icon_url.as_deref(), accent);
        indent(ui);
        let bold = ui.push_font(fonts.bold);
        ui.set_window_font_scale(1.6);
        ui.text_colored(strong, &name);
        ui.set_window_font_scale(1.0);
        bold.pop();
        indent(ui);
        draw_bold_sentence(ui, fonts, muted, strong, "Welcome to the beginning of the ", &name, " group.");
        ui.dummy([0.0, 16.0]);
        return;
    }

    let Some(user) = dm.recipients.first() else { return };
    let display = user.global_name.as_deref().filter(|n| !n.is_empty()).unwrap_or(&user.username);
    let avatar = rustcord_client::cdn::avatar_url(user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
    indent(ui);
    draw_start_avatar(ui, textures, cmd_tx, Some(&avatar), accent);
    indent(ui);
    let bold = ui.push_font(fonts.bold);
    ui.set_window_font_scale(1.6);
    ui.text_colored(strong, display);
    ui.set_window_font_scale(1.0);
    bold.pop();
    indent(ui);
    ui.set_window_font_scale(1.25);
    ui.text_colored(strong, &user.username);
    ui.set_window_font_scale(1.0);
    ui.dummy([0.0, 4.0]);
    indent(ui);
    draw_bold_sentence(ui, fonts, muted, strong, "This is the beginning of your direct message history with ", display, ".");
    ui.dummy([0.0, 2.0]);
    indent(ui);

    if !state.profiles_requested.contains(&user.id) {
        state.profiles_requested.insert(user.id.clone());
        let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: user.id.clone() });
    }
    let mut started_row = false;
    let profile = state.profiles.get(&user.id).filter(|p| !p.mutual_guild_ids.is_empty());
    if let (Some(profile), Some(label)) = (profile, state.mutual_server_labels.get(&user.id)) {
        let row_y = ui.cursor_screen_pos()[1];
        let icon_y = row_y + (ui.frame_height() - START_MUTUAL_ICON) / 2.0;
        let mut x = ui.cursor_screen_pos()[0];
        for guild in profile.mutual_guild_ids.iter().take(3).filter_map(|id| state.guilds.iter().find(|g| &g.id == id)) {
            if let Some(url) = rustcord_client::cdn::guild_icon_url(guild, 64) {
                match textures.get(&url) {
                    Some((texture, _)) => {
                        ui.get_window_draw_list()
                            .add_image_rounded(texture, [x, icon_y], [x + START_MUTUAL_ICON, icon_y + START_MUTUAL_ICON], START_MUTUAL_ICON / 2.0)
                            .build();
                    }
                    None => textures.request(&url, cmd_tx),
                }
            }
            x += START_MUTUAL_ICON - 4.0;
        }
        if x > ui.cursor_screen_pos()[0] {
            ui.set_cursor_screen_pos([x + 8.0, row_y]);
        }
        let text_y = ui.cursor_pos()[1] + (ui.frame_height() - ui.text_line_height()) / 2.0;
        ui.set_cursor_pos([ui.cursor_pos()[0], text_y]);
        if clickable_text(ui, "mutual_servers_link", label, muted) {
            ui.open_popup("mutual_servers");
        }
        if ui.is_item_hovered() {
            ui.set_mouse_cursor(Some(imgui::MouseCursor::Hand));
        }
        ui.same_line();
        ui.text_colored(muted, "\u{2022}");
        ui.same_line();
        started_row = true;
        let mut picked = None;
        ui.popup("mutual_servers", || {
            ui.text_disabled("Mutual Servers");
            ui.separator();
            for id in &profile.mutual_guild_ids {
                let Some(guild) = state.guilds.iter().find(|g| &g.id == id) else { continue };
                if ui.selectable(&guild.name) {
                    picked = Some(guild.id.clone());
                }
            }
        });
        if let Some(guild_id) = picked {
            let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild_id.clone() });
            state.selected_guild = Some(guild_id);
            state.selected_channel = None;
        }
    }
    if !started_row {
        indent(ui);
    }

    if !user.bot {
        let relationship = state.relationship_with(&user.id);
        let busy = state.relationship_busy.contains(&user.id);
        let mut request = None;
        let _button = ui.push_style_color(imgui::StyleColor::Button, [muted[0], muted[1], muted[2], 0.22]);
        let disabled = ui.begin_disabled(busy);
        let friend_button = match relationship {
            Some(RelationshipType::Friend) => Some(("Remove Friend", Some(RelationshipAction::Remove))),
            Some(RelationshipType::OutgoingRequest) => Some(("Friend Request Sent", None)),
            Some(RelationshipType::IncomingRequest) => Some(("Accept Friend Request", Some(RelationshipAction::SendRequest))),
            Some(RelationshipType::Blocked) => None,
            None => Some(("Add Friend", Some(RelationshipAction::SendRequest))),
        };
        if let Some((label, action)) = friend_button {
            let inert = ui.begin_disabled(action.is_none());
            if ui.button(label) {
                request = action;
            }
            drop(inert);
            ui.same_line();
        }
        let block_action = if relationship == Some(RelationshipType::Blocked) { RelationshipAction::Unblock } else { RelationshipAction::Block };
        if ui.button(if block_action == RelationshipAction::Unblock { "Unblock" } else { "Block" }) {
            request = Some(block_action);
        }
        drop(disabled);

        match request {
            Some(action @ (RelationshipAction::Remove | RelationshipAction::Block)) => {
                state.relationship_confirm = Some((action, user.id.clone(), display.to_string()));
                ui.open_popup("Confirm##relationship");
            }
            Some(action) => {
                state.relationship_busy.insert(user.id.clone());
                let _ = cmd_tx.send(UiCommand::UpdateRelationship { user_id: user.id.clone(), action });
            }
            None => {}
        }
    }

    if let Some(_modal) = ui.modal_popup_config("Confirm##relationship").always_auto_resize(true).begin_popup() {
        let mut close = false;
        if let Some((action, user_id, name)) = state.relationship_confirm.clone() {
            let (title, body, confirm) = if action == RelationshipAction::Block {
                ("Block", "They won't be able to message you, and you won't see their messages.", "Block")
            } else {
                ("Remove Friend", "You'll need to send a new friend request to be friends again.", "Remove Friend")
            };
            let bold = ui.push_font(fonts.bold);
            ui.text(format!("{title} {name}?"));
            bold.pop();
            ui.text_colored(muted, body);
            ui.dummy([0.0, 6.0]);
            if ui.button("Cancel") {
                close = true;
            }
            ui.same_line();
            let _danger = ui.push_style_color(imgui::StyleColor::Button, state.theme.mention_badge());
            if ui.button(confirm) {
                state.relationship_busy.insert(user_id.clone());
                let _ = cmd_tx.send(UiCommand::UpdateRelationship { user_id, action });
                close = true;
            }
        } else {
            close = true;
        }
        if close {
            state.relationship_confirm = None;
            ui.close_current_popup();
        }
    }
    ui.dummy([0.0, 16.0]);
}
const INVITE_CARD_WIDTH: f32 = 400.0;
const INVITE_CARD_HEIGHT: f32 = 76.0;
const INVITE_ICON_SIZE: f32 = 48.0;

enum InviteAction {
    JoinGuild,
    JoinVoice { guild_id: String, channel_id: String, member: bool },
}

fn draw_invite_card(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, code: &str) {
    use crate::invites::InviteState;
    let _id = ui.push_id(code);
    ui.dummy([0.0, 2.0]);
    let start = ui.cursor_pos();
    let origin = ui.cursor_screen_pos();
    let width = INVITE_CARD_WIDTH.min(ui.content_region_avail()[0]).max(200.0);
    let max = [origin[0] + width, origin[1] + INVITE_CARD_HEIGHT];
    if ui.is_rect_visible(origin, max) {
        state.ensure_invite(code, cmd_tx);
    }
    let panel = state.theme.panel;
    let text_color = state.theme.text;
    let muted = state.theme.text_muted;
    let accent = state.theme.accent;
    let call_green = state.theme.call_active();
    ui.get_window_draw_list().add_rect(origin, max, panel).filled(true).rounding(8.0).build();
    let icon_min = [origin[0] + 14.0, origin[1] + (INVITE_CARD_HEIGHT - INVITE_ICON_SIZE) / 2.0];
    let icon_max = [icon_min[0] + INVITE_ICON_SIZE, icon_min[1] + INVITE_ICON_SIZE];
    let text_x = start[0] + 14.0 + INVITE_ICON_SIZE + 14.0;
    let line = ui.text_line_height_with_spacing();
    let text_top = start[1] + (INVITE_CARD_HEIGHT - line * 2.0) / 2.0;
    let button_y = start[1] + (INVITE_CARD_HEIGHT - ui.frame_height()) / 2.0;
    let mut action = None;

    match state.invites.get(code).map(|entry| &entry.state) {
        None | Some(InviteState::Loading) => {
            let skeleton = [muted[0], muted[1], muted[2], 0.18];
            let dl = ui.get_window_draw_list();
            dl.add_rect(icon_min, icon_max, skeleton).filled(true).rounding(12.0).build();
            let bar_x = icon_max[0] + 14.0;
            dl.add_rect([bar_x, origin[1] + 22.0], [bar_x + 140.0, origin[1] + 36.0], skeleton).filled(true).rounding(4.0).build();
            dl.add_rect([bar_x, origin[1] + 44.0], [bar_x + 90.0, origin[1] + 54.0], skeleton).filled(true).rounding(4.0).build();
        }
        Some(InviteState::Invalid) | Some(InviteState::Unavailable) => {
            let invalid = matches!(state.invites.get(code).map(|e| &e.state), Some(InviteState::Invalid));
            let tint = [muted[0], muted[1], muted[2], 0.6];
            ui.get_window_draw_list().add_rect(icon_min, icon_max, [tint[0], tint[1], tint[2], 0.12]).filled(true).rounding(12.0).build();
            draw_icon_at(ui, textures, crate::icons::Icon::CircleHelp, [icon_min[0] + 12.0, icon_min[1] + 12.0], 24.0, tint);
            ui.set_cursor_pos([text_x, text_top]);
            ui.text_colored(muted, if invalid { "Invite invalid" } else { "Couldn't load invite" });
            ui.set_cursor_pos([text_x, text_top + line]);
            ui.set_window_font_scale(0.85);
            ui.text_colored(
                muted,
                if invalid { "This invite may be expired, or you might not have permission to join." } else { "Try again in a few minutes." },
            );
            ui.set_window_font_scale(1.0);
        }
        Some(InviteState::Ready(resolved)) => {
            let guild = resolved.invite.guild.as_ref();
            let guild_id = guild.map(|g| g.id.as_str()).unwrap_or_default();
            let member = state.guilds.iter().any(|g| g.id == guild_id);
            let voice = resolved.invite.channel.as_ref().filter(|c| c.is_voice());
            match voice {
                Some(channel) => {
                    ui.get_window_draw_list().add_rect(icon_min, icon_max, [accent[0], accent[1], accent[2], 0.18]).filled(true).rounding(12.0).build();
                    draw_icon_at(ui, textures, crate::icons::Icon::Volume, [icon_min[0] + 12.0, icon_min[1] + 12.0], 24.0, text_color);
                    ui.set_cursor_pos([text_x, text_top]);
                    ui.text(&resolved.channel_name);
                    ui.set_cursor_pos([text_x, text_top + line]);
                    ui.set_window_font_scale(0.85);
                    if member {
                        let occupants = state.members_in_voice_channel(guild_id, &channel.id).len();
                        if occupants == 0 {
                            ui.text_colored(muted, "No one is currently in voice");
                        } else {
                            ui.text_colored(call_green, format!("{occupants} in voice"));
                        }
                    } else {
                        ui.text_colored(muted, "Only server members can see who's in voice");
                    }
                    ui.set_window_font_scale(1.0);
                    let in_channel = state.connected_voice_channel.as_ref().is_some_and(|(_, id)| *id == channel.id);
                    ui.set_cursor_pos([start[0] + width - 108.0, button_y]);
                    let _green = ui.push_style_color(imgui::StyleColor::Button, call_green);
                    let disabled = ui.begin_disabled(in_channel || state.voice_connecting);
                    if ui.button_with_size(if in_channel { "Connected" } else { "Join Voice" }, [96.0, 0.0]) {
                        action = Some(InviteAction::JoinVoice { guild_id: guild_id.to_string(), channel_id: channel.id.clone(), member });
                    }
                    drop(disabled);
                }
                None => {
                    match resolved.icon_url.as_deref().and_then(|url| textures.get(url).map(|t| (url, t))) {
                        Some((_, (texture, _))) => {
                            ui.get_window_draw_list().add_image_rounded(texture, icon_min, icon_max, 12.0).build();
                        }
                        None => {
                            if let Some(url) = resolved.icon_url.as_deref() {
                                textures.request(url, cmd_tx);
                            }
                            let dl = ui.get_window_draw_list();
                            dl.add_rect(icon_min, icon_max, accent).filled(true).rounding(12.0).build();
                            let size = ui.calc_text_size(&resolved.initials);
                            dl.add_text(
                                [icon_min[0] + (INVITE_ICON_SIZE - size[0]) / 2.0, icon_min[1] + (INVITE_ICON_SIZE - size[1]) / 2.0],
                                text_color,
                                &resolved.initials,
                            );
                        }
                    }
                    ui.set_cursor_pos([text_x, text_top]);
                    let name = guild.map(|g| g.name.as_str()).unwrap_or("Unknown server");
                    ui.text(name);
                    ui.set_cursor_pos([text_x, text_top + line]);
                    ui.set_window_font_scale(0.85);
                    ui.text_colored(muted, &resolved.counts);
                    ui.set_window_font_scale(1.0);
                    ui.set_cursor_pos([start[0] + width - 88.0, button_y]);
                    if member {
                        let disabled = ui.begin_disabled(true);
                        ui.button_with_size("Joined", [76.0, 0.0]);
                        drop(disabled);
                    } else {
                        let _green = ui.push_style_color(imgui::StyleColor::Button, call_green);
                        if ui.button_with_size("Join", [76.0, 0.0]) {
                            action = Some(InviteAction::JoinGuild);
                        }
                    }
                }
            }
        }
    }

    match action {
        Some(InviteAction::JoinGuild) => {
            let _ = cmd_tx.send(UiCommand::JoinGuildByInvite { invite_code: code.to_string() });
        }
        Some(InviteAction::JoinVoice { guild_id, channel_id, member: true }) => {
            let _ = cmd_tx.send(UiCommand::JoinVoiceChannel { guild_id: Some(guild_id.clone()), channel_id: channel_id.clone(), ring: false });
            state.pending_voice_channel = Some((Some(guild_id), channel_id));
        }
        Some(InviteAction::JoinVoice { guild_id, channel_id, member: false }) => {
            let _ = cmd_tx.send(UiCommand::JoinGuildByInvite { invite_code: code.to_string() });
            state.pending_invite_voice = Some((guild_id, channel_id));
        }
        None => {}
    }
    ui.set_cursor_pos(start);
    ui.dummy([width, INVITE_CARD_HEIGHT]);
}

fn draw_system_row(
    ui: &Ui,
    state: &AppState,
    textures: &TextureCache,
    fonts: &Fonts,
    row: &crate::message_rows::MessageRow,
    system: &crate::message_rows::SystemRow,
    in_call: bool,
) -> Option<crate::message_rows::SystemAction> {
    use crate::message_rows::{SystemAction, Tone};
    let theme = &state.theme;
    let tint = match system.tone {
        Tone::CallActive | Tone::Positive => theme.call_active(),
        Tone::CallMissed => theme.call_missed(),
        Tone::Muted => theme.text_muted,
    };
    let line = ui.text_line_height();
    let origin = ui.cursor_pos();
    let icon_x = origin[0] + 24.0 + (AVATAR_SIZE[0] - SYSTEM_ICON_SIZE) / 2.0;
    ui.set_cursor_pos([icon_x, origin[1] + (line - SYSTEM_ICON_SIZE) / 2.0]);
    draw_icon(ui, textures, system.icon, SYSTEM_ICON_SIZE, tint);
    ui.set_cursor_pos([origin[0] + 24.0 + AVATAR_SIZE[0] + 12.0, origin[1]]);

    let text_min = ui.cursor_screen_pos();
    for (idx, (text, bold)) in system.parts.iter().enumerate() {
        if idx > 0 {
            ui.same_line_with_spacing(0.0, 0.0);
        }
        if *bold {
            let font = ui.push_font(fonts.bold);
            ui.text_colored(theme.text, text);
            font.pop();
        } else {
            ui.text_colored(theme.text_muted, text);
        }
    }
    let text_max = ui.item_rect_max();
    let mouse = ui.io().mouse_pos;
    let text_hovered = ui.is_window_hovered()
        && mouse[0] >= text_min[0]
        && mouse[0] <= text_max[0]
        && mouse[1] >= text_min[1]
        && mouse[1] <= text_max[1];
    let mut action = None;
    if text_hovered {
        if let Some(tooltip) = &system.tooltip {
            ui.tooltip_text(tooltip);
        }
        if let Some(jump @ SystemAction::JumpTo { .. }) = &system.action {
            ui.set_mouse_cursor(Some(imgui::MouseCursor::Hand));
            if ui.is_mouse_clicked(imgui::MouseButton::Left) {
                action = Some(jump.clone());
            }
        }
    }
    ui.same_line_with_spacing(0.0, 6.0);
    timestamp_text(ui, theme.text_muted, &row.times.inline, &row.times.full, 0.85);
    if system.action == Some(SystemAction::JoinCall) && !in_call {
        ui.same_line_with_spacing(0.0, 8.0);
        let _id = ui.push_id(row.message.id.as_str());
        if ui.small_button("Join") {
            action = Some(SystemAction::JoinCall);
        }
    }
    action
}

fn draw_icon(ui: &Ui, textures: &TextureCache, icon: crate::icons::Icon, size: f32, tint: [f32; 4]) {
    match textures.icon(icon, size) {
        Some(texture) => imgui::Image::new(texture, [size, size]).tint_col(tint).build(ui),
        None => ui.dummy([size, size]),
    }
}

fn draw_icon_at(ui: &Ui, textures: &TextureCache, icon: crate::icons::Icon, min: [f32; 2], size: f32, tint: [f32; 4]) {
    if let Some(texture) = textures.icon(icon, size) {
        ui.get_window_draw_list().add_image(texture, min, [min[0] + size, min[1] + size]).col(tint).build();
    }
}

#[derive(Clone, Copy)]
enum VoiceToggle {
    Mute,
    Deafen,
}

fn persist_settings(state: &AppState) {
    crate::settings::save(&crate::settings::Settings {
        theme_file: current_theme_file(&state.theme),
        input_device: state.selected_input_device.clone(),
        output_device: state.selected_output_device.clone(),
        ..state.audio_settings()
    });
}

#[cfg(feature = "screenshare")]
fn toggle_camera(state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some((guild_id, channel_id)) = state.connected_voice_channel.clone() else { return };
    state.camera_on = !state.camera_on;
    let _ = cmd_tx.send(UiCommand::SetCamera {
        enabled: state.camera_on,
        guild_id,
        channel_id,
        self_mute: state.self_muted,
        self_deaf: state.self_deafened,
    });
}

#[cfg(feature = "screenshare")]
fn camera_tooltip(on: bool) -> &'static str {
    if on { "Turn off camera" } else { "Turn on camera" }
}

fn toggle_voice(state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, toggle: VoiceToggle) {
    match toggle {
        VoiceToggle::Mute => state.toggle_mute(),
        VoiceToggle::Deafen => state.toggle_deafen(),
    }
    persist_settings(state);
    if let Some((guild_id, channel_id)) = state.connected_voice_channel.clone() {
        let _ = cmd_tx.send(UiCommand::SetVoiceMuteDeaf {
            guild_id,
            channel_id,
            self_mute: state.self_muted,
            self_deaf: state.self_deafened,
        });
    }
}

fn chevron_button(ui: &Ui, textures: &TextureCache, id: &str, tooltip: &str, width: f32, height: f32) -> bool {
    let p = ui.cursor_screen_pos();
    let clicked = ui.invisible_button(id, [width, height]);
    let hovered = ui.is_item_hovered();
    let tint = ui.style_color(if hovered { imgui::StyleColor::Text } else { imgui::StyleColor::TextDisabled });
    let size = width;
    draw_icon_at(ui, textures, crate::icons::Icon::ChevronDown, [p[0], p[1] + (height - size) / 2.0], size, tint);
    if hovered {
        ui.tooltip_text(tooltip);
    }
    clicked
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    Input,
    Output,
}

fn draw_device_menu(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, kind: DeviceKind) {
    let (title, devices, selected) = match kind {
        DeviceKind::Input => ("Input Device", &state.voice_input_devices, &state.selected_input_device),
        DeviceKind::Output => ("Output Device", &state.voice_output_devices, &state.selected_output_device),
    };
    ui.text_disabled(title);
    ui.separator();
    let mut picked: Option<Option<String>> = None;
    if ui.selectable_config("Default").selected(selected.is_none()).build() {
        picked = Some(None);
    }
    if devices.is_empty() {
        ui.text_disabled("Loading devices...");
    }
    for device in devices {
        if ui.selectable_config(device).selected(selected.as_deref() == Some(device.as_str())).build() {
            picked = Some(Some(device.clone()));
        }
    }
    let Some(device) = picked else { return };
    match kind {
        DeviceKind::Input => {
            state.selected_input_device = device.clone();
            let _ = cmd_tx.send(UiCommand::SetVoiceInputDevice { device });
        }
        DeviceKind::Output => {
            state.selected_output_device = device.clone();
            let _ = cmd_tx.send(UiCommand::SetVoiceOutputDevice { device });
        }
    }
    persist_settings(state);
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
            #[cfg(feature = "screenshare")]
            if !state.screen_share_stats.endpoint.is_empty() {
                let video = &state.screen_share_stats;
                let resolution = if video.video_width > 0 && video.video_height > 0 { format!("{}×{}", video.video_width, video.video_height) } else { "waiting for frame".into() };
                ui.separator();
                ui.text("Screen share");
                row("Video", format!("{} | {:.1} fps | {}", if video.video_codec.is_empty() { "-" } else { &video.video_codec }, video.video_fps, resolution));
                let send_resolution = if video.video_up_width > 0 && video.video_up_height > 0 { format!("{}×{}", video.video_up_width, video.video_up_height) } else { "-".into() };
                row("Video sent", format!("{} | {:.1} fps | {}", format_bitrate(video.video_bitrate_up_bps), video.video_fps_up, send_resolution));
                row("Video received", format!("{} | {:.1} fps", format_bitrate(video.video_bitrate_down_bps), video.video_fps));
                row("Stream audio", format!("up {} | {}", format_bitrate(video.audio_stream_bitrate_up_bps), if video.audio_stream_active { "active" } else { "no packets" }));
                row("Video errors", format!("decrypt {} | decoder drops {}", video.video_decrypt_failures, video.video_decoder_drops));
            }
            ui.separator();
            let size = [340.0, 44.0];
            let graph = |id: &str, values: Vec<f32>, overlay: String| {
                if values.len() > 1 {
                    ui.plot_lines(id, &values).graph_size(size).overlay_text(overlay).scale_min(0.0).build();
                }
            };
            graph("##g_up", series(&|x| x.bitrate_up_bps as f32 / 1000.0), format!("upload {:.0} kbps", s.bitrate_up_bps as f32 / 1000.0));
            graph("##g_down", series(&|x| x.bitrate_down_bps as f32 / 1000.0), format!("download {:.0} kbps", s.bitrate_down_bps as f32 / 1000.0));
            #[cfg(feature = "screenshare")]
            if !state.screen_share_stats.endpoint.is_empty() {
                let video_series: Vec<f32> = state.screen_share_history.iter().map(|x| x.video_bitrate_down_bps as f32 / 1000.0).collect();
                graph("##g_video_down", video_series, format!("video received {:.0} kbps", state.screen_share_stats.video_bitrate_down_bps as f32 / 1000.0));
                let video_up_series: Vec<f32> = state.screen_share_history.iter().map(|x| x.video_bitrate_up_bps as f32 / 1000.0).collect();
                graph("##g_video_up", video_up_series, format!("video sent {:.0} kbps", state.screen_share_stats.video_bitrate_up_bps as f32 / 1000.0));
            }
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
fn fit_size(natural: [f32; 2], max: [f32; 2]) -> [f32; 2] {
    let scale = (max[0] / natural[0].max(1.0)).min(max[1] / natural[1].max(1.0)).min(1.0);
    [natural[0] * scale, natural[1] * scale]
}

fn draw_gifv_embed(ui: &Ui, embed: &rustcord_client::models::Embed, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) -> bool {
    let max = [MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT];
    let poster = embed.thumbnail.as_ref().map(|t| t.best_url()).filter(|u| !u.is_empty());
    let video_url = embed.video.as_ref().map(|v| v.url.as_str()).filter(|u| !u.is_empty());
    if poster.is_none() && video_url.is_none() {
        return false;
    }
    ui.dummy([0.0, 4.0]);
    #[cfg(feature = "video")]
    if let Some(video_url) = video_url {
        if textures.is_video(video_url) {
            textures.loop_video_if_ended(video_url, cmd_tx);
            if let Some((texture_id, natural)) = textures.get(video_url) {
                imgui::Image::new(texture_id, fit_size(natural, max)).build(ui);
                return true;
            }
        } else {
            let placeholder = poster.and_then(|p| textures.get(p)).map_or(max, |(_, natural)| fit_size(natural, max));
            textures.start_looping_video(video_url, placeholder, cmd_tx);
        }
    }
    match poster.map(|p| (p, textures.get(p))) {
        Some((_, Some((texture_id, natural)))) => {
            imgui::Image::new(texture_id, fit_size(natural, max)).build(ui);
            if ui.is_item_clicked() {
                if let Some(url) = embed.url.as_deref() {
                    let _ = open::that(url);
                }
            }
        }
        Some((poster, None)) => {
            textures.request(poster, cmd_tx);
            ui.dummy([max[0] / 2.0, max[1] / 2.0]);
        }
        None => ui.dummy([max[0] / 2.0, max[1] / 2.0]),
    }
    true
}

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
    mentions: &mut MentionCtx,
) {
    if embed.kind.as_deref() == Some("gifv") && draw_gifv_embed(ui, embed, textures, cmd_tx) {
        return;
    }
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
            draw_markdown(ui, fonts, theme, description, revealed_spoilers, &format!("embed_{id_seed}"), textures, cmd_tx, mentions);
        }
        for (field_idx, field) in embed.fields.iter().enumerate() {
            let font = ui.push_font(fonts.bold);
            ui.text_wrapped(&field.name);
            font.pop();
            draw_markdown(
                ui,
                fonts,
                theme,
                &field.value,
                revealed_spoilers,
                &format!("embed_{id_seed}_field_{field_idx}"),
                textures,
                cmd_tx,
                mentions,
            );
        }
        let mut draw_media = |media: &rustcord_client::models::EmbedMedia, max: [f32; 2], open_on_click: bool| {
            let url = media.best_url();
            if url.is_empty() {
                return;
            }
            #[cfg(feature = "video")]
            if open_on_click {
                if let Some(video_url) = embed.video.as_ref().map(|v| v.url.as_str()).filter(|u| !u.is_empty()) {
                    if textures.is_video(video_url) {
                        if let Some(status) = textures.video_status(video_url) {
                            if let Some((texture_id, natural)) = textures.get(video_url) {
                                let size = scaled_image_size(natural);
                                imgui::Image::new(texture_id, size).build(ui);
                                if ui.is_item_clicked() {
                                    textures.toggle_video_playback(video_url);
                                }
                                draw_video_controls(ui, cmd_tx, textures, video_url, id_seed, &status, size[0]);
                            }
                        }
                        return;
                    }
                    if let Some((texture_id, natural)) = textures.get(url) {
                        let scale = (max[0] / natural[0].max(1.0)).min(max[1] / natural[1].max(1.0)).min(1.0);
                        let size = [natural[0] * scale, natural[1] * scale];
                        imgui::Image::new(texture_id, size).build(ui);
                        if ui.is_item_clicked() {
                            textures.start_video(video_url, size, cmd_tx);
                        }
                    } else {
                        textures.request(url, cmd_tx);
                        ui.text_disabled("[loading video preview...]");
                    }
                    return;
                }
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

fn mute_menu(ui: &Ui, noun: &str, muted: bool) -> Option<Option<i64>> {
    if muted {
        return ui.selectable(format!("Unmute {noun}")).then_some(None);
    }
    let mut picked = None;
    ui.menu(format!("Mute {noun}"), || {
        for (label, secs) in crate::app_state::MUTE_CHOICES {
            if ui.menu_item(label) {
                picked = Some(Some(*secs));
            }
        }
    });
    picked
}

fn notification_level_menu(ui: &Ui, current: u8, allow_inherit: bool) -> Option<u8> {
    use rustcord_client::models::{NOTIFY_ALL, NOTIFY_INHERIT, NOTIFY_MENTIONS, NOTIFY_NOTHING};
    let mut picked = None;
    ui.menu("Notifications", || {
        let inherit = allow_inherit.then_some(("Use category default", NOTIFY_INHERIT));
        let levels = [("All messages", NOTIFY_ALL), ("Only @mentions", NOTIFY_MENTIONS), ("Nothing", NOTIFY_NOTHING)];
        for (label, level) in inherit.into_iter().chain(levels) {
            if ui.menu_item_config(label).selected(current == level).build() && current != level {
                picked = Some(level);
            }
        }
    });
    picked
}

fn draw_channel_notification_menu(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let (Some(guild_id), Some(channel_id)) = (state.selected_guild.clone(), state.channel_context.clone()) else { return };
    let now = chrono::Utc::now();
    if state.is_channel_muted(Some(&guild_id), &channel_id, now) && !state.channel_mute_is_direct(Some(&guild_id), &channel_id, now) {
        ui.text_disabled("Muted by its category");
    } else if let Some(mute) = mute_menu(ui, "channel", state.channel_mute_is_direct(Some(&guild_id), &channel_id, now)) {
        let fields = crate::app_state::mute_patch(mute.is_some(), mute.unwrap_or(-1), now);
        let patch = rustcord_client::models::channel_override_patch(&channel_id, fields);
        let _ = cmd_tx.send(UiCommand::UpdateGuildSettings { guild_key: guild_id.clone(), patch });
    }
    if let Some(level) = notification_level_menu(ui, state.channel_notification_override(&guild_id, &channel_id), true) {
        let patch = rustcord_client::models::channel_override_patch(&channel_id, serde_json::json!({ "message_notifications": level }));
        let _ = cmd_tx.send(UiCommand::UpdateGuildSettings { guild_key: guild_id, patch });
    }
    ui.separator();
}

fn draw_channel_admin_menu(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    use rustcord_client::permissions::{CREATE_INSTANT_INVITE, MANAGE_CHANNELS, MANAGE_GUILD};
    let Some(guild_id) = state.selected_guild.clone() else { return };
    let channel = state
        .channel_context
        .as_deref()
        .and_then(|id| state.guild_channels.get(&guild_id)?.iter().find(|c| c.id == id))
        .cloned();
    let mut any = false;
    if let Some(channel) = &channel {
        if state.has_channel_permission(&guild_id, channel, MANAGE_CHANNELS) && ui.selectable("Edit channel") {
            state.channel_editor = Some(crate::app_state::ChannelEditor::edit(guild_id.clone(), channel));
        }
        if state.has_channel_permission(&guild_id, channel, CREATE_INSTANT_INVITE) && ui.selectable("Create invite link") {
            let _ = cmd_tx.send(UiCommand::CreateInvite { channel_id: channel.id.clone() });
        }
        any = true;
    }
    if state.has_guild_permission(&guild_id, MANAGE_CHANNELS) {
        if ui.selectable("Create channel") {
            let parent_id = channel.as_ref().and_then(|c| c.parent_id.clone());
            state.channel_editor = Some(crate::app_state::ChannelEditor::create(guild_id.clone(), parent_id));
        }
        any = true;
    }
    if state.has_guild_permission(&guild_id, MANAGE_GUILD) {
        if ui.selectable("Manage invites") {
            state.invite_manager = Some(crate::app_state::InviteManager { guild_id: guild_id.clone(), invites: None });
            let _ = cmd_tx.send(UiCommand::LoadGuildInvites { guild_id });
        }
        any = true;
    }
    if any {
        ui.separator();
    }
}

fn draw_channel_editor(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, display_w: f32, display_h: f32) {
    let Some(editor) = state.channel_editor.as_mut() else { return };
    let creating = editor.channel_id.is_none();
    let title = if creating { "Create channel###channel_editor" } else { "Edit channel###channel_editor" };
    let mut open = true;
    let mut done = false;
    ui.window(title)
        .opened(&mut open)
        .position([display_w / 2.0, display_h / 2.0], imgui::Condition::Appearing)
        .position_pivot([0.5, 0.5])
        .always_auto_resize(true)
        .collapsible(false)
        .build(|| {
            if creating {
                ui.radio_button("Text", &mut editor.voice, false);
                ui.same_line();
                ui.radio_button("Voice", &mut editor.voice, true);
            }
            if std::mem::take(&mut editor.focus) {
                ui.set_keyboard_focus_here();
            }
            ui.set_next_item_width(320.0);
            let submitted = ui.input_text("Name", &mut editor.name).enter_returns_true(true).build();
            if !editor.voice {
                ui.set_next_item_width(320.0);
                ui.input_text("Topic", &mut editor.topic).build();
            }
            let valid = !editor.name.trim().is_empty();
            ui.disabled(!valid, || {
                if ui.button(if creating { "Create" } else { "Save" }) || (submitted && valid) {
                    let topic = (!editor.voice).then(|| editor.topic.trim().to_string());
                    let _ = cmd_tx.send(UiCommand::SaveChannel {
                        guild_id: editor.guild_id.clone(),
                        channel_id: editor.channel_id.clone(),
                        name: editor.name.trim().to_string(),
                        topic,
                        voice: editor.voice,
                        parent_id: editor.parent_id.clone(),
                    });
                    done = true;
                }
            });
            ui.same_line();
            if ui.button("Cancel") || ui.is_key_pressed(imgui::Key::Escape) {
                done = true;
            }
        });
    if done || !open {
        state.channel_editor = None;
    }
}

fn draw_invite_manager(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, display_w: f32, display_h: f32) {
    let Some(manager) = state.invite_manager.as_ref() else { return };
    let guild_name = state.guilds.iter().find(|g| g.id == manager.guild_id).map_or("Server", |g| g.name.as_str());
    let channel_names = state.channel_name_map();
    let mut open = true;
    ui.window(format!("Invites for {guild_name}###invite_manager"))
        .opened(&mut open)
        .position([display_w / 2.0, display_h / 2.0], imgui::Condition::Appearing)
        .position_pivot([0.5, 0.5])
        .size([560.0, 360.0], imgui::Condition::Appearing)
        .collapsible(false)
        .build(|| {
            let invites = match &manager.invites {
                None => return ui.text_disabled("Loading..."),
                Some(Err(e)) => return ui.text_colored(state.theme.text_muted, format!("Couldn't load invites: {e}")),
                Some(Ok(invites)) if invites.is_empty() => return ui.text_disabled("This server has no active invites."),
                Some(Ok(invites)) => invites,
            };
            let flags = imgui::TableFlags::ROW_BG | imgui::TableFlags::BORDERS_INNER_H | imgui::TableFlags::SIZING_STRETCH_PROP;
            let Some(_table) = ui.begin_table_with_flags("invite_table", 6, flags) else { return };
            for header in ["Code", "Channel", "Inviter", "Uses", "Expires", ""] {
                ui.table_setup_column(header);
            }
            ui.table_headers_row();
            for invite in invites {
                ui.table_next_row();
                ui.table_next_column();
                ui.text(&invite.code);
                ui.table_next_column();
                let channel = invite.channel.as_ref().and_then(|c| c.name.clone().or_else(|| channel_names.get(&c.id).cloned()));
                ui.text(format!("#{}", channel.unwrap_or_default()));
                ui.table_next_column();
                ui.text(invite.inviter.as_ref().map_or("", |u| u.display_name()));
                ui.table_next_column();
                let uses = invite.uses.unwrap_or(0);
                ui.text(match invite.max_uses {
                    Some(max) if max > 0 => format!("{uses}/{max}"),
                    _ => uses.to_string(),
                });
                ui.table_next_column();
                let expires = invite
                    .expires_at
                    .as_deref()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map_or_else(|| "Never".to_string(), |t| t.with_timezone(&chrono::Local).format("%b %-d, %H:%M").to_string());
                ui.text(expires);
                ui.table_next_column();
                if ui.small_button(format!("Copy##copy_{}", invite.code)) {
                    crate::clipboard::write_text(&format!("https://discord.gg/{}", invite.code));
                }
                ui.same_line();
                if ui.small_button(format!("Revoke##revoke_{}", invite.code)) {
                    let _ = cmd_tx.send(UiCommand::RevokeInvite { code: invite.code.clone() });
                }
            }
        });
    if !open {
        state.invite_manager = None;
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

#[cfg(feature = "video")]
fn draw_video_fullscreen(ui: &Ui, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some(url) = textures.fullscreen_video() else { return };
    let Some(status) = textures.video_status(&url) else { return };

    let [display_w, display_h] = ui.io().display_size;
    const CONTROL_HEIGHT: f32 = 52.0;
    let max_h = display_h - CONTROL_HEIGHT;
    let size = match textures.get(&url) {
        Some((_, [nw, nh])) if nw > 0.0 && nh > 0.0 => {
            let scale = (display_w / nw).min(max_h / nh);
            [nw * scale, nh * scale]
        }
        _ => [display_w, max_h],
    };

    ui.window("##video_fs_backdrop")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([display_w, display_h], imgui::Condition::Always)
        .no_decoration()
        .no_inputs()
        .draw_background(true)
        .bg_alpha(0.95)
        .build(|| {});

    let toggled = ui
        .window("##video_fs")
        .position([display_w / 2.0 - size[0] / 2.0, (max_h - size[1]) / 2.0], imgui::Condition::Always)
        .size(size, imgui::Condition::Always)
        .no_decoration()
        .draw_background(false)
        .build(|| match textures.get(&url) {
            Some((texture_id, _)) => {
                imgui::Image::new(texture_id, size).build(ui);
                ui.is_item_clicked()
            }
            None => {
                ui.text_disabled("Buffering...");
                false
            }
        })
        .unwrap_or(false);

    ui.window("##video_fs_controls")
        .position([0.0, display_h - CONTROL_HEIGHT], imgui::Condition::Always)
        .size([display_w, CONTROL_HEIGHT], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .bg_alpha(0.6)
        .build(|| {
            draw_video_controls(ui, cmd_tx, textures, &url, "fs", &status, display_w);
        });

    if toggled {
        textures.toggle_video_playback(&url);
    }
    if ui.is_key_pressed(imgui::Key::Escape) {
        textures.exit_video_fullscreen(&url);
    }
}

const SWITCHER_WIDTH: f32 = 560.0;
const SWITCHER_ROW_HEIGHT: f32 = 34.0;
const SWITCHER_ICON_SIZE: f32 = 22.0;
const SWITCHER_MAX_RESULTS: usize = 8;
const SWITCHER_HEADER_HEIGHT: f32 = 46.0;
const SWITCHER_PADDING: f32 = 8.0;
const SWITCHER_ROUNDING: f32 = 10.0;
const COMPOSER_ROUNDING: f32 = 8.0;

fn draw_quick_switcher(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache, display_w: f32, display_h: f32) {
    if !state.switcher_open {
        return;
    }
    let entries = state.switcher_entries();
    let results = crate::switcher::rank(&entries, state.switcher_query.trim(), SWITCHER_MAX_RESULTS);

    if ui.is_key_pressed(imgui::Key::DownArrow) && !results.is_empty() {
        state.switcher_selected = (state.switcher_selected + 1) % results.len();
    }
    if ui.is_key_pressed(imgui::Key::UpArrow) && !results.is_empty() {
        state.switcher_selected = (state.switcher_selected + results.len() - 1) % results.len();
    }
    state.switcher_selected = state.switcher_selected.min(results.len().saturating_sub(1));

    let rows = results.len().max(1) as f32;
    let height = SWITCHER_HEADER_HEIGHT + rows * SWITCHER_ROW_HEIGHT + SWITCHER_PADDING * 2.0;
    let x = ((display_w - SWITCHER_WIDTH) / 2.0).max(0.0);
    let y = (display_h * 0.22).min(150.0);

    let mut chosen: Option<crate::switcher::Target> = None;
    let mut close = ui.is_key_pressed(imgui::Key::Escape);

    ui.window("##quick_switcher_backdrop")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([display_w, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .bg_alpha(0.78)
        .focus_on_appearing(false)
        .build(|| {
            if ui.is_window_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Left) {
                close = true;
            }
        });

    let _rounding = ui.push_style_var(imgui::StyleVar::WindowRounding(SWITCHER_ROUNDING));
    let _padding = ui.push_style_var(imgui::StyleVar::WindowPadding([SWITCHER_PADDING, SWITCHER_PADDING]));
    let _spacing = ui.push_style_var(imgui::StyleVar::ItemSpacing([0.0, 2.0]));
    let _frame_rounding = ui.push_style_var(imgui::StyleVar::FrameRounding(6.0));
    ui.window("##quick_switcher")
        .position([x, y], imgui::Condition::Always)
        .size([SWITCHER_WIDTH, height], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .build(|| {
            if state.switcher_focus_input {
                ui.set_keyboard_focus_here();
                state.switcher_focus_input = false;
            }
            let inner_width = ui.content_region_avail()[0];
            ui.set_next_item_width(inner_width);
            let submitted = ui
                .input_text("##switcher_query", &mut state.switcher_query)
                .hint("Jump to a server, channel or DM...")
                .enter_returns_true(true)
                .build();
            ui.dummy([0.0, 4.0]);

            if results.is_empty() {
                ui.text_disabled("No matches");
            }
            let _row_rounding = ui.push_style_var(imgui::StyleVar::FrameRounding(6.0));
            for (idx, result) in results.iter().enumerate() {
                let selected = idx == state.switcher_selected;
                let row_min = ui.cursor_screen_pos();
                if selected {
                    let accent = state.theme.accent;
                    ui.get_window_draw_list()
                        .add_rect(
                            row_min,
                            [row_min[0] + inner_width, row_min[1] + SWITCHER_ROW_HEIGHT],
                            [accent[0], accent[1], accent[2], 0.45],
                        )
                        .filled(true)
                        .rounding(6.0)
                        .build();
                }
                let clicked = ui.invisible_button(format!("##switcher_row_{idx}"), [inner_width, SWITCHER_ROW_HEIGHT]);
                if ui.is_item_hovered() {
                    state.switcher_selected = idx;
                }

                let icon_pos = [row_min[0] + 8.0, row_min[1] + (SWITCHER_ROW_HEIGHT - SWITCHER_ICON_SIZE) / 2.0];
                draw_switcher_icon(ui, state, textures, cmd_tx, &result.entry.icon, icon_pos);
                let label_x = row_min[0] + 8.0 + SWITCHER_ICON_SIZE + 10.0;
                ui.set_cursor_screen_pos([label_x, row_min[1] + (SWITCHER_ROW_HEIGHT - ui.text_line_height()) / 2.0]);
                draw_highlighted_label(ui, &state.theme, &result.entry.label, &result.hits);
                if let Some(context) = &result.entry.context {
                    ui.same_line_with_spacing(0.0, 8.0);
                    ui.text_colored(state.theme.text_muted, context);
                }
                ui.set_cursor_screen_pos([row_min[0], row_min[1] + SWITCHER_ROW_HEIGHT]);

                if clicked {
                    chosen = Some(result.entry.target.clone());
                }
            }

            if submitted {
                chosen = results.get(state.switcher_selected).map(|r| r.entry.target.clone());
                if chosen.is_none() {
                    close = true;
                }
            }
        });
    _frame_rounding.pop();
    _spacing.pop();
    _padding.pop();
    _rounding.pop();

    if let Some(target) = chosen {
        apply_switcher_target(state, cmd_tx, target);
        close = true;
    }
    if close {
        state.switcher_open = false;
        state.switcher_query.clear();
        state.switcher_selected = 0;
    }
}

fn draw_switcher_icon(
    ui: &Ui,
    state: &AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    icon: &crate::switcher::Icon,
    pos: [f32; 2],
) {
    use crate::switcher::Icon;
    const SIZE: f32 = SWITCHER_ICON_SIZE;
    let radius = SIZE / 2.0;
    let centre = [pos[0] + radius, pos[1] + radius];
    let draw_list = ui.get_window_draw_list();

    match icon {
        Icon::Image(url) => {
            if let Some((texture_id, _)) = textures.get(url) {
                draw_list
                    .add_image_rounded(texture_id, pos, [pos[0] + SIZE, pos[1] + SIZE], radius)
                    .build();
            } else {
                textures.request(url, cmd_tx);
                draw_list.add_circle(centre, radius, state.theme.border).filled(true).build();
            }
        }
        Icon::Initials(text) => {
            draw_list.add_circle(centre, radius, state.theme.border).filled(true).build();
            let size = ui.calc_text_size(text);
            let scale = 0.8;
            let origin = [centre[0] - size[0] * scale / 2.0, centre[1] - size[1] * scale / 2.0];
            ui.set_cursor_screen_pos(origin);
            ui.set_window_font_scale(scale);
            ui.text_colored(state.theme.text, text);
            ui.set_window_font_scale(1.0);
        }
        Icon::Channel => {
            let size = ui.calc_text_size("#");
            let origin = [centre[0] - size[0] / 2.0, centre[1] - size[1] / 2.0];
            ui.set_cursor_screen_pos(origin);
            ui.text_colored(state.theme.text_muted, "#");
        }
    }
}

fn draw_highlighted_label(ui: &Ui, theme: &crate::theme::Theme, label: &str, hits: &[usize]) {
    let mut run = String::new();
    let mut run_is_hit = false;
    let mut first = true;
    let flush = |run: &mut String, is_hit: bool, first: &mut bool| {
        if run.is_empty() {
            return;
        }
        if !*first {
            ui.same_line_with_spacing(0.0, 0.0);
        }
        *first = false;
        if is_hit {
            ui.text_colored(theme.accent, run.as_str());
        } else {
            ui.text(run.as_str());
        }
        run.clear();
    };
    for (idx, ch) in label.chars().enumerate() {
        let is_hit = hits.contains(&idx);
        if is_hit != run_is_hit {
            flush(&mut run, run_is_hit, &mut first);
            run_is_hit = is_hit;
        }
        run.push(ch);
    }
    flush(&mut run, run_is_hit, &mut first);
}

fn apply_switcher_target(state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, target: crate::switcher::Target) {
    use crate::switcher::Target;
    match target {
        Target::Guild { guild_id } => {
            let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild_id.clone() });
            state.selected_guild = Some(guild_id);
            state.selected_channel = None;
        }
        Target::Channel { guild_id, channel_id } => {
            if state.selected_guild.as_deref() != Some(guild_id.as_str()) {
                let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild_id.clone() });
                state.selected_guild = Some(guild_id);
            }
            let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
            state.selected_channel = Some(channel_id);
            state.server_call_minimized = true;
        }
        Target::Dm { channel_id } => {
            state.selected_guild = None;
            state.pending_sidebar_tab = Some(0);
            let _ = cmd_tx.send(UiCommand::SelectChannel { channel_id: channel_id.clone() });
            state.selected_channel = Some(channel_id);
            state.server_call_minimized = true;
        }
    }
}

fn icon_button(ui: &Ui, textures: &TextureCache, id: &str, icon: crate::icons::Icon, tooltip: &str, active: Option<[f32; 4]>) -> bool {
    icon_button_sized(ui, textures, id, icon, tooltip, active, 30.0)
}

fn icon_button_sized(
    ui: &Ui,
    textures: &TextureCache,
    id: &str,
    icon: crate::icons::Icon,
    tooltip: &str,
    active: Option<[f32; 4]>,
    size: f32,
) -> bool {
    const SIZE_REF: f32 = 30.0;
    let pad = (6.0 * size / SIZE_REF).round();
    let p = ui.cursor_screen_pos();
    let clicked = ui.invisible_button(id, [size, size]);
    let hovered = ui.is_item_hovered();
    let fill = match (active, hovered) {
        (Some(c), _) => Some([c[0], c[1], c[2], 0.18]),
        (None, true) => Some(ui.style_color(imgui::StyleColor::ButtonHovered)),
        (None, false) => None,
    };
    if let Some(c) = fill {
        ui.get_window_draw_list().add_rect(p, [p[0] + size, p[1] + size], c).filled(true).rounding(9.0).build();
    }
    let tint = match (active, hovered) {
        (Some(c), _) => c,
        (None, true) => ui.style_color(imgui::StyleColor::Text),
        (None, false) => ui.style_color(imgui::StyleColor::TextDisabled),
    };
    draw_icon_at(ui, textures, icon, [p[0] + pad, p[1] + pad], size - pad * 2.0, tint);
    if hovered {
        ui.tooltip_text(tooltip);
    }
    clicked
}

#[cfg(feature = "screenshare")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ServerCallIcon { Microphone, Headphones, Camera, Screen, Info, Hangup }

#[cfg(feature = "screenshare")]
fn draw_server_call_button(ui: &Ui, textures: &TextureCache, id: &str, tooltip: &str, icon: ServerCallIcon, active: Option<[f32; 4]>, x: f32, y: f32, size: f32) -> bool {
    ui.set_cursor_pos([x, y]);
    let clicked = ui.invisible_button(id, [size, size]);
    let hovered = ui.is_item_hovered();
    let p = ui.item_rect_min();
    let base = active.unwrap_or([0.17, 0.17, 0.20, 1.0]);
    let fill = if icon == ServerCallIcon::Hangup {
        [0.78, 0.16, 0.19, 1.0]
    } else if hovered && active.is_none() {
        [0.27, 0.27, 0.31, 1.0]
    } else {
        base
    };
    ui.get_window_draw_list().add_rect(p, [p[0] + size, p[1] + size], fill).filled(true).rounding(size * 0.3).build();
    let glyph = match (icon, active.is_some()) {
        (ServerCallIcon::Microphone, false) => crate::icons::Icon::Mic,
        (ServerCallIcon::Microphone, true) => crate::icons::Icon::MicOff,
        (ServerCallIcon::Headphones, false) => crate::icons::Icon::Headphones,
        (ServerCallIcon::Headphones, true) => crate::icons::Icon::HeadphoneOff,
        (ServerCallIcon::Camera, _) => crate::icons::Icon::Video,
        (ServerCallIcon::Screen, _) => crate::icons::Icon::MonitorUp,
        (ServerCallIcon::Info, _) => crate::icons::Icon::Info,
        (ServerCallIcon::Hangup, _) => crate::icons::Icon::PhoneOff,
    };
    let glyph_size = (size * 0.45).round();
    let inset = (size - glyph_size) / 2.0;
    draw_icon_at(ui, textures, glyph, [p[0] + inset, p[1] + inset], glyph_size, [0.96, 0.96, 0.98, 1.0]);
    if hovered { ui.tooltip_text(tooltip); }
    clicked
}

#[cfg(feature = "screenshare")]
fn draw_server_call_controls(ui: &Ui, state: &mut AppState, textures: &TextureCache, cmd_tx: &UnboundedSender<UiCommand>, guild_id: Option<String>, channel_id: &str) {
    const SIZE: f32 = 60.0;
    const GAP: f32 = 14.0;
    const COUNT: f32 = 6.0;
    let width = ui.window_size()[0];
    let x = ((width - (COUNT * SIZE + (COUNT - 1.0) * GAP)) * 0.5).max(0.0);
    let y = (ui.window_size()[1] - 88.0).max(8.0);
    let dock_width = COUNT * SIZE + (COUNT - 1.0) * GAP + 28.0;
    {
        let dl = ui.get_window_draw_list();
        let origin = ui.cursor_screen_pos();
        dl.add_rect([origin[0] + x - 14.0, origin[1] + y - 8.0], [origin[0] + x + dock_width - 14.0, origin[1] + y + SIZE + 8.0], [0.105, 0.105, 0.125, 0.96]).filled(true).rounding(22.0).build();
    }
    let mut index = 0.0f32;
    let mut next_x = || {
        let pos = x + index * (SIZE + GAP);
        index += 1.0;
        pos
    };
    let muted = state.self_muted;
    if draw_server_call_button(ui, textures, "server_call_mute", if muted { "Unmute microphone" } else { "Mute microphone" }, ServerCallIcon::Microphone, muted.then_some([0.75, 0.17, 0.20, 1.0]), next_x(), y, SIZE) {
        toggle_voice(state, cmd_tx, VoiceToggle::Mute);
    }
    let deafened = state.self_deafened;
    if draw_server_call_button(ui, textures, "server_call_deafen", if deafened { "Undeafen" } else { "Deafen" }, ServerCallIcon::Headphones, deafened.then_some([0.75, 0.17, 0.20, 1.0]), next_x(), y, SIZE) {
        toggle_voice(state, cmd_tx, VoiceToggle::Deafen);
    }
    let camera = state.camera_on;
    if draw_server_call_button(ui, textures, "server_call_camera", camera_tooltip(camera), ServerCallIcon::Camera, camera.then_some([0.18, 0.58, 0.34, 1.0]), next_x(), y, SIZE) {
        toggle_camera(state, cmd_tx);
    }
    let sharing = state.screen_sharing;
    if draw_server_call_button(ui, textures, "server_call_share", if sharing { "Stop screen sharing" } else { "Share your screen" }, ServerCallIcon::Screen, sharing.then_some([0.18, 0.58, 0.34, 1.0]), next_x(), y, SIZE) {
        if sharing {
            state.screen_sharing = false;
            let _ = cmd_tx.send(UiCommand::SetScreenSharing { enabled: false, include_system_audio: state.share_system_audio, audio_source_pid: state.selected_screen_audio_pid, max_height: state.screen_share_max_height, framerate: state.screen_share_fps, guild_id: guild_id.clone(), channel_id: channel_id.to_string() });
        } else {
            state.show_screen_share_setup = true;
        }
    }
    if draw_server_call_button(ui, textures, "server_call_info", "Connection info", ServerCallIcon::Info, state.show_voice_info.then_some([0.25, 0.35, 0.72, 1.0]), next_x(), y, SIZE) {
        state.show_voice_info = !state.show_voice_info;
    }
    if draw_server_call_button(ui, textures, "server_call_hangup", "Leave voice channel", ServerCallIcon::Hangup, None, next_x(), y, SIZE) {
        let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id });
    }
}

fn cover_uv(source: [f32; 2], target: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let source_aspect = source[0].max(1.0) / source[1].max(1.0);
    let target_aspect = target[0].max(1.0) / target[1].max(1.0);
    if source_aspect > target_aspect {
        let visible = target_aspect / source_aspect;
        let inset = (1.0 - visible) / 2.0;
        ([inset, 0.0], [1.0 - inset, 1.0])
    } else {
        let visible = source_aspect / target_aspect;
        let inset = (1.0 - visible) / 2.0;
        ([0.0, inset], [1.0, 1.0 - inset])
    }
}

#[cfg(feature = "screenshare")]
fn draw_call_participant_tiles(
    ui: &Ui,
    state: &mut AppState,
    textures: &mut TextureCache,
    cmd_tx: &UnboundedSender<UiCommand>,
    participants: &[(rustcord_client::models::User, bool)],
    origin: [f32; 2],
    width: f32,
    height: f32,
    large: bool,
) {
    if participants.is_empty() { return; }
    let gap = if large { 18.0 } else { 12.0 };
    let columns = if large {
        participants.len().min(3).max(1)
    } else {
        (((width + gap) / (220.0 + gap)).floor() as usize).clamp(1, 6).min(participants.len())
    };
    let rows = (participants.len() + columns - 1) / columns;
    let area_top = if large { 72.0 } else { height - 224.0 };
    let area_bottom = if large { height - 122.0 } else { height - 106.0 };
    let area_h = (area_bottom - area_top).max(90.0);
    let tile_w = if large {
        ((width - 48.0 - gap * (columns.saturating_sub(1) as f32)) / columns as f32).min(420.0).max(180.0)
    } else { 220.0 };
    let tile_h = if large {
        ((area_h - gap * (rows.saturating_sub(1) as f32)) / rows as f32).min(340.0).max(180.0)
    } else { 118.0 };
    let visible = participants.len().min(columns * if large { 3 } else { 1 });
    let grid_w = columns as f32 * tile_w + columns.saturating_sub(1) as f32 * gap;
    let grid_h = if large { rows.min(3) as f32 * tile_h + rows.min(3).saturating_sub(1) as f32 * gap } else { tile_h };
    let start_x = (width - grid_w) * 0.5;
    let start_y = if large { area_top + (area_h - grid_h) * 0.5 } else { area_top };
    for (index, (user, speaking)) in participants.iter().take(visible).enumerate() {
        let col = index % columns;
        let row = index / columns;
        let p_min = [origin[0] + start_x + col as f32 * (tile_w + gap), origin[1] + start_y + row as f32 * (tile_h + gap)];
        let p_max = [p_min[0] + tile_w, p_min[1] + tile_h];
        let avatar_size = if large { 124.0 } else { 58.0 };
        let avatar_min = [p_min[0] + (tile_w - avatar_size) * 0.5, p_min[1] + (tile_h - avatar_size) * 0.5 - if large { 14.0 } else { 8.0 }];
        let avatar_max = [avatar_min[0] + avatar_size, avatar_min[1] + avatar_size];
        let avatar_url = rustcord_client::cdn::avatar_url(user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
        let avatar = textures.get_still(&avatar_url).map(|(id, _)| id);
        if avatar.is_none() { textures.request(&avatar_url, cmd_tx); }
        let camera = textures.live_video(&crate::app_state::camera_video_key(&user.id))
            .or_else(|| state.current_user.as_ref().filter(|me| me.id == user.id)
                .and_then(|_| textures.live_video(&crate::app_state::screen_preview_video_key(&user.id))));
        let name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
        let display_name = truncate_to_width(ui, &name, tile_w - if large { 32.0 } else { 82.0 }).unwrap_or(name);
        let speaking_color = [0.26, 0.83, 0.44, 1.0];
        let border = if *speaking { speaking_color } else { [0.23, 0.23, 0.27, 1.0] };
        let fill = if *speaking { [0.10, 0.16, 0.13, 1.0] } else { [0.09, 0.09, 0.105, 1.0] };
        {
            let dl = ui.get_window_draw_list();
            dl.add_rect(p_min, p_max, border).filled(true).rounding(if large { 16.0 } else { 12.0 }).build();
            dl.add_rect([p_min[0] + 2.0, p_min[1] + 2.0], [p_max[0] - 2.0, p_max[1] - 2.0], fill).filled(true).rounding(if large { 14.0 } else { 10.0 }).build();
            if let Some((texture_id, source)) = camera {
                let (inner_min, inner_max) = ([p_min[0] + 2.0, p_min[1] + 2.0], [p_max[0] - 2.0, p_max[1] - 2.0]);
                let (uv_min, uv_max) = cover_uv(source, [inner_max[0] - inner_min[0], inner_max[1] - inner_min[1]]);
                dl.add_image_rounded(texture_id, inner_min, inner_max, if large { 14.0 } else { 10.0 }).uv_min(uv_min).uv_max(uv_max).build();
            } else if let Some(texture_id) = avatar {
                dl.add_image_rounded(texture_id, avatar_min, avatar_max, avatar_size * 0.5).build();
            } else {
                dl.add_circle([avatar_min[0] + avatar_size * 0.5, avatar_min[1] + avatar_size * 0.5], avatar_size * 0.5, [0.35, 0.35, 0.40, 1.0]).filled(true).build();
            }
            let text_y = p_max[1] - if large { 34.0 } else { 25.0 };
            dl.add_rect([p_min[0] + 8.0, text_y - 4.0], [p_max[0] - 8.0, p_max[1] - 8.0], [0.035, 0.035, 0.045, 0.9]).filled(true).rounding(8.0).build();
            dl.add_text([p_min[0] + 16.0, text_y], ui.style_color(imgui::StyleColor::Text), display_name);
            if *speaking {
                dl.add_circle([p_max[0] - 18.0, text_y + 6.0], 4.0, speaking_color).filled(true).build();
            }
            if large && state.active_streams.contains_key(&user.id) {
                dl.add_rect([p_min[0] + 14.0, p_min[1] + 14.0], [p_min[0] + 60.0, p_min[1] + 38.0], [0.82, 0.12, 0.18, 1.0]).filled(true).rounding(8.0).build();
                dl.add_text([p_min[0] + 22.0, p_min[1] + 18.0], [1.0, 1.0, 1.0, 1.0], "LIVE");
            }
        }
        if large {
            if let Some(stream_key) = state.active_streams.get(&user.id).cloned() {
                let button_width = 152.0;
                let button_height = 38.0;
                ui.set_cursor_pos([p_min[0] - origin[0] + (tile_w - button_width) * 0.5, p_max[1] - origin[1] - 82.0]);
                let clicked = ui.invisible_button(format!("watch_stream_tile_{}", user.id), [button_width, button_height]);
                let hovered = ui.is_item_hovered();
                let button_min = ui.item_rect_min();
                {
                    let dl = ui.get_window_draw_list();
                    let color = if hovered { [0.42, 0.27, 0.83, 1.0] } else { [0.36, 0.22, 0.73, 1.0] };
                    dl.add_rect(button_min, [button_min[0] + button_width, button_min[1] + button_height], color).filled(true).rounding(9.0).build();
                    let label = "Watch Stream";
                    let label_size = ui.calc_text_size(label);
                    dl.add_text([button_min[0] + (button_width - label_size[0]) * 0.5, button_min[1] + (button_height - label_size[1]) * 0.5], [1.0, 1.0, 1.0, 1.0], label);
                }
                if clicked {
                    let _ = cmd_tx.send(UiCommand::WatchStream { stream_key: stream_key.clone() });
                    state.watching_stream = Some(stream_key);
                    state.server_call_minimized = false;
                    state.server_call_focus_stream = false;
                }
            }
        }
    }
    if participants.len() > visible {
        let text = format!("+{} more", participants.len() - visible);
        ui.set_cursor_pos([start_x + grid_w + 8.0, start_y + tile_h * 0.5]);
        ui.text_disabled(text);
    }
}

fn dispatch_keybinds(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    if ui.io().want_text_input {
        return;
    }
    use crate::keybinds::Action;

    let pressed = |action: Action| state.keybinds.get(action).is_some_and(|key| ui.is_key_pressed(key));
    let (focus, mute, deafen, leave) =
        (pressed(Action::FocusCompose), pressed(Action::ToggleMute), pressed(Action::ToggleDeafen), pressed(Action::LeaveVoiceChannel));

    if focus {
        state.focus_compose = true;
    }

    if let Some((guild_id, _)) = state.connected_voice_channel.clone() {
        if mute {
            toggle_voice(state, cmd_tx, VoiceToggle::Mute);
        }
        if deafen {
            toggle_voice(state, cmd_tx, VoiceToggle::Deafen);
        }
        if leave {
            let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id });
        }
    }
}

#[cfg(feature = "screenshare")]
fn draw_screen_share_setup_dialog(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    ui.window("##screen_share_setup_host")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([1.0, 1.0], imgui::Condition::Always)
        .no_decoration()
        .draw_background(false)
        .build(|| {
            let _popup_bg = ui.push_style_color(imgui::StyleColor::PopupBg, state.theme.panel);
            let _popup_title = ui.push_style_color(imgui::StyleColor::TitleBg, state.theme.panel);
            let _popup_title_active = ui.push_style_color(imgui::StyleColor::TitleBgActive, state.theme.panel);
            let _popup_text = ui.push_style_color(imgui::StyleColor::Text, state.theme.text);
            let _popup_muted = ui.push_style_color(imgui::StyleColor::TextDisabled, state.theme.text_muted);
            let _popup_frame = ui.push_style_color(imgui::StyleColor::FrameBg, state.theme.background);
            let _popup_button = ui.push_style_color(imgui::StyleColor::Button, state.theme.accent);
            let _popup_button_hovered = ui.push_style_color(imgui::StyleColor::ButtonHovered, state.theme.accent_hover);
            let _popup_button_active = ui.push_style_color(imgui::StyleColor::ButtonActive, state.theme.accent_hover);
            if state.show_screen_share_setup {
                ui.open_popup("Start screen share");
                state.show_screen_share_setup = false;
                if !state.screen_audio_sources_loaded {
                    let _ = cmd_tx.send(UiCommand::LoadScreenAudioSources);
                }
            }
            if let Some(_popup) = ui.modal_popup_config("Start screen share").always_auto_resize(true).begin_popup() {
                ui.text("Choose stream quality and audio before sharing.");
                #[cfg(target_os = "macos")]
                {
                    let selected_app = state.selected_screen_app_pid
                        .and_then(|pid| state.screen_audio_sources.iter().find(|(source_pid, _, _)| *source_pid == pid))
                        .map(|(_, name, _)| name.as_str())
                        .unwrap_or("Entire display");
                    ui.set_next_item_width(260.0);
                    if let Some(_combo) = ui.begin_combo("Share", selected_app) {
                        if ui.selectable_config("Entire display").selected(state.selected_screen_app_pid.is_none()).build() {
                            state.selected_screen_app_pid = None;
                        }
                        for (pid, name, _) in &state.screen_audio_sources {
                            let label = format!("{name}##app{pid}");
                            if ui.selectable_config(&label).selected(state.selected_screen_app_pid == Some(*pid)).build() {
                                state.selected_screen_app_pid = Some(*pid);
                            }
                        }
                    }
                }
                ui.checkbox("Share audio", &mut state.share_system_audio);
                if state.share_system_audio {
                    let selected_audio = state.selected_screen_audio_pid
                        .and_then(|pid| state.screen_audio_sources.iter().find(|(source_pid, _, _)| *source_pid == pid))
                        .map(|(_, name, _)| name.as_str())
                        .unwrap_or("System audio (exclude RustCord)");
                    ui.set_next_item_width(260.0);
                    if let Some(_combo) = ui.begin_combo("Audio source", selected_audio) {
                        if ui.selectable_config("System audio (exclude RustCord)")
                            .selected(state.selected_screen_audio_pid.is_none()).build()
                        {
                            state.selected_screen_audio_pid = None;
                        }
                        for (pid, name, active) in &state.screen_audio_sources {
                            let label = if *active { format!("{name} (playing)##{pid}") } else { format!("{name}##{pid}") };
                            if ui.selectable_config(&label).selected(state.selected_screen_audio_pid == Some(*pid)).build() {
                                state.selected_screen_audio_pid = Some(*pid);
                            }
                        }
                    }
                    if let Some(error) = &state.screen_audio_sources_error {
                        ui.text_colored([1.0, 0.55, 0.35, 1.0], format!("App audio sources unavailable: {error}"));
                    } else if state.screen_audio_sources_loaded && state.screen_audio_sources.is_empty() {
                        ui.text_disabled("No other apps with audio output found.");
                    }
                    if ui.small_button("Refresh audio sources") {
                        state.screen_audio_sources_loaded = false;
                        state.screen_audio_sources_error = None;
                        let _ = cmd_tx.send(UiCommand::LoadScreenAudioSources);
                    }
                }
                let quality = format!("{}p / {} fps", state.screen_share_max_height, state.screen_share_fps);
                ui.set_next_item_width(180.0);
                if let Some(_combo) = ui.begin_combo("Stream quality", &quality) {
                    for (height, fps, label) in [(720, 15, "720p / 15 fps"), (720, 30, "720p / 30 fps"), (1080, 15, "1080p / 15 fps"), (1080, 30, "1080p / 30 fps"), (1080, 60, "1080p / 60 fps")] {
                        let selected = state.screen_share_max_height == height && state.screen_share_fps == fps;
                        if ui.selectable_config(label).selected(selected).build() {
                            state.screen_share_max_height = height;
                            state.screen_share_fps = fps;
                        }
                    }
                }
                ui.spacing();
                if ui.button("Start sharing") {
                    #[cfg(all(feature = "screenshare", target_os = "macos"))]
                    rustcord_client::screen_capture::set_target_app(state.selected_screen_app_pid);
                    if let Some((guild_id, channel_id)) = state.connected_voice_channel.clone() {
                        state.screen_sharing = true;
                        let _ = cmd_tx.send(UiCommand::SetScreenSharing {
                            enabled: true,
                            include_system_audio: state.share_system_audio,
                            audio_source_pid: state.selected_screen_audio_pid,
                            max_height: state.screen_share_max_height,
                            framerate: state.screen_share_fps,
                            guild_id,
                            channel_id,
                        });
                    }
                    ui.close_current_popup();
                }
                ui.same_line();
                if ui.button("Cancel") {
                    ui.close_current_popup();
                }
            }
        });
}

fn draw_voice_controls(ui: &Ui, state: &mut AppState, textures: &TextureCache, cmd_tx: &UnboundedSender<UiCommand>, guild_id: Option<String>, channel_id: &str, center: bool) {
    const BUTTON: f32 = 30.0;
    const GAP: f32 = 8.0;
    const RED: [f32; 4] = [0.75, 0.25, 0.25, 1.0];
    const GREEN: [f32; 4] = [0.20, 0.60, 0.35, 1.0];
    #[cfg(not(feature = "screenshare"))]
    let _ = channel_id;
    if center {
        let button_count = if cfg!(feature = "screenshare") { 6.0 } else { 5.0 };
        let total = button_count * BUTTON + (button_count - 1.0) * GAP;
        let y = ui.cursor_pos()[1];
        ui.set_cursor_pos([(ui.window_size()[0] - total) / 2.0, y]);
    }
    let muted = state.self_muted;
    if icon_button(ui, textures, "vc_mute", if muted { crate::icons::Icon::MicOff } else { crate::icons::Icon::Mic }, if muted { "Unmute" } else { "Mute" }, muted.then_some(RED)) {
        toggle_voice(state, cmd_tx, VoiceToggle::Mute);
    }
    ui.same_line_with_spacing(0.0, GAP);
    let deafened = state.self_deafened;
    if icon_button(ui, textures, "vc_deafen", if deafened { crate::icons::Icon::HeadphoneOff } else { crate::icons::Icon::Headphones }, if deafened { "Undeafen" } else { "Deafen" }, deafened.then_some(RED)) {
        toggle_voice(state, cmd_tx, VoiceToggle::Deafen);
    }
    ui.same_line_with_spacing(0.0, GAP);
    let denoise = state.denoise;
    if icon_button(ui, textures, "vc_rnn", crate::icons::Icon::AudioWaveform, if denoise { "Noise suppression: on" } else { "Noise suppression: off" }, denoise.then_some(GREEN)) {
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
    if icon_button(ui, textures, "vc_info", crate::icons::Icon::Info, "Connection info", state.show_voice_info.then_some([0.35, 0.40, 0.95, 1.0])) {
        state.show_voice_info = !state.show_voice_info;
    }
    ui.same_line_with_spacing(0.0, GAP);
    #[cfg(feature = "screenshare")]
    {
        let camera = state.camera_on;
        if icon_button(ui, textures, "vc_camera", crate::icons::Icon::Video, camera_tooltip(camera), camera.then_some(GREEN)) {
            toggle_camera(state, cmd_tx);
        }
        ui.same_line_with_spacing(0.0, GAP);
        let active = state.screen_sharing;
        if icon_button(ui, textures, "vc_screenshare", SCREENSHARE_ICON, if active { "Stop screen sharing" } else { "Share your screen" }, active.then_some(GREEN)) {
            if active {
                state.screen_sharing = false;
                let _ = cmd_tx.send(UiCommand::SetScreenSharing { enabled: false, include_system_audio: state.share_system_audio, audio_source_pid: state.selected_screen_audio_pid, max_height: state.screen_share_max_height, framerate: state.screen_share_fps, guild_id: guild_id.clone(), channel_id: channel_id.to_string() });
            } else {
                state.show_screen_share_setup = true;
            }
        }
        ui.same_line_with_spacing(0.0, GAP);
    }
    if icon_button(ui, textures, "vc_disconnect", crate::icons::Icon::PhoneOff, "Disconnect", None) {
        let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id: guild_id.clone() });
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

#[cfg(feature = "screenshare")]
const CALL_PANEL_HEIGHT: f32 = 230.0;
#[cfg(not(feature = "screenshare"))]
const CALL_PANEL_HEIGHT: f32 = 130.0;

#[cfg(feature = "screenshare")]
fn draw_server_call_fullscreen(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some((Some(guild_id), channel_id)) = state.connected_voice_channel.clone() else { return };
    let stream_key = state.watching_stream.clone();
    let owner_id = stream_key.as_ref().and_then(|key| state.active_streams.iter().find_map(|(user_id, active_key)| (active_key == key).then(|| user_id.clone())));
    let owner_name = owner_id.as_ref().and_then(|id| state.known_users.get(id))
        .map(|user| user.global_name.clone().unwrap_or_else(|| user.username.clone()))
        .or(owner_id.clone())
        .unwrap_or_else(|| "No stream selected".into());
    let channel_name = state.connected_voice_channel_name().unwrap_or_else(|| channel_id.clone());
    let [width, height] = ui.window_size();
    let origin = ui.cursor_screen_pos();
    let draw_list = ui.get_window_draw_list();
    draw_list.add_rect(origin, [origin[0] + width, origin[1] + height], [0.035, 0.035, 0.045, 1.0]).filled(true).build();
    drop(draw_list);

    ui.set_cursor_pos([20.0, 14.0]);
    ui.text_colored([0.92, 0.92, 0.96, 1.0], if stream_key.is_some() { format!("{channel_name}  ·  Live: {owner_name}") } else { format!("Voice channel  ·  {channel_name}") });
    let button_width = if stream_key.is_some() { 330.0 } else { 120.0 };
    ui.set_cursor_pos([(width - button_width).max(12.0), 10.0]);
    if stream_key.is_some() {
        if ui.small_button(if state.server_call_focus_stream { "Show tiles##server_call" } else { "Expand stream##server_call" }) {
            state.server_call_focus_stream = !state.server_call_focus_stream;
        }
        ui.same_line();
    }
    if ui.small_button("Return to chat##server_call") {
        state.server_call_minimized = true;
    }
    if let Some(stream_key) = stream_key {
        ui.same_line();
        if ui.small_button("Stop watching##server_call") {
            let _ = cmd_tx.send(UiCommand::StopWatchingStream { stream_key: stream_key.clone() });
            state.unwatching_stream = Some(stream_key);
            state.watching_stream = None;
            state.server_call_minimized = false;
        }
    }

    let stage_top = 48.0;
    let stage_bottom = if state.server_call_focus_stream { height - 108.0 } else { height - 230.0 };
    let stage_width = if state.server_call_focus_stream { width - 32.0 } else { width * 0.92 };
    let stage_width = stage_width.clamp(1.0, (width - 48.0).max(1.0));
    let stage_height = (stage_bottom - stage_top).max(100.0);
    let remote_frame = owner_id.as_deref().and_then(|id| textures.live_video(id));
    let local_preview = state.current_user.as_ref()
        .filter(|user| owner_id.as_deref() == Some(user.id.as_str()) && state.screen_sharing)
        .and_then(|user| textures.live_video(&crate::app_state::screen_preview_video_key(&user.id)));
    let showing_local_preview = remote_frame.is_none() && local_preview.is_some();
    let frame = remote_frame.or(local_preview);
    if let Some((texture_id, source)) = frame {
        let scale = (stage_width / source[0].max(1.0)).min(stage_height / source[1].max(1.0));
        let size = [source[0] * scale, source[1] * scale];
        let pos = [(width - size[0]) * 0.5, stage_top + (stage_height - size[1]) * 0.5];
        ui.set_cursor_pos(pos);
        imgui::Image::new(texture_id, size).build(ui);
        if showing_local_preview {
            ui.set_cursor_pos([20.0, stage_top + 8.0]);
            ui.text_colored([0.7, 0.8, 0.85, 1.0], "Preparing your local preview");
        }
    } else if state.watching_stream.is_some() {
        let text = "Connecting to the stream…";
        let text_size = ui.calc_text_size(text);
        ui.set_cursor_pos([(width - text_size[0]) * 0.5, stage_top + stage_height * 0.5]);
        ui.text_disabled(text);
    }

    let participants: Vec<(rustcord_client::models::User, bool)> = state.members_in_voice_channel(&guild_id, &channel_id)
        .into_iter()
        .filter_map(|voice_state| {
            let user = state.known_users.get(&voice_state.user_id)
                .or_else(|| voice_state.member.as_ref().and_then(|m| m.user.as_ref()))
                .or_else(|| state.current_user.as_ref().filter(|u| u.id == voice_state.user_id))?;
            Some((user.clone(), state.voice_speaking_users.contains(&voice_state.user_id)))
        })
        .collect();
    if state.watching_stream.is_some() {
        if !state.server_call_focus_stream {
            draw_call_participant_tiles(ui, state, textures, cmd_tx, &participants, origin, width, height, false);
        }
    } else {
        if !state.active_streams.is_empty() {
            ui.set_cursor_pos([20.0, 51.0]);
            ui.text_disabled("Right-click a LIVE participant in the channel list to watch their stream.");
        }
        draw_call_participant_tiles(ui, state, textures, cmd_tx, &participants, origin, width, height, true);
    }

    draw_server_call_controls(ui, state, textures, cmd_tx, Some(guild_id), &channel_id);
}

#[cfg(feature = "screenshare")]
fn draw_watched_stream_panel(ui: &Ui, state: &mut AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>) -> f32 {
    let Some(stream_key) = state.watching_stream.clone() else { return 0.0 };
    let owner_id = state.active_streams.iter().find_map(|(user_id, key)| (key == &stream_key).then(|| user_id.clone()));
    let owner_name = owner_id.as_ref().and_then(|id| state.known_users.get(id))
        .map(|user| user.global_name.clone().unwrap_or_else(|| user.username.clone()))
        .or(owner_id.clone())
        .unwrap_or_else(|| "screen share".into());
    let remote_frame = owner_id.as_deref().and_then(|id| textures.live_video(id));
    let local_preview = state.current_user.as_ref()
        .filter(|user| owner_id.as_deref() == Some(user.id.as_str()) && state.screen_sharing)
        .and_then(|user| textures.live_video(&crate::app_state::screen_preview_video_key(&user.id)));
    let showing_local_preview = remote_frame.is_none() && local_preview.is_some();
    let frame = remote_frame.or(local_preview);
    let image_size = frame.map(|(_, source)| {
        let max_width = (ui.window_size()[0] - 24.0).clamp(1.0, 900.0);
        let max_height = 400.0;
        let scale = (max_width / source[0].max(1.0)).min(max_height / source[1].max(1.0));
        [source[0] * scale, source[1] * scale]
    });
    let panel_height = image_size.map_or(74.0, |size| size[1] + 44.0);
    ui.child_window("watched_stream_panel").size([0.0, panel_height]).build(|| {
        ui.text(if showing_local_preview { format!("Local preview: {owner_name}") } else { format!("Live: {owner_name}") });
        ui.same_line();
        if ui.small_button("Stop watching##live_stream_panel") {
            let _ = cmd_tx.send(UiCommand::StopWatchingStream { stream_key: stream_key.clone() });
            state.unwatching_stream = Some(stream_key.clone());
            state.watching_stream = None;
        }
        if let (Some((texture_id, _)), Some(size)) = (frame, image_size) {
            let x = ((ui.window_size()[0] - size[0]) * 0.5).max(0.0);
            ui.set_cursor_pos([x, 28.0]);
            imgui::Image::new(texture_id, size).build(ui);
        } else {
            ui.text_disabled("Connecting to the stream…");
        }
    });
    panel_height
}

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
    #[cfg(feature = "screenshare")]
    let live_videos: Vec<_> = members
        .iter()
        .map(|(user, _)| textures.live_video(&user.id)
            .or_else(|| textures.live_video(&crate::app_state::camera_video_key(&user.id)))
            .or_else(|| state.current_user.as_ref().filter(|me| me.id == user.id)
                .and_then(|_| textures.live_video(&crate::app_state::screen_preview_video_key(&user.id)))))
        .collect();
    ui.child_window("dm_call_panel").size([0.0, CALL_PANEL_HEIGHT]).build(|| {
        let width = ui.window_size()[0];
        #[cfg(feature = "screenshare")]
        let row_w = live_videos.iter().map(|live| if live.is_some() { 180.0 } else { SIZE }).sum::<f32>()
            + members.len().saturating_sub(1) as f32 * GAP;
        #[cfg(not(feature = "screenshare"))]
        let row_w = members.len() as f32 * SIZE + members.len().saturating_sub(1) as f32 * GAP;
        ui.set_cursor_pos([(width - row_w) / 2.0, 12.0]);
        let mut row_height = SIZE;
        for (member_index, (user, speaking)) in members.iter().enumerate() {
            let p = ui.cursor_screen_pos();
            #[cfg(feature = "screenshare")]
            let live = live_videos.get(member_index).and_then(|v| *v);
            #[cfg(feature = "screenshare")]
            let tile_size = if let Some((_, source_size)) = live {
                let scale = (180.0 / source_size[0].max(1.0)).min(112.0 / source_size[1].max(1.0));
                [source_size[0] * scale, source_size[1] * scale]
            } else {
                [SIZE, SIZE]
            };
            #[cfg(not(feature = "screenshare"))]
            let tile_size = [SIZE, SIZE];
            row_height = row_height.max(tile_size[1]);
            let center = [p[0] + tile_size[0] / 2.0, p[1] + tile_size[1] / 2.0];
            let url = rustcord_client::cdn::avatar_url(user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
            #[cfg(feature = "screenshare")]
            if let Some((texture_id, _)) = live {
                imgui::Image::new(texture_id, tile_size).build(ui);
            } else {
                match textures.get(&url) {
                    Some((texture_id, _)) => draw_circular_image(ui, texture_id, [SIZE, SIZE]),
                    None => {
                        textures.request(&url, cmd_tx);
                        ui.dummy([SIZE, SIZE]);
                    }
                }
            }
            #[cfg(not(feature = "screenshare"))]
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
            if member_index + 1 < members.len() {
                ui.same_line_with_spacing(0.0, GAP);
            }
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
            #[cfg(feature = "screenshare")]
            if let Some(stream_key) = state.active_streams.get(&member_id).cloned() {
                if state.watching_stream.as_deref() == Some(stream_key.as_str()) {
                    if ui.selectable("Stop watching") {
                        let _ = cmd_tx.send(UiCommand::StopWatchingStream { stream_key: stream_key.clone() });
                        state.unwatching_stream = Some(stream_key.clone());
                        state.watching_stream = None;
                        state.server_call_minimized = false;
                    }
                } else if ui.selectable("Watch screen share") {
                    let _ = cmd_tx.send(UiCommand::WatchStream { stream_key: stream_key.clone() });
                    state.watching_stream = Some(stream_key);
                    state.server_call_minimized = false;
                    state.server_call_focus_stream = false;
                }
            }
            draw_user_volume_controls(ui, state, cmd_tx, &member_id);
        });
        if in_call {
            ui.set_cursor_pos([0.0, 12.0 + row_height + 18.0]);
            draw_voice_controls(ui, state, textures, cmd_tx, None, &channel_id, true);
        }
    });
}

fn draw_channel_header(
    ui: &Ui,
    state: &mut AppState,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    dm: Option<&rustcord_client::models::Channel>,
) {
    const ICON_SIZE: f32 = 30.0;
    const ICON_GAP: f32 = 4.0;

    let guild_channel = match dm {
        Some(_) => None,
        None => state
            .selected_channel
            .as_ref()
            .and_then(|id| state.guild_channels.values().flatten().find(|c| &c.id == id))
            .cloned(),
    };

    let (title, topic, avatar) = match (dm, &guild_channel) {
        (Some(dm), _) => {
            let avatar = dm.recipients.first().map(|u| {
                rustcord_client::cdn::avatar_url(u, 64).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(u))
            });
            (AppState::dm_display_name(dm), None, avatar)
        }
        (None, Some(channel)) => (
            format!("#{}", channel.name.clone().unwrap_or_default()),
            channel.topic.clone().filter(|t| !t.trim().is_empty()),
            None,
        ),
        (None, None) => return,
    };

    let is_dm = dm.is_some();
    let channel_id = dm.map(|d| d.id.clone()).or_else(|| guild_channel.as_ref().map(|c| c.id.clone()));
    let is_connected = channel_id
        .as_ref()
        .is_some_and(|id| state.connected_voice_channel.as_ref().is_some_and(|(_, cid)| cid == id));
    let is_connecting = state.voice_connecting
        && channel_id
            .as_ref()
            .is_some_and(|id| state.pending_voice_channel.as_ref().is_some_and(|(_, cid)| cid == id));

    ui.child_window("channel_header").size([0.0, HEADER_HEIGHT]).build(|| {
        let mut text_x = 12.0;
        if let Some(url) = &avatar {
            if let Some((texture_id, _)) = textures.get(url) {
                ui.set_cursor_pos([12.0, (HEADER_HEIGHT - 24.0) / 2.0]);
                draw_circular_image(ui, texture_id, [24.0, 24.0]);
            } else {
                textures.request(url, cmd_tx);
            }
            text_x = 44.0;
        }

        let is_one_to_one = dm.is_some_and(|d| d.kind == rustcord_client::models::ChannelType::Dm);
        let icon_count = match (is_dm, is_one_to_one) {
            (true, true) => 6,
            (true, false) => 5,
            _ => 2,
        };
        let icons_width = icon_count as f32 * (ICON_SIZE + ICON_GAP) + HEADER_SEARCH_WIDTH;
        let icons_x = ui.window_size()[0] - icons_width - 12.0;

        ui.set_cursor_pos([text_x, (HEADER_HEIGHT - ui.text_line_height()) / 2.0]);
        let title_room = (icons_x - text_x - 12.0).max(40.0);
        let clipped_title = truncate_to_width(ui, &title, title_room);
        let drawn_title = clipped_title.as_deref().unwrap_or(title.as_str());
        ui.text(drawn_title);
        if clipped_title.is_some() && ui.is_item_hovered() {
            ui.tooltip_text(&title);
        }

        if let Some(topic) = topic.filter(|_| clipped_title.is_none()) {
            let topic_x = text_x + ui.calc_text_size(drawn_title)[0] + 12.0;
            let topic_room = icons_x - topic_x - 12.0;
            if topic_room > 60.0 {
                ui.same_line_with_pos(topic_x);
                let separator = ui.push_style_color(imgui::StyleColor::Text, state.theme.border);
                ui.text("|");
                separator.pop();
                ui.same_line();
                let muted = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
                let one_line = topic.replace('\n', " ");
                match truncate_to_width(ui, &one_line, topic_room - 16.0) {
                    Some(cut) => {
                        ui.text(&cut);
                        muted.pop();
                        if ui.is_item_hovered() {
                            ui.tooltip_text(&topic);
                        }
                    }
                    None => {
                        ui.text(&one_line);
                        muted.pop();
                    }
                }
            }
        }

        ui.set_cursor_pos([icons_x, (HEADER_HEIGHT - ICON_SIZE) / 2.0]);

        if is_dm {
            let Some(channel_id) = channel_id.clone() else { return };
            let call_tint = is_connected.then_some(state.theme.accent);
            let call_tip = if is_connected { "Hang up" } else { "Start voice call" };
            let call_disabled = ui.begin_disabled(is_connecting);
            let call_clicked = icon_button(ui, textures, "header_call", HEADER_ICONS[0], call_tip, call_tint);
            drop(call_disabled);
            if call_clicked && !is_connecting {
                if is_connected {
                    let _ = cmd_tx.send(UiCommand::LeaveVoiceChannel { guild_id: None });
                } else {
                    let _ = cmd_tx
                        .send(UiCommand::JoinVoiceChannel { guild_id: None, channel_id: channel_id.clone(), ring: true });
                    state.pending_voice_channel = Some((None, channel_id.clone()));
                }
            }
            ui.same_line_with_spacing(0.0, ICON_GAP);
            #[cfg(feature = "screenshare")]
            {
                let disabled = ui.begin_disabled(!is_connected);
                let tint = state.camera_on.then_some(state.theme.accent);
                let clicked = icon_button(ui, textures, "header_camera", HEADER_ICONS[1], camera_tooltip(state.camera_on), tint);
                drop(disabled);
                if clicked && is_connected {
                    toggle_camera(state, cmd_tx);
                }
            }
            #[cfg(not(feature = "screenshare"))]
            {
                let disabled = ui.begin_disabled(true);
                icon_button(ui, textures, "header_camera", HEADER_ICONS[1], "Video calls aren't available in this build", None);
                drop(disabled);
            }
            ui.same_line_with_spacing(0.0, ICON_GAP);

            #[cfg(feature = "screenshare")]
            {
                let disabled = ui.begin_disabled(!is_connected);
                let tint = state.screen_sharing.then_some(state.theme.accent);
                let tip = if state.screen_sharing { "Stop screen sharing" } else { "Share your screen" };
                let clicked = icon_button(ui, textures, "header_video", SCREENSHARE_ICON, tip, tint);
                drop(disabled);
                if clicked && is_connected {
                    if state.screen_sharing {
                        state.screen_sharing = false;
                        let _ = cmd_tx.send(UiCommand::SetScreenSharing { enabled: false, include_system_audio: state.share_system_audio, audio_source_pid: state.selected_screen_audio_pid, max_height: state.screen_share_max_height, framerate: state.screen_share_fps, guild_id: None, channel_id: channel_id.clone() });
                    } else {
                        state.show_screen_share_setup = true;
                    }
                }
            }
            #[cfg(not(feature = "screenshare"))]
            {
                let disabled = ui.begin_disabled(true);
                icon_button(ui, textures, "header_video", SCREENSHARE_ICON, "Screen sharing isn't available in this build", None);
                drop(disabled);
            }
            ui.same_line_with_spacing(0.0, ICON_GAP);
            draw_pins_button(ui, state, cmd_tx, textures, &channel_id);
            ui.same_line_with_spacing(0.0, ICON_GAP);
            if icon_button(ui, textures, "header_add_people", crate::icons::Icon::UserPlus, "Add Friends to DM", None) {
                state.add_people_selected.clear();
                state.add_people_filter.clear();
                ui.open_popup("add_people");
            }
            draw_add_people_popup(ui, state, cmd_tx, dm);
            ui.same_line_with_spacing(0.0, ICON_GAP);
            if is_one_to_one {
                let tint = state.show_profile_panel.then_some(state.theme.accent);
                let tip = if state.show_profile_panel { "Hide User Profile" } else { "Show User Profile" };
                if icon_button(ui, textures, "header_profile", crate::icons::Icon::UserRound, tip, tint) {
                    state.show_profile_panel = !state.show_profile_panel;
                    if state.show_profile_panel {
                        state.search_open = false;
                    }
                }
                ui.same_line_with_spacing(0.0, ICON_GAP);
            }
        } else {
            if let Some(channel_id) = channel_id.clone() {
                draw_pins_button(ui, state, cmd_tx, textures, &channel_id);
                ui.same_line_with_spacing(0.0, ICON_GAP);
            }
            let members_tint = state.show_members_panel.then_some(state.theme.accent);
            if icon_button(ui, textures, "header_members", HEADER_ICONS[2], "Members", members_tint) {
                state.show_members_panel = !state.show_members_panel;
                if state.show_members_panel {
                    state.search_open = false;
                    if let Some(guild_id) = state.selected_guild.clone() {
                        if state.guild_members_requested.insert(guild_id.clone()) {
                            let _ = cmd_tx.send(UiCommand::RequestGuildMembers { guild_id });
                        }
                    }
                }
            }
            ui.same_line_with_spacing(0.0, ICON_GAP);
        }

        let search_y = (HEADER_HEIGHT - ui.frame_height()) / 2.0;
        ui.set_cursor_pos([ui.cursor_pos()[0], search_y]);
        let search_min = ui.cursor_screen_pos();
        ui.set_next_item_width(HEADER_SEARCH_WIDTH);
        let submitted = ui.input_text("##header_search", &mut state.search_input).hint("Search").enter_returns_true(true).build();
        let icon = 16.0;
        draw_icon_at(
            ui,
            textures,
            HEADER_ICONS[3],
            [search_min[0] + HEADER_SEARCH_WIDTH - icon - 8.0, search_min[1] + (ui.frame_height() - icon) / 2.0],
            icon,
            ui.style_color(imgui::StyleColor::TextDisabled),
        );
        if submitted {
            if !state.search_open {
                toggle_search_panel(state);
            }
            state.show_profile_panel = false;
            submit_search(state, cmd_tx, 0);
        }
    });
    ui.separator();
}

const HEADER_SEARCH_WIDTH: f32 = 180.0;

fn draw_pins_button(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache, channel_id: &str) {
    use crate::app_state::PinsState;
    if icon_button(ui, textures, "header_pins", crate::icons::Icon::Pin, "Pinned Messages", None) {
        state.pins.insert(channel_id.to_string(), PinsState::Loading);
        let _ = cmd_tx.send(UiCommand::LoadPins { channel_id: channel_id.to_string() });
        ui.open_popup("pinned_messages");
    }
    let mut jump = None;
    ui.popup("pinned_messages", || {
        ui.text("Pinned Messages");
        ui.separator();
        match state.pins.get(channel_id) {
            None | Some(PinsState::Loading) => ui.text_disabled("Loading..."),
            Some(PinsState::Failed) => ui.text_disabled("Couldn't load pinned messages."),
            Some(PinsState::Ready(pins)) if pins.is_empty() => ui.text_disabled("This channel doesn't have any pinned messages yet."),
            Some(PinsState::Ready(pins)) => {
                ui.child_window("pins_list").size([380.0, 360.0f32.min(pins.len() as f32 * 64.0 + 8.0)]).build(|| {
                    for pin in pins {
                        let _id = ui.push_id(pin.id.as_str());
                        let start = ui.cursor_pos();
                        let clicked = ui.selectable_config("##pin").size([0.0, 56.0]).build();
                        ui.set_cursor_pos([start[0] + 6.0, start[1] + 4.0]);
                        ui.text(pin.author.display_name());
                        ui.same_line();
                        ui.text_disabled(format_result_timestamp(&pin.timestamp));
                        ui.set_cursor_pos([start[0] + 6.0, start[1] + 6.0 + ui.text_line_height()]);
                        let preview = pin.content.lines().next().unwrap_or_default();
                        let preview = if preview.is_empty() && !pin.attachments.is_empty() { "[attachment]" } else { preview };
                        let room = ui.content_region_avail()[0] - 12.0;
                        ui.text_disabled(truncate_to_width(ui, preview, room).as_deref().unwrap_or(preview));
                        ui.set_cursor_pos([start[0], start[1] + 60.0]);
                        if clicked {
                            jump = Some((pin.channel_id.clone(), pin.id.clone()));
                        }
                    }
                });
            }
        }
    });
    if let Some((channel_id, message_id)) = jump {
        let loaded = state.messages.get(&channel_id).is_some_and(|list| list.iter().any(|m| m.id == message_id));
        if loaded {
            state.scroll_to_message = Some(message_id);
        } else {
            let _ = cmd_tx.send(UiCommand::JumpToMessage { channel_id, message_id });
        }
    }
}

fn draw_add_people_popup(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, dm: Option<&rustcord_client::models::Channel>) {
    const GROUP_LIMIT: usize = 10;
    let Some(dm) = dm else { return };
    ui.popup("add_people", || {
        let existing: Vec<&str> = dm.recipients.iter().map(|u| u.id.as_str()).collect();
        let room = GROUP_LIMIT.saturating_sub(existing.len() + 1);
        ui.text("Select Friends");
        ui.text_disabled(format!("You can add {} more friend{}.", room.saturating_sub(state.add_people_selected.len()), if room == 1 { "" } else { "s" }));
        ui.set_next_item_width(300.0);
        ui.input_text("##add_people_filter", &mut state.add_people_filter).hint("Type the username of a friend").build();
        let filter = state.add_people_filter.trim().to_lowercase();
        let friends: Vec<(String, String, String)> = state
            .relationships
            .iter()
            .filter(|r| r.kind == rustcord_client::models::RelationshipType::Friend)
            .filter(|r| !existing.contains(&r.user.id.as_str()))
            .filter(|r| {
                filter.is_empty()
                    || r.user.username.to_lowercase().contains(&filter)
                    || r.user.display_name().to_lowercase().contains(&filter)
            })
            .map(|r| (r.user.id.clone(), r.user.display_name().to_string(), r.user.username.clone()))
            .collect();
        ui.child_window("add_people_list").size([300.0, 240.0]).build(|| {
            if friends.is_empty() {
                ui.text_disabled("No friends found.");
            }
            for (id, display, username) in &friends {
                let mut checked = state.add_people_selected.contains(id);
                let full = !checked && state.add_people_selected.len() >= room;
                let disabled = ui.begin_disabled(full);
                if ui.checkbox(format!("{display}  ({username})##{id}"), &mut checked) {
                    if checked {
                        state.add_people_selected.insert(id.clone());
                    } else {
                        state.add_people_selected.remove(id);
                    }
                }
                drop(disabled);
            }
        });
        let label = if dm.kind == rustcord_client::models::ChannelType::GroupDm { "Add" } else { "Create Group DM" };
        let disabled = ui.begin_disabled(state.add_people_selected.is_empty());
        if ui.button(label) {
            let user_ids: Vec<String> = state.add_people_selected.drain().collect();
            let _ = cmd_tx.send(UiCommand::AddDmRecipients {
                channel_id: dm.id.clone(),
                existing: existing.iter().map(|id| id.to_string()).collect(),
                group: dm.kind == rustcord_client::models::ChannelType::GroupDm,
                user_ids,
            });
            ui.close_current_popup();
        }
        drop(disabled);
    });
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
    let member_color = if offline {
        None
    } else {
        state.selected_guild.clone().and_then(|g| state.member_color(&g, &member.user.id))
    };
    let _dim = offline.then(|| ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted));
    let result = draw_user_row(ui, cmd_tx, textures, &member.user, &name, false, status, platform.as_deref(), activity.as_deref(), None, state.theme.panel, member_color);
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
        let can_moderate = state.has_guild_permission(&guild_id, rustcord_client::permissions::MODERATE_MEMBERS);
        let can_manage_roles = state.has_guild_permission(&guild_id, rustcord_client::permissions::MANAGE_ROLES);
        let member = state.guild_members.get(&guild_id).and_then(|m| m.iter().find(|m| m.user.id == member_id)).cloned();
        if can_moderate || can_manage_roles {
            ui.separator();
        }
        if can_moderate {
            let timed_out = member
                .as_ref()
                .and_then(|m| crate::app_state::active_timeout(m.communication_disabled_until.as_deref(), chrono::Utc::now()));
            if let Some(until) = timed_out {
                ui.text_disabled(format!("Timed out until {}", until.with_timezone(&chrono::Local).format("%b %-d, %H:%M")));
            }
            ui.menu("Timeout", || {
                for (label, secs) in crate::app_state::TIMEOUT_CHOICES {
                    if ui.menu_item(label) {
                        let _ = cmd_tx.send(UiCommand::TimeoutMember { guild_id: guild_id.clone(), user_id: member_id.clone(), duration_secs: Some(*secs) });
                    }
                }
                if timed_out.is_some() {
                    ui.separator();
                    if ui.menu_item("Remove timeout") {
                        let _ = cmd_tx.send(UiCommand::TimeoutMember { guild_id: guild_id.clone(), user_id: member_id.clone(), duration_secs: None });
                    }
                }
            });
        }
        if can_manage_roles {
            if let Some(member) = &member {
                let roles: Vec<(String, String, Option<[f32; 4]>)> =
                    state.assignable_roles(&guild_id).iter().map(|r| (r.id.clone(), r.name.clone(), r.rgba())).collect();
                ui.menu_with_enabled("Roles", !roles.is_empty(), || {
                    for (role_id, name, color) in &roles {
                        let mut has = member.roles.contains(role_id);
                        let _tint = color.map(|c| ui.push_style_color(imgui::StyleColor::Text, c));
                        if ui.checkbox(format!("{name}##role_{role_id}"), &mut has) {
                            let _ = cmd_tx.send(UiCommand::SetMemberRole { guild_id: guild_id.clone(), user_id: member_id.clone(), role_id: role_id.clone(), add: has });
                        }
                    }
                });
            }
        }
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
        let forced = state.pending_sidebar_tab.take();
        for (i, label) in labels.iter().enumerate() {
            let flags = match forced {
                Some(target) if target == i => imgui::TabItemFlags::SET_SELECTED,
                _ => imgui::TabItemFlags::empty(),
            };
            if ui.tab_item_with_flags(format!("{label}###tab{i}"), None, flags).is_some() {
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
            let result = draw_user_row(ui, cmd_tx, textures, user, &name, false, status, platform.as_deref(), activity.as_deref(), None, state.theme.panel, None);
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
    let dms: Vec<(String, String, Option<rustcord_client::models::User>, Option<String>, usize, Option<String>)> = state
        .filtered_dm_channels()
        .into_iter()
        .filter(|dm| group.is_none_or(|g| state.groups[g].channels.contains(&dm.id)))
        .map(|dm| {
            let recipient = (dm.kind == rustcord_client::models::ChannelType::Dm).then(|| dm.recipients.first().cloned()).flatten();
            let icon = dm.icon.as_deref().map(|hash| rustcord_client::cdn::channel_icon_url(&dm.id, hash, 64));
            (dm.id.clone(), AppState::dm_display_name(dm), recipient, dm.last_message_id.clone(), dm.recipients.len() + 1, icon)
        })
        .collect();
    if dms.is_empty() {
        ui.text_disabled(if group.is_some() { "Empty group. Right-click a DM to add it." } else { "No direct messages." });
    }
    for (channel_id, name, recipient, last_message_id, member_count, icon) in &dms {
        let is_selected = state.selected_channel.as_deref() == Some(channel_id.as_str());
        let row_start = ui.cursor_screen_pos();
        let (row_clicked, avatar_clicked, right_clicked) = if let Some(user) = recipient {
            let status = state.presence_for(&user.id);
            let platform = state.platform_for(&user.id).map(str::to_string);
            let status_line = state.status_line_for(&user.id);
            let (status_emoji, status_text) = match &status_line {
                Some((emoji, text)) => (emoji.as_ref(), Some(text.as_str())),
                None => (None, None),
            };
            let r = draw_user_row(ui, cmd_tx, textures, user, name, is_selected, status, platform.as_deref(), status_text, status_emoji, state.theme.panel, None);
            (r.row_clicked, r.avatar_clicked, r.right_clicked)
        } else {
            draw_group_dm_row(ui, state, cmd_tx, textures, channel_id, name, *member_count, icon.as_deref(), is_selected)
        };
        if state.channel_shows_unread(None, channel_id, last_message_id.as_deref()) {
            draw_unread_dot(ui, row_start, ui.item_rect_size()[1]);
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
            state.server_call_minimized = true;
            if let Some(last_id) = last_message_id {
                if state.mark_channel_read(channel_id, last_id) {
                    let _ = cmd_tx.send(UiCommand::AckMessage { channel_id: channel_id.clone(), message_id: last_id.clone() });
                }
            }
        }
    }

    ui.popup("dm_context", || {
        let Some(channel_id) = state.context_dm.clone() else { return };
        if let Some(mute) = mute_menu(ui, "conversation", state.channel_mute_is_direct(None, &channel_id, chrono::Utc::now())) {
            let fields = crate::app_state::mute_patch(mute.is_some(), mute.unwrap_or(-1), chrono::Utc::now());
            let patch = rustcord_client::models::channel_override_patch(&channel_id, fields);
            let _ = cmd_tx.send(UiCommand::UpdateGuildSettings { guild_key: "@me".into(), patch });
        }
        ui.separator();
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

#[allow(clippy::too_many_arguments)]
fn draw_group_dm_row(
    ui: &Ui,
    state: &AppState,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    channel_id: &str,
    name: &str,
    member_count: usize,
    icon_url: Option<&str>,
    selected: bool,
) -> (bool, bool, bool) {
    let row_start = ui.cursor_pos();
    let row_height = USER_ROW_AVATAR_SIZE[1] + 8.0;
    let _id = ui.push_id(channel_id);
    let clicked = ui.selectable_config("##group_dm_row").selected(selected).size([0.0, row_height]).build();
    let right_clicked = ui.is_item_hovered() && ui.is_mouse_clicked(imgui::MouseButton::Right);
    ui.set_cursor_pos([row_start[0] + 4.0, row_start[1] + 4.0]);
    let icon_min = ui.cursor_screen_pos();
    let size = USER_ROW_AVATAR_SIZE[0];
    match icon_url.and_then(|url| textures.get_still(url)) {
        Some((texture, _)) => draw_circular_image(ui, texture, USER_ROW_AVATAR_SIZE),
        None => {
            if let Some(url) = icon_url {
                textures.request(url, cmd_tx);
            }
            ui.get_window_draw_list()
                .add_circle([icon_min[0] + size / 2.0, icon_min[1] + size / 2.0], size / 2.0, state.theme.accent)
                .filled(true)
                .num_segments(32)
                .build();
            let glyph = 16.0;
            let inset = (size - glyph) / 2.0;
            draw_icon_at(ui, textures, crate::icons::Icon::Users, [icon_min[0] + inset, icon_min[1] + inset], glyph, [1.0, 1.0, 1.0, 1.0]);
            ui.dummy(USER_ROW_AVATAR_SIZE);
        }
    }
    ui.same_line();
    let text_x = ui.cursor_pos()[0];
    let room = ui.content_region_avail()[0] - 4.0;
    ui.set_cursor_pos([text_x, row_start[1] + 3.0]);
    ui.text(truncate_to_width(ui, name, room).as_deref().unwrap_or(name));
    ui.set_cursor_pos([text_x, row_start[1] + 3.0 + ui.text_line_height()]);
    let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
    ui.set_window_font_scale(0.8);
    ui.text(format!("{member_count} Members"));
    ui.set_window_font_scale(1.0);
    token.pop();
    ui.set_cursor_pos([row_start[0], row_start[1] + row_height + ui.clone_style().item_spacing[1]]);
    (clicked, false, right_clicked)
}

fn draw_profile_panel(
    ui: &Ui,
    state: &mut AppState,
    cmd_tx: &UnboundedSender<UiCommand>,
    textures: &mut TextureCache,
    fonts: &Fonts,
    user: &rustcord_client::models::User,
) {
    const BANNER_HEIGHT: f32 = 110.0;
    const AVATAR: f32 = 80.0;
    if !state.profiles_requested.contains(&user.id) {
        state.profiles_requested.insert(user.id.clone());
        let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: user.id.clone() });
    }
    let width = ui.content_region_avail()[0];
    let origin = ui.cursor_screen_pos();
    let banner_url = state.profiles.get(&user.id).and_then(|p| p.banner_url.clone());
    match banner_url.as_deref().and_then(|url| textures.get(url).map(|t| (url, t))) {
        Some((_, (texture, _))) => {
            ui.get_window_draw_list().add_image_rounded(texture, origin, [origin[0] + width, origin[1] + BANNER_HEIGHT], 6.0).build();
        }
        None => {
            if let Some(url) = banner_url.as_deref() {
                textures.request(url, cmd_tx);
            }
            ui.get_window_draw_list()
                .add_rect(origin, [origin[0] + width, origin[1] + BANNER_HEIGHT], state.theme.accent)
                .filled(true)
                .rounding(6.0)
                .build();
        }
    }
    let start = ui.cursor_pos();
    ui.set_cursor_pos([start[0] + 12.0, start[1] + BANNER_HEIGHT - AVATAR / 2.0]);
    let avatar_min = ui.cursor_screen_pos();
    let radius = AVATAR / 2.0;
    ui.get_window_draw_list()
        .add_circle([avatar_min[0] + radius, avatar_min[1] + radius], radius + 5.0, state.theme.panel)
        .filled(true)
        .num_segments(48)
        .build();
    let avatar_url = rustcord_client::cdn::avatar_url(user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(user));
    match textures.get(&avatar_url) {
        Some((texture, _)) => draw_circular_image(ui, texture, [AVATAR; 2]),
        None => {
            textures.request(&avatar_url, cmd_tx);
            ui.dummy([AVATAR; 2]);
        }
    }
    draw_status_dot(ui, [avatar_min[0] + AVATAR - 10.0, avatar_min[1] + AVATAR - 10.0], 8.0, state.presence_for(&user.id), state.theme.panel);

    ui.dummy([0.0, 6.0]);
    let bold = ui.push_font(fonts.bold);
    ui.set_window_font_scale(1.3);
    ui.text_wrapped(user.display_name());
    ui.set_window_font_scale(1.0);
    bold.pop();
    ui.text(&user.username);
    if let Some(tag) = user.server_tag() {
        ui.same_line_with_spacing(0.0, 6.0);
        draw_server_tag(ui, textures, cmd_tx, tag, tag.badge_url(32).as_deref());
    }
    if let Some((emoji, text)) = state.status_line_for(&user.id) {
        if let Some(emoji) = &emoji {
            draw_status_emoji(ui, textures, cmd_tx, emoji, ui.text_line_height());
            ui.same_line_with_spacing(0.0, 4.0);
        }
        ui.text_colored(state.theme.text_muted, text);
    }

    ui.dummy([0.0, 6.0]);
    ui.separator();
    if let Some(bio) = state.profiles.get(&user.id).and_then(|p| p.bio.clone()) {
        let bold = ui.push_font(fonts.bold);
        ui.text_colored(state.theme.text_muted, "About Me");
        bold.pop();
        ui.text_wrapped(bio);
        ui.dummy([0.0, 4.0]);
    }
    let bold = ui.push_font(fonts.bold);
    ui.text_colored(state.theme.text_muted, "Member Since");
    bold.pop();
    ui.text(snowflake_date(&user.id));
    if let Some(label) = state.mutual_server_labels.get(&user.id).filter(|_| {
        state.profiles.get(&user.id).is_some_and(|p| !p.mutual_guild_ids.is_empty())
    }) {
        ui.dummy([0.0, 4.0]);
        ui.text_colored(state.theme.text_muted, label);
    }
    ui.dummy([0.0, 8.0]);
    if ui.button_with_size("View Full Profile", [-1.0, 0.0]) {
        state.profile_popout = Some(user.clone());
    }
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

const SETTINGS_SIDEBAR_WIDTH: f32 = 200.0;

fn draw_settings_dialog(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
    if !state.show_settings {
        return;
    }
    let [display_w, display_h] = ui.io().display_size;
    let opaque_bg = [state.theme.background[0], state.theme.background[1], state.theme.background[2], 1.0];
    let _bg_token = ui.push_style_color(imgui::StyleColor::WindowBg, opaque_bg);
    let mut logged_out = false;
    ui.window("Settings")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size([display_w, display_h], imgui::Condition::Always)
        .no_decoration()
        .build(|| {
            const CLOSE_SIZE: f32 = 30.0;
            const CONTENT_PAD: f32 = 32.0;
            let content_x = SETTINGS_SIDEBAR_WIDTH + CONTENT_PAD;
            let content_w = (display_w - content_x - CONTENT_PAD).max(120.0);

            ui.set_cursor_pos([0.0, 0.0]);
            let _sidebar_bg = ui.push_style_color(imgui::StyleColor::ChildBg, state.theme.panel);
            ui.child_window("settings_sidebar").size([SETTINGS_SIDEBAR_WIDTH, display_h]).build(|| {
                ui.spacing();
                for category in SettingsCategory::ALL {
                    let y = ui.cursor_pos()[1];
                    ui.set_cursor_pos([16.0, y]);
                    if ui.selectable_config(category.label()).selected(state.settings_category == category).build() {
                        state.settings_category = category;
                    }
                }
            });
            _sidebar_bg.pop();

            ui.set_cursor_pos([content_x, CONTENT_PAD]);
            ui.child_window("settings_content").size([content_w, display_h - CONTENT_PAD * 2.0]).build(|| {
                let heading_w = (content_w - CLOSE_SIZE - 12.0).max(40.0);
                ui.set_window_font_scale(1.25);
                ui.text(state.settings_category.label());
                ui.set_window_font_scale(1.0);
                ui.same_line_with_spacing(heading_w, 0.0);
                if icon_button(ui, textures, "close_settings", CHROME_ICONS[0], "Close settings", None) {
                    state.show_settings = false;
                }
                ui.spacing();
                ui.separator();
                ui.spacing();
                match state.settings_category {
                    SettingsCategory::Account => {
                        if draw_settings_account(ui, state, cmd_tx, textures) {
                            logged_out = true;
                        }
                    }
                    SettingsCategory::Notifications => draw_settings_notifications(ui, state),
                    SettingsCategory::Activity => draw_settings_activity(ui, state, cmd_tx, textures),
                    SettingsCategory::VoiceVideo => draw_settings_voice(ui, state, cmd_tx),
                    SettingsCategory::Keybinds => draw_settings_keybinds(ui, state),
                    SettingsCategory::Appearance => draw_settings_appearance(ui, state),
                    SettingsCategory::About => draw_settings_about(ui, state, textures),
                }
            });
        });
    if logged_out {
        let theme = state.theme.clone();
        *state = AppState { theme, ..Default::default() };
    }
}

fn draw_settings_account(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) -> bool {
    let Some(user) = state.current_user.clone() else { return false };
    if state.profiles_requested.insert(user.id.clone()) {
        let _ = cmd_tx.send(UiCommand::LoadProfile { user_id: user.id.clone() });
    }
    let profile = state.profiles.get(&user.id).cloned().unwrap_or_default();

    let width = ui.content_region_avail()[0].min(480.0);
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
    let avatar_url = rustcord_client::cdn::avatar_url(&user, 128).unwrap_or_else(|| rustcord_client::cdn::default_avatar_url(&user));
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
    ui.set_cursor_pos([banner_pos[0], banner_pos[1] + banner_size[1] + 52.0]);

    let display_name = user.global_name.clone().unwrap_or_else(|| user.username.clone());
    ui.text(format!("Username: {display_name} (@{})", user.username));
    ui.text(format!("User ID: {}", user.id));
    ui.spacing();
    if ui.button("Log out") {
        let _ = cmd_tx.send(UiCommand::Logout);
        return true;
    }
    false
}

fn draw_settings_notifications(ui: &Ui, state: &mut AppState) {
    ui.checkbox("Desktop notifications for DMs and mentions", &mut state.notifications_enabled);
}

fn draw_settings_activity(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache) {
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
}

fn draw_settings_voice(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let muted = state.theme.text_muted;
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

        ui.spacing();
        if state.mic_test_running {
            ui.text_colored(state.theme.accent, "Recording... speak now (10s), then it will play back");
        } else if ui.button("Mic Test") {
            let _ = cmd_tx.send(UiCommand::RunMicTest {
                input_device: state.selected_input_device.clone(),
                output_device: state.selected_output_device.clone(),
            });
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

    #[cfg(feature = "screenshare")]
    draw_camera_settings(ui, state, cmd_tx);
}

#[cfg(feature = "screenshare")]
fn draw_camera_settings(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    if !state.cameras_loaded {
        state.cameras_loaded = true;
        let _ = cmd_tx.send(UiCommand::LoadCameras);
    }
    ui.spacing();
    ui.text_colored(state.theme.text_muted, "Camera");
    let current = match &state.camera_device {
        None => "Default".to_string(),
        Some(id) => state.cameras.iter().find(|(cid, _)| cid == id).map_or_else(|| id.clone(), |(_, name)| name.clone()),
    };
    let mut picked: Option<Option<String>> = None;
    ui.set_next_item_width(-80.0);
    if let Some(_token) = ui.begin_combo("##camera_device", &current) {
        if ui.selectable_config("Default").selected(state.camera_device.is_none()).build() {
            picked = Some(None);
        }
        for (id, name) in &state.cameras {
            if ui.selectable_config(format!("{name}##{id}")).selected(state.camera_device.as_deref() == Some(id.as_str())).build() {
                picked = Some(Some(id.clone()));
            }
        }
    }
    ui.same_line();
    if ui.button("Refresh##cameras") {
        let _ = cmd_tx.send(UiCommand::LoadCameras);
    }
    if state.cameras_loaded && state.cameras.is_empty() {
        ui.text_disabled("No cameras found.");
    }
    if let Some(device) = picked {
        state.camera_device = device.clone();
        persist_settings(state);
        let _ = cmd_tx.send(UiCommand::SetCameraDevice { device });
    }
    ui.text_disabled("A new camera applies the next time you turn your camera on.");
}

fn draw_settings_keybinds(ui: &Ui, state: &mut AppState) {
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
}

fn draw_settings_about(ui: &Ui, state: &mut AppState, textures: &TextureCache) {
    use crate::build_info;

    const LOGO_SIZE: f32 = 44.0;
    const LABEL_COLUMN: f32 = 130.0;

    let row_top = ui.cursor_pos()[1];
    draw_icon(ui, textures, CHROME_ICONS[1], LOGO_SIZE, ui.style_color(imgui::StyleColor::Button));
    ui.same_line_with_spacing(0.0, 14.0);
    let text_block = ui.text_line_height() * 2.0 + ui.clone_style().item_spacing[1];
    ui.set_cursor_pos([ui.cursor_pos()[0], row_top + (LOGO_SIZE - text_block) / 2.0]);
    ui.group(|| {
        ui.set_window_font_scale(1.3);
        ui.text(build_info::NAME);
        ui.set_window_font_scale(1.0);
        ui.text_disabled(build_info::version_line());
    });
    ui.set_cursor_pos([ui.cursor_pos()[0], row_top + LOGO_SIZE]);

    ui.dummy([0.0, 10.0]);
    ui.text_wrapped(build_info::DESCRIPTION);
    ui.dummy([0.0, 10.0]);
    ui.separator();
    ui.dummy([0.0, 6.0]);

    let row = |label: &str, value: &str, copyable: bool| {
        let y = ui.cursor_pos()[1];
        ui.text_disabled(label);
        ui.set_cursor_pos([LABEL_COLUMN, y]);
        ui.text(value);
        if copyable {
            ui.same_line_with_spacing(0.0, 8.0);
            if ui.small_button(format!("Copy##copy_{label}")) {
                crate::clipboard::write_text(value);
            }
        }
    };

    row("Version", &format!("v{}", build_info::VERSION), false);
    match build_info::tag() {
        Some(tag) => row("Tag", tag, false),
        None => row("Tag", "untagged build", false),
    }
    match build_info::commit() {
        Some(commit) => row("Commit", commit, true),
        None => row("Commit", "unknown", false),
    }
    if let Some(date) = build_info::commit_date() {
        row("Committed", date, false);
    }
    row("Video playback", if cfg!(feature = "video") { "enabled" } else { "not built in" }, false);

    let identity = rustcord_client::identity::ClientIdentity::current();
    let runtime = identity.runtime();
    ui.dummy([0.0, 10.0]);
    ui.separator();
    ui.dummy([0.0, 6.0]);
    ui.text("Client identity");
    ui.dummy([0.0, 4.0]);
    row("User agent", identity.user_agent(), true);
    row("OS", &format!("{} ({})", runtime.os, runtime.arch), false);
    row("OS version", runtime.os_version.as_deref().unwrap_or("not detected"), false);
    row("Locale", runtime.locale.as_deref().unwrap_or("not detected"), false);
    let properties = identity.super_properties_json();
    let y = ui.cursor_pos()[1];
    ui.text_disabled("Properties");
    ui.set_cursor_pos([LABEL_COLUMN, y]);
    ui.text_wrapped(&properties);
    if ui.small_button("Copy##copy_properties") {
        crate::clipboard::write_text(&properties);
    }

    ui.dummy([0.0, 10.0]);
    ui.separator();
    ui.dummy([0.0, 6.0]);
    if ui.button("Open developer tools") {
        state.devtools_requested = true;
    }
    ui.same_line();
    ui.text_disabled("Ctrl+Shift+I");
    ui.text_disabled("Logs, API requests and gateway/voice socket frames. Tokens and passwords are redacted.");
}

fn draw_gradient_editor(ui: &Ui, theme: &mut crate::theme::Theme) {
    use crate::theme::GradientRatio;
    if ui.checkbox("Use global gradient", &mut theme.use_global_gradient) && theme.use_global_gradient {
        theme.global_gradient_start.get_or_insert(theme.background);
        theme.global_gradient_end.get_or_insert(theme.panel);
    }
    if !theme.use_global_gradient {
        return;
    }
    let mut start = theme.global_gradient_start.unwrap_or(theme.background);
    if ui.color_edit4("Gradient start", &mut start) {
        theme.global_gradient_start = Some(start);
    }
    let mut end = theme.global_gradient_end.unwrap_or(theme.panel);
    if ui.color_edit4("Gradient end", &mut end) {
        theme.global_gradient_end = Some(end);
    }
    let mut angle = theme.global_gradient_angle.unwrap_or(crate::theme::DEFAULT_GRADIENT_ANGLE);
    if ui.slider_config("Angle", 0.0, 360.0).display_format("%.0f°").build(&mut angle) {
        theme.global_gradient_angle = Some(angle);
    }
    ui.same_line();
    ui.text_disabled("0° = to top, 90° = to right, 180° = to bottom");
    let mut stops = match theme.global_gradient_ratio {
        Some(GradientRatio::Stops(stops)) => stops,
        Some(GradientRatio::Midpoint(m)) => [m, m],
        None => [0.0, 1.0],
    };
    let mut midpoint = matches!(theme.global_gradient_ratio, Some(GradientRatio::Midpoint(_)));
    if ui.checkbox("Single midpoint", &mut midpoint) {
        theme.global_gradient_ratio = Some(if midpoint { GradientRatio::Midpoint(0.5) } else { GradientRatio::Stops([0.0, 1.0]) });
    }
    match theme.global_gradient_ratio {
        Some(GradientRatio::Midpoint(mut m)) => {
            if ui.slider_config("Midpoint", 0.01, 0.99).display_format("%.2f").build(&mut m) {
                theme.global_gradient_ratio = Some(GradientRatio::Midpoint(m));
            }
            ui.text_disabled("Where the two colours are mixed 50/50 along the gradient.");
        }
        _ => {
            let changed_start = ui.slider_config("Blend starts", 0.0, 1.0).display_format("%.2f").build(&mut stops[0]);
            let changed_end = ui.slider_config("Blend ends", 0.0, 1.0).display_format("%.2f").build(&mut stops[1]);
            if changed_start || changed_end {
                stops[1] = stops[1].max(stops[0]);
                theme.global_gradient_ratio = Some(GradientRatio::Stops(stops));
            }
            ui.text_disabled("The start colour is solid before the first stop, the end colour after the second.");
        }
    }
}

fn draw_settings_appearance(ui: &Ui, state: &mut AppState) {
    let clock_options = [(None, "System default"), (Some(false), "12-hour"), (Some(true), "24-hour")];
    let mut clock_idx = clock_options.iter().position(|(value, _)| *value == state.clock_24h).unwrap_or(0);
    ui.set_next_item_width(200.0);
    if ui.combo("Clock", &mut clock_idx, &clock_options, |(_, label)| std::borrow::Cow::Borrowed(*label)) {
        state.clock_24h = clock_options[clock_idx].0;
        crate::settings::save(&crate::settings::Settings {
            theme_file: current_theme_file(&state.theme),
            input_device: state.selected_input_device.clone(),
            output_device: state.selected_output_device.clone(),
            ..state.audio_settings()
        });
    }
    ui.separator();
    let theme_paths = crate::theme::Theme::list_available();
    if theme_paths.is_empty() {
        ui.text_disabled("No themes are available.");
    }
    for path in &theme_paths {
        let label = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let is_current = label == state.theme.name;
        let Ok(theme) = crate::theme::Theme::load_from_file(path) else { continue };

        let swatch_size = [28.0, 20.0];
        let swatch_pos = ui.cursor_screen_pos();
        match theme.global_gradient() {
            Some(gradient) => {
                gradient.draw(&ui.get_window_draw_list(), swatch_pos, [swatch_pos[0] + swatch_size[0], swatch_pos[1] + swatch_size[1]]);
            }
            None => {
                ui.get_window_draw_list()
                    .add_rect(swatch_pos, [swatch_pos[0] + swatch_size[0], swatch_pos[1] + swatch_size[1]], theme.accent)
                    .filled(true)
                    .build();
            }
        }
        ui.dummy(swatch_size);
        ui.same_line();

        if ui.selectable_config(&label).selected(is_current).build() {
            state.theme = theme;
            let theme_file = path.file_name().map(|name| name.to_string_lossy().into_owned());
            crate::settings::save(&crate::settings::Settings { theme_file, input_device: state.selected_input_device.clone(), output_device: state.selected_output_device.clone(), ..state.audio_settings() });
        }
    }

    ui.separator();
    ui.text("Create or edit a theme");
    ui.text_disabled("Background colors the app when no gradient is active. Panel colors inputs, popups, and raised surfaces.");
    ui.input_text("Theme name", &mut state.theme_name_input).hint("My theme").build();
    for (label, color) in [
        ("Background", &mut state.theme.background),
        ("Panel", &mut state.theme.panel),
        ("Text", &mut state.theme.text),
        ("Muted text", &mut state.theme.text_muted),
        ("Accent", &mut state.theme.accent),
        ("Accent hover", &mut state.theme.accent_hover),
        ("Border", &mut state.theme.border),
    ] {
        ui.color_edit4(label, color);
    }
    ui.separator();
    ui.text("Global gradient");
    ui.text_disabled("When enabled, this gradient fills the app background across all panes.");
    draw_gradient_editor(ui, &mut state.theme);
    if ui.button("Save theme as TOML") {
        let name = state.theme_name_input.trim();
        let slug: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if name.is_empty() || slug.is_empty() {
            state.theme_save_status = Some("Enter a name using letters or numbers.".into());
        } else if let Some(dir) = crate::theme::Theme::themes_dir() {
            let path = dir.join(format!("{slug}.toml"));
            let mut theme = state.theme.clone();
            theme.name = name.to_string();
            let result = std::fs::create_dir_all(&dir)
                .map_err(|error| error.to_string())
                .and_then(|_| theme.save_to_file(&path).map_err(|error| error.to_string()));
            state.theme_save_status = match result {
                Ok(()) => {
                    state.theme = theme;
                    crate::settings::save(&crate::settings::Settings {
                        theme_file: path.file_name().map(|file| file.to_string_lossy().into_owned()),
                        input_device: state.selected_input_device.clone(),
                        output_device: state.selected_output_device.clone(),
                        ..state.audio_settings()
                    });
                    state.theme_name_input.clear();
                    Some(format!("Saved to {}", path.display()))
                }
                Err(error) => Some(format!("Could not save theme: {error}")),
            };
        } else {
            state.theme_save_status = Some("Could not find the themes directory.".into());
        }
    }
    if let Some(status) = &state.theme_save_status {
        ui.text_colored(state.theme.text_muted, status);
    }
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

#[cfg(test)]
mod link_tests {
    use super::*;

    fn people() -> Vec<(String, String)> {
        vec![
            ("1".into(), "Ada Lovelace".into()),
            ("2".into(), "Grace Hopper".into()),
            ("3".into(), "ada_fan".into()),
        ]
    }

    #[test]
    fn an_empty_author_filter_lists_everyone() {
        assert_eq!(super::author_filter_matches(&people(), "").len(), 3);
    }

    #[test]
    fn the_author_filter_matches_anywhere_in_the_name() {
        let hits = super::author_filter_matches(&people(), "lovelace");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "1");
    }

    #[test]
    fn the_author_filter_ignores_case_and_spans_entries() {
        let hits = super::author_filter_matches(&people(), "ADA");
        assert_eq!(hits.len(), 2, "{hits:?}");
    }

    #[test]
    fn the_author_filter_trims_surrounding_whitespace() {
        assert_eq!(super::author_filter_matches(&people(), "  grace  ").len(), 1);
    }

    #[test]
    fn an_author_filter_matching_no_one_is_empty_rather_than_everyone() {
        assert!(super::author_filter_matches(&people(), "zzz").is_empty());
    }

    #[test]
    fn the_author_list_is_capped() {
        let many: Vec<(String, String)> =
            (0..200).map(|i| (i.to_string(), format!("user{i}"))).collect();
        assert_eq!(super::author_filter_matches(&many, "").len(), super::AUTHOR_PICKER_MAX_RESULTS);
    }

    #[test]
    fn the_first_has_option_means_no_filter() {
        assert_eq!(super::SEARCH_HAS_OPTIONS[0].1, None);
    }

    #[test]
    fn every_has_option_after_the_first_sends_a_value() {
        for (label, value) in &super::SEARCH_HAS_OPTIONS[1..] {
            assert!(value.is_some(), "{label} has no query value");
        }
    }

    #[test]
    fn has_option_values_are_lowercase_api_tokens() {
        for (_, value) in super::SEARCH_HAS_OPTIONS.iter().filter_map(|(l, v)| v.map(|v| (l, v))) {
            assert_eq!(value, value.to_lowercase(), "{value} must be lowercase for the API");
        }
    }

    #[test]
    fn the_jump_highlight_is_translucent_so_text_stays_readable() {
        let highlight = super::jump_highlight([0.35, 0.40, 0.95, 1.0]);
        assert_eq!(highlight[3], 0.22);
        assert_eq!(&highlight[..3], &[0.35, 0.40, 0.95]);
    }

    #[test]
    fn a_dm_result_jumps_to_a_dm() {
        let target = super::jump_target(None, "c".into(), None);
        assert_eq!(target, crate::switcher::Target::Dm { channel_id: "c".into() });
    }

    #[test]
    fn a_guild_result_jumps_to_its_own_guild() {
        let target = super::jump_target(Some("g1".into()), "c".into(), Some("g2".into()));
        assert_eq!(target, crate::switcher::Target::Channel { guild_id: "g1".into(), channel_id: "c".into() });
    }

    #[test]
    fn a_guild_result_missing_its_guild_id_falls_back_to_the_search_scope() {
        let target = super::jump_target(None, "c".into(), Some("g".into()));
        assert_eq!(target, crate::switcher::Target::Channel { guild_id: "g".into(), channel_id: "c".into() });
    }

    #[test]
    fn a_reaction_pill_hugs_its_contents() {
        let count_w = 7.0;
        let w = reaction_pill_width(count_w);
        let content = REACTION_EMOJI_SIZE + REACTION_PILL_GAP + count_w;
        assert_eq!(w - content, REACTION_PILL_PAD_X * 2.0, "padding should be the only slack");
        assert!(w < REACTION_EMOJI_SIZE * 3.0, "pill is too wide for one emoji and a digit: {w}");
    }

    #[test]
    fn the_emoji_is_the_largest_part_of_the_pill() {
        let w = reaction_pill_width(7.0);
        assert!(REACTION_EMOJI_SIZE / w > 0.45, "emoji should dominate the pill, got {}", REACTION_EMOJI_SIZE / w);
    }

    #[test]
    fn a_unicode_glyph_is_its_own_reaction_path() {
        assert_eq!(reaction_emoji_path("\u{1F600}"), "\u{1F600}");
    }

    #[test]
    fn a_custom_emoji_tag_becomes_name_colon_id() {
        assert_eq!(reaction_emoji_path("<:smile:12345>"), "smile:12345");
        assert_eq!(reaction_emoji_path("<a:dance:67890>"), "dance:67890");
    }

    #[test]
    fn a_malformed_tag_is_passed_through_unchanged() {
        for src in ["<:smile>", "<>", "<::>", "plain"] {
            assert_eq!(reaction_emoji_path(src), src, "{src}");
        }
    }

    fn fixed_width(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    #[test]
    fn text_that_fits_is_not_truncated() {
        assert_eq!(truncate_measured("abc", 100.0, fixed_width), None);
        assert_eq!(truncate_measured("abc", 30.0, fixed_width), None);
    }

    #[test]
    fn long_text_is_cut_and_marked_with_an_ellipsis() {
        let out = truncate_measured("mercury doing mercury things", 100.0, fixed_width).expect("truncated");
        assert!(out.ends_with('\u{2026}'), "{out}");
        assert!(fixed_width(&out) <= 100.0, "still too wide: {out}");
    }

    #[test]
    fn truncation_is_char_safe_on_multibyte_text() {
        let out = truncate_measured(&"\u{e9}".repeat(40), 100.0, fixed_width).expect("truncated");
        assert!(out.ends_with('\u{2026}'), "{out}");
        assert!(fixed_width(&out) <= 100.0);
    }

    #[test]
    fn an_impossibly_narrow_column_yields_just_the_ellipsis() {
        assert_eq!(truncate_measured("abcdef", 5.0, fixed_width), Some("\u{2026}".into()));
    }

    #[test]
    fn a_short_url_loses_only_its_scheme() {
        assert_eq!(elide_url("https://example.com/a"), "example.com/a");
        assert_eq!(elide_url("http://example.com"), "example.com");
    }

    #[test]
    fn a_long_url_is_elided_in_the_middle_keeping_host_and_tail() {
        let url = format!("https://cdn.example.com/attachments/{}/file_name.png", "9".repeat(120));
        let out = elide_url(&url);
        assert!(out.chars().count() < 80, "still long: {out} ({})", out.chars().count());
        assert!(out.starts_with("cdn.example.com/"), "lost the host: {out}");
        assert!(out.ends_with("file_name.png"), "lost the tail: {out}");
        assert!(out.contains('\u{2026}'), "no ellipsis: {out}");
    }

    #[test]
    fn eliding_is_char_safe_on_multibyte_urls() {
        let url = format!("https://example.com/{}", "\u{e9}".repeat(200));
        let out = elide_url(&url);
        assert!(out.chars().count() < 80, "{out}");
    }

    #[test]
    fn relative_time_reads_naturally_in_both_directions() {
        let now = chrono::Utc::now().timestamp();
        assert_eq!(format_relative(now - 3600), "1 hour ago");
        assert_eq!(format_relative(now + 7200), "in 2 hours");
        assert_eq!(format_relative(now - 86_400 * 3), "3 days ago");
        assert_eq!(format_relative(now), "1 second ago");
    }

    #[test]
    fn a_timestamp_style_never_renders_as_an_empty_label() {
        use markdown::TimestampStyle::*;
        for style in [ShortTime, LongTime, ShortDate, LongDate, ShortDateTime, LongDateTime, Relative] {
            let out = format_discord_timestamp(1_730_000_000, style);
            assert!(!out.trim().is_empty(), "{style:?} rendered empty");
            assert!(!out.contains("invalid"), "{style:?} -> {out}");
        }
    }
}

fn toggle_search_panel(state: &mut AppState) {
    state.search_open = !state.search_open;
    if state.search_open {
        state.search_focus_input = true;
        state.show_members_panel = false;
    }
}

fn format_result_timestamp(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%d %b %Y, %H:%M").to_string())
        .unwrap_or_else(|_| "unknown date".to_string())
}

fn search_scope_for(state: &AppState) -> Option<(crate::backend::SearchScope, String)> {
    use crate::backend::SearchScope;
    if let Some(guild_id) = &state.selected_guild {
        let name = state
            .guilds
            .iter()
            .find(|g| &g.id == guild_id)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "this server".to_string());
        return Some((SearchScope::Guild { guild_id: guild_id.clone() }, name));
    }
    let channel_id = state.selected_channel.clone()?;
    let name = state
        .dm_channels
        .iter()
        .find(|c| c.id == channel_id)
        .map(AppState::dm_display_name)
        .unwrap_or_else(|| "this conversation".to_string());
    Some((SearchScope::Channel { channel_id }, name))
}

const AUTHOR_PICKER_MAX_RESULTS: usize = 50;

const SEARCH_HAS_OPTIONS: [(&str, Option<&str>); 6] = [
    ("Anything", None),
    ("Image", Some("image")),
    ("Video", Some("video")),
    ("File", Some("file")),
    ("Link", Some("link")),
    ("Embed", Some("embed")),
];

fn current_search_filters(state: &AppState) -> crate::backend::SearchFilters {
    crate::backend::SearchFilters {
        author_id: state.search_author.clone(),
        has: SEARCH_HAS_OPTIONS.get(state.search_has).and_then(|(_, v)| v.map(str::to_string)),
    }
}

fn author_filter_matches(authors: &[(String, String)], needle: &str) -> Vec<(String, String)> {
    let needle = needle.trim().to_lowercase();
    authors
        .iter()
        .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
        .take(AUTHOR_PICKER_MAX_RESULTS)
        .cloned()
        .collect()
}

fn submit_search(state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, offset: u32) {
    let query = state.search_input.trim().to_string();
    let filters = current_search_filters(state);
    if query.is_empty() && filters == Default::default() {
        state.search = None;
        return;
    }
    let Some((scope, _)) = search_scope_for(state) else { return };
    let _ = cmd_tx.send(UiCommand::SearchMessages {
        scope: scope.clone(),
        query: query.clone(),
        filters: filters.clone(),
        offset,
    });
    let carried_total = state
        .search
        .as_ref()
        .filter(|s| s.query == query && s.scope == scope && s.filters == filters)
        .map(|s| s.total)
        .unwrap_or(0);
    state.search = Some(crate::app_state::SearchState {
        scope,
        query,
        filters,
        offset,
        total: carried_total,
        results: Vec::new(),
        status: crate::app_state::SearchStatus::Loading,
    });
}

fn draw_search_panel(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    use crate::app_state::SearchStatus;

    let close_width = ui.calc_text_size("Close")[0] + 20.0;
    ui.text("Search");
    ui.same_line_with_pos(SEARCH_PANEL_WIDTH - close_width - 12.0);
    if ui.button("Close") {
        state.search_open = false;
        return;
    }

    if state.search_focus_input {
        ui.set_keyboard_focus_here();
        state.search_focus_input = false;
    }
    ui.set_next_item_width(SEARCH_PANEL_WIDTH - 16.0);
    let submitted = ui
        .input_text("##search_input", &mut state.search_input)
        .hint("Search messages...")
        .enter_returns_true(true)
        .build();
    if submitted {
        submit_search(state, cmd_tx, 0);
    }

    let scope_guild_id = match search_scope_for(state) {
        Some((crate::backend::SearchScope::Guild { guild_id }, _)) => Some(guild_id),
        _ => None,
    };
    let authors = state.searchable_authors(scope_guild_id.as_deref());
    let selected_author_name = state
        .search_author
        .as_ref()
        .and_then(|id| authors.iter().find(|(aid, _)| aid == id).map(|(_, name)| name.clone()));

    let half = (ui.content_region_avail()[0] - 8.0) / 2.0;
    let mut filters_changed = false;

    ui.set_next_item_width(half);
    let author_label = selected_author_name.clone().unwrap_or_else(|| "Anyone".to_string());
    if let Some(_combo) = ui.begin_combo("##search_author", &author_label) {
        ui.set_next_item_width(-1.0);
        ui.input_text("##search_author_filter", &mut state.search_author_input)
            .hint("Filter people...")
            .build();
        if ui.selectable_config("Anyone").selected(state.search_author.is_none()).build() {
            state.search_author = None;
            state.search_author_input.clear();
            filters_changed = true;
        }
        let matches = author_filter_matches(&authors, &state.search_author_input);
        if matches.is_empty() {
            ui.text_disabled("No one matches");
        }
        for (id, name) in &matches {
            let selected = state.search_author.as_deref() == Some(id.as_str());
            if ui.selectable_config(format!("{name}##author_{id}")).selected(selected).build() {
                state.search_author = Some(id.clone());
                state.search_author_input.clear();
                filters_changed = true;
            }
        }
    }

    ui.same_line();
    ui.set_next_item_width(half);
    let current = SEARCH_HAS_OPTIONS[state.search_has.min(SEARCH_HAS_OPTIONS.len() - 1)].0;
    if let Some(_combo) = ui.begin_combo("##search_has", current) {
        for (idx, (label, _)) in SEARCH_HAS_OPTIONS.iter().enumerate() {
            if ui.selectable_config(label).selected(idx == state.search_has).build() {
                state.search_has = idx;
                filters_changed = true;
            }
        }
    }

    if scope_guild_id.is_some() && authors.len() <= 1 {
        if let Some(guild_id) = scope_guild_id.clone() {
            if state.guild_members_requested.insert(guild_id.clone()) {
                let _ = cmd_tx.send(UiCommand::RequestGuildMembers { guild_id });
            }
        }
    }

    if filters_changed {
        submit_search(state, cmd_tx, 0);
    }

    if let Some((_, label)) = search_scope_for(state) {
        ui.text_colored(state.theme.text_muted, format!("in {label}"));
    }
    ui.separator();

    if state.search_follows_selection {
        let current_scope = search_scope_for(state).map(|(scope, _)| scope);
        if state.search.as_ref().is_some_and(|s| Some(&s.scope) != current_scope.as_ref()) {
            state.search = None;
        }
    }
    state.search_follows_selection = true;

    let Some(search) = &state.search else {
        ui.text_disabled("Enter a term to search this server or conversation.");
        return;
    };

    match &search.status {
        SearchStatus::Loading => {
            ui.text_disabled("Searching...");
            return;
        }
        SearchStatus::Indexing { retry_after } => {
            ui.text_colored(state.theme.text_muted, "Discord is still indexing this server.");
            ui.text_colored(state.theme.text_muted, format!("Try again in {retry_after:.0}s."));
            if ui.button("Retry") {
                submit_search(state, cmd_tx, 0);
            }
            return;
        }
        SearchStatus::Failed(message) => {
            ui.text_colored(state.theme.text_muted, "Search failed:");
            ui.text_wrapped(message);
            return;
        }
        SearchStatus::Ready => {}
    }

    if search.results.is_empty() {
        ui.text_disabled("No results.");
        return;
    }

    let (total, page, pages, has_next) = (search.total, search.page(), search.pages(), search.has_next_page());
    ui.text_colored(state.theme.text_muted, format!("{total} results — page {} of {pages}", page + 1));

    let footer = ui.frame_height() + 12.0;
    let list_height = (ui.content_region_avail()[1] - footer).max(80.0);
    let mut jump_to: Option<(Option<String>, String, String)> = None;
    let scope_guild = match &search.scope {
        crate::backend::SearchScope::Guild { guild_id } => Some(guild_id.clone()),
        crate::backend::SearchScope::Channel { .. } => None,
    };

    ui.child_window("##search_results").size([0.0, list_height]).build(|| {
        let channel_names = state.channel_name_map();
        for result in &search.results {
            let author = result.author.global_name.clone().unwrap_or_else(|| result.author.username.clone());
            let channel_label = channel_names
                .get(&result.channel_id)
                .map(|n| format!("#{n}"))
                .unwrap_or_else(|| "direct message".to_string());

            let token = ui.push_style_color(imgui::StyleColor::Text, state.theme.text_muted);
            ui.set_window_font_scale(0.85);
            ui.text(format!("{channel_label} — {}", format_result_timestamp(&result.timestamp)));
            ui.set_window_font_scale(1.0);
            token.pop();

            ui.text_colored(state.theme.accent, &author);
            if result.content.is_empty() {
                ui.text_disabled("(no text content)");
            } else {
                ui.text_wrapped(&result.content);
            }

            if ui.button(format!("Jump##jump_{}", result.id)) {
                jump_to = Some((result.guild_id.clone(), result.channel_id.clone(), result.id.clone()));
            }
            ui.separator();
        }
    });

    let offset = search.offset;
    if page > 0 {
        if ui.button("< Previous") {
            submit_search(state, cmd_tx, offset.saturating_sub(rustcord_client::rest::SEARCH_PAGE_SIZE));
        }
        ui.same_line();
    }
    if has_next && ui.button("Next >") {
        submit_search(state, cmd_tx, offset + rustcord_client::rest::SEARCH_PAGE_SIZE);
    }

    if let Some((guild_id, channel_id, message_id)) = jump_to {
        apply_switcher_target(state, cmd_tx, jump_target(guild_id, channel_id.clone(), scope_guild));
        let _ = cmd_tx.send(UiCommand::JumpToMessage { channel_id, message_id });
        state.search_follows_selection = false;
    }
}

fn jump_target(
    result_guild_id: Option<String>,
    channel_id: String,
    scope_guild_id: Option<String>,
) -> crate::switcher::Target {
    match result_guild_id.or(scope_guild_id) {
        Some(guild_id) => crate::switcher::Target::Channel { guild_id, channel_id },
        None => crate::switcher::Target::Dm { channel_id },
    }
}

#[cfg(test)]
mod line_break_tests {
    use super::line_break;

    fn chars(text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn text_that_fits_is_taken_whole() {
        assert_eq!(line_break("hello world", 20.0, false, chars), Some(11));
    }

    #[test]
    fn a_long_run_breaks_at_the_last_space_that_fits() {
        assert_eq!(line_break("one two three", 8.0, true, chars), Some(7));
    }

    #[test]
    fn mid_line_a_first_word_that_does_not_fit_moves_to_the_next_line() {
        assert_eq!(line_break("Dopo averlo", 2.0, false, chars), None);
    }

    #[test]
    fn a_word_wider_than_the_line_is_split_only_at_line_start() {
        assert_eq!(line_break("abcdefgh", 3.0, true, chars), Some(3));
        assert_eq!(line_break("abcdefgh", 0.0, true, chars), Some(1));
    }

    #[test]
    fn leading_whitespace_is_not_a_break_point() {
        assert_eq!(line_break(" abc def", 4.0, false, chars), Some(4));
    }
}

#[cfg(test)]
mod signal_tests {
    use super::signal_quality;

    #[test]
    fn ping_maps_to_green_yellow_red() {
        let green = signal_quality(Some(40.0));
        assert_eq!(signal_quality(Some(120.0)), green);
        assert_ne!(signal_quality(Some(200.0)), green);
        assert_ne!(signal_quality(Some(400.0)), signal_quality(Some(200.0)));
        assert_ne!(signal_quality(None), green);
    }
}

#[cfg(test)]
mod cover_uv_tests {
    use super::cover_uv;

    #[test]
    fn a_wide_source_is_cropped_at_the_sides() {
        let (min, max) = cover_uv([1280.0, 720.0], [100.0, 100.0]);
        assert!((min[0] - 0.21875).abs() < 1e-4 && (max[0] - 0.78125).abs() < 1e-4, "{min:?} {max:?}");
        assert_eq!((min[1], max[1]), (0.0, 1.0));
    }

    #[test]
    fn a_tall_source_is_cropped_top_and_bottom() {
        let (min, max) = cover_uv([100.0, 200.0], [200.0, 100.0]);
        assert_eq!((min[0], max[0]), (0.0, 1.0));
        assert!((min[1] - 0.375).abs() < 1e-4 && (max[1] - 0.625).abs() < 1e-4, "{min:?} {max:?}");
    }

    #[test]
    fn matching_aspects_use_the_whole_image() {
        assert_eq!(cover_uv([1280.0, 720.0], [640.0, 360.0]), ([0.0, 0.0], [1.0, 1.0]));
    }
}

#[cfg(test)]
mod mosaic_tests {
    use super::mosaic_rows;

    #[test]
    fn rows_follow_discords_layouts() {
        let layouts: Vec<Vec<usize>> = (2..=10).map(mosaic_rows).collect();
        assert_eq!(
            layouts,
            [
                vec![2],
                vec![1, 2],
                vec![2, 2],
                vec![2, 3],
                vec![3, 3],
                vec![1, 3, 3],
                vec![2, 3, 3],
                vec![3, 3, 3],
                vec![1, 3, 3, 3],
            ]
        );
    }

    #[test]
    fn every_image_gets_exactly_one_cell() {
        for count in 0..=10 {
            assert_eq!(mosaic_rows(count).iter().sum::<usize>(), count);
        }
    }
}

#[cfg(test)]
mod voice_message_ui_tests {
    use super::{format_clip_time, waveform_bar_count};

    #[test]
    fn clip_times_read_as_minutes_and_seconds() {
        assert_eq!(format_clip_time(0.0), "0:00");
        assert_eq!(format_clip_time(4.6), "0:05");
        assert_eq!(format_clip_time(125.0), "2:05");
        assert_eq!(format_clip_time(-3.0), "0:00");
    }

    #[test]
    fn waveform_bars_fill_the_width_without_a_trailing_gap() {
        assert_eq!(waveform_bar_count(0.0), 0);
        assert_eq!(waveform_bar_count(3.0), 1);
        assert_eq!(waveform_bar_count(8.0), 2);
        assert_eq!(waveform_bar_count(7.9), 1);
    }
}

#[cfg(test)]
mod poll_ui_tests {
    use super::{poll_percent, poll_time_left};

    fn at(text: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&chrono::Utc)
    }

    #[test]
    fn time_left_reads_like_discord() {
        let now = at("2026-11-08T12:00:00Z");
        let left = |end: &str| poll_time_left(Some(at(end)), false, now);
        assert_eq!(left("2026-11-08T12:00:30Z"), "Less than a minute left");
        assert_eq!(left("2026-11-08T12:45:00Z"), "45m left");
        assert_eq!(left("2026-11-08T17:10:00Z"), "5h left");
        assert_eq!(left("2026-11-10T12:00:00Z"), "2d left");
        assert_eq!(left("2026-11-10T15:00:00Z"), "2d 3h left");
        assert_eq!(poll_time_left(Some(at("2026-11-10T15:00:00Z")), true, now), "Poll closed");
        assert_eq!(poll_time_left(None, false, now), "Poll closed");
    }

    #[test]
    fn percentages_round_and_survive_zero_votes() {
        assert_eq!(poll_percent(1, 3), 33);
        assert_eq!(poll_percent(2, 3), 67);
        assert_eq!(poll_percent(0, 0), 0);
    }
}
