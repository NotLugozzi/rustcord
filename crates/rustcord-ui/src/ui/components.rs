use super::{draw_icon_at, draw_markdown, emoji_texture, faces, mix, truncate_to_width, EmojiTexture, Fonts, MentionCtx};
use crate::app_state::AppState;
use crate::backend::UiCommand;
use crate::images::TextureCache;
use imgui::Ui;
use rustcord_client::interactions::{self, MessageTarget, ModalAnswer};
use rustcord_client::models::{button_style, component_kind as kind, Component, ComponentEmoji, Message, MESSAGE_FLAG_EPHEMERAL, MESSAGE_FLAG_LOADING};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::UnboundedSender;

const BUTTON_HEIGHT: f32 = 32.0;
const BUTTON_PAD: f32 = 16.0;
const EMOJI_SIZE: f32 = 18.0;
const SELECT_HEIGHT: f32 = 40.0;
const SELECT_MAX_WIDTH: f32 = 400.0;
const GALLERY_MAX: [f32; 2] = [320.0, 240.0];
const THUMBNAIL_SIZE: f32 = 72.0;

pub(super) enum ComponentAction {
    Click { custom_id: String },
    Pick { custom_id: String, kind: u8, values: Vec<String> },
}

pub(super) struct ComponentCtx<'a, 'm> {
    pub fonts: &'a Fonts,
    pub theme: &'a crate::theme::Theme,
    pub textures: &'a mut TextureCache,
    pub cmd_tx: &'a UnboundedSender<UiCommand>,
    pub busy: bool,
    pub drafts: &'a mut HashMap<(String, String), Vec<String>>,
    pub revealed_spoilers: &'a mut HashSet<usize>,
    pub mentions: &'a mut MentionCtx<'m>,
}

fn button_fill(theme: &crate::theme::Theme, style: Option<u8>) -> [f32; 4] {
    match style {
        Some(button_style::PRIMARY | button_style::PREMIUM) => theme.accent,
        Some(button_style::SUCCESS) => theme.call_active(),
        Some(button_style::DANGER) => theme.danger(),
        _ => mix(theme.field(), theme.text, 0.12),
    }
}

fn button_ink(theme: &crate::theme::Theme, style: Option<u8>) -> [f32; 4] {
    match style {
        Some(button_style::PRIMARY | button_style::PREMIUM | button_style::SUCCESS | button_style::DANGER) => [1.0, 1.0, 1.0, 1.0],
        _ => theme.text,
    }
}

fn with_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], color[3] * alpha]
}

fn draw_component_emoji(ui: &Ui, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, emoji: &ComponentEmoji, min: [f32; 2], size: f32) -> bool {
    let max = [min[0] + size, min[1] + size];
    if let Some(id) = &emoji.id {
        let url = format!("https://cdn.discordapp.com/emojis/{id}.{}?size=48", if emoji.animated { "gif" } else { "png" });
        match textures.get_still(&url) {
            Some((texture, _)) => ui.get_window_draw_list().add_image(texture, min, max).build(),
            None => textures.request(&url, cmd_tx),
        }
        return true;
    }
    let Some(ch) = emoji.name.as_deref().and_then(|name| name.chars().next()) else { return false };
    if let EmojiTexture::Ready(texture) = emoji_texture(textures, ch) {
        ui.get_window_draw_list().add_image(texture, min, max).build();
    }
    true
}

fn component_key(message: &Message, component: &Component, index: &str) -> String {
    format!("{}_{}_{}", message.id, component.custom_id.as_deref().unwrap_or(""), index)
}

fn draw_button(ui: &Ui, ctx: &mut ComponentCtx<'_, '_>, message: &Message, component: &Component, index: &str) -> Option<ComponentAction> {
    let label = component.label.clone().unwrap_or_default();
    let link = matches!(component.style, Some(button_style::LINK | button_style::PREMIUM));
    let suffix = if link { " \u{2197}" } else { "" };
    let text = format!("{label}{suffix}");
    let font = ui.push_font(faces().sm_semibold);
    let text_w = if text.is_empty() { 0.0 } else { ui.calc_text_size(&text)[0] };
    let emoji_w = if component.emoji.is_some() { EMOJI_SIZE + if text.is_empty() { 0.0 } else { 6.0 } } else { 0.0 };
    let size = [BUTTON_PAD * 2.0 + emoji_w + text_w, BUTTON_HEIGHT];
    let min = ui.cursor_screen_pos();
    let disabled = component.disabled || (!link && ctx.busy);
    let pressed = ui.invisible_button(format!("##component_{}", component_key(message, component, index)), size);
    let hovered = ui.is_item_hovered();
    if hovered && !disabled {
        ui.set_mouse_cursor(Some(imgui::MouseCursor::Hand));
    }
    let mut fill = button_fill(ctx.theme, component.style);
    if hovered && !disabled {
        fill = mix(fill, [0.0, 0.0, 0.0, 1.0], 0.12);
    }
    let alpha = if disabled { 0.5 } else { 1.0 };
    let max = [min[0] + size[0], min[1] + size[1]];
    ui.get_window_draw_list().add_rect(min, max, with_alpha(fill, alpha)).filled(true).rounding(4.0).build();
    let mut x = min[0] + BUTTON_PAD;
    if let Some(emoji) = &component.emoji {
        draw_component_emoji(ui, ctx.textures, ctx.cmd_tx, emoji, [x, min[1] + (BUTTON_HEIGHT - EMOJI_SIZE) / 2.0], EMOJI_SIZE);
        x += emoji_w;
    }
    if !text.is_empty() {
        let y = min[1] + (BUTTON_HEIGHT - ui.text_line_height()) / 2.0;
        ui.get_window_draw_list().add_text([x, y], with_alpha(button_ink(ctx.theme, component.style), alpha), &text);
    }
    font.pop();
    if !pressed || disabled {
        return None;
    }
    match component.style {
        Some(button_style::LINK) => {
            if let Some(url) = &component.url {
                let _ = open::that(url);
            }
            None
        }
        Some(button_style::PREMIUM) => {
            if let (Some(app), Some(sku)) = (message.application_id.as_deref().or(Some(message.author.id.as_str())), &component.sku_id) {
                let _ = open::that(format!("https://discord.com/application-directory/{app}/store/{sku}"));
            }
            None
        }
        _ => component.custom_id.clone().map(|custom_id| ComponentAction::Click { custom_id }),
    }
}

fn select_summary(component: &Component, picked: &[String]) -> Option<String> {
    let labels: Vec<&str> = component.options.iter().filter(|o| picked.contains(&o.value)).map(|o| o.label.as_str()).collect();
    (!labels.is_empty()).then(|| labels.join(", "))
}

fn draw_select(ui: &Ui, ctx: &mut ComponentCtx<'_, '_>, message: &Message, component: &Component, index: &str) -> Option<ComponentAction> {
    let key = (message.id.clone(), component_key(message, component, index));
    let supported = component.kind == kind::STRING_SELECT;
    let defaults: Vec<String> = component.options.iter().filter(|o| o.default).map(|o| o.value.clone()).collect();
    let picked = ctx.drafts.get(&key).cloned().unwrap_or_else(|| defaults.clone());
    let disabled = component.disabled || ctx.busy || !supported;
    let width = ui.content_region_avail()[0].min(SELECT_MAX_WIDTH);
    let min = ui.cursor_screen_pos();
    let max = [min[0] + width, min[1] + SELECT_HEIGHT];
    let popup_id = format!("select_popup_{}", key.1);
    let pressed = ui.invisible_button(format!("##select_{}", key.1), [width, SELECT_HEIGHT]);
    let hovered = ui.is_item_hovered();
    if hovered && !supported {
        ui.tooltip_text("This kind of menu isn't supported in rustcord yet");
    }
    if hovered && !disabled {
        ui.set_mouse_cursor(Some(imgui::MouseCursor::Hand));
    }
    let alpha = if disabled { 0.5 } else { 1.0 };
    {
        let draw_list = ui.get_window_draw_list();
        draw_list.add_rect(min, max, with_alpha(ctx.theme.field(), alpha)).filled(true).rounding(4.0).build();
        let edge = if hovered && !disabled { ctx.theme.text_muted } else { ctx.theme.line() };
        draw_list.add_rect(min, max, with_alpha(edge, alpha)).rounding(4.0).build();
    }
    let summary = select_summary(component, &picked);
    let (text, ink) = match &summary {
        Some(text) => (text.clone(), ctx.theme.text),
        None => (component.placeholder.clone().unwrap_or_else(|| "Make a selection".into()), ctx.theme.text_muted),
    };
    let shown = truncate_to_width(ui, &text, width - 48.0).unwrap_or(text);
    ui.get_window_draw_list().add_text([min[0] + 12.0, min[1] + (SELECT_HEIGHT - ui.text_line_height()) / 2.0], with_alpha(ink, alpha), &shown);
    draw_icon_at(ui, ctx.textures, crate::icons::Icon::ChevronDown, [max[0] - 30.0, min[1] + (SELECT_HEIGHT - 18.0) / 2.0], 18.0, with_alpha(ctx.theme.text_muted, alpha));
    if pressed && !disabled {
        ctx.drafts.insert(key.clone(), defaults.clone());
        ui.open_popup(&popup_id);
    }
    let max_values = component.max_values.unwrap_or(1).max(1) as usize;
    let min_values = component.min_values.unwrap_or(1) as usize;
    let mut action = None;
    let mut open = false;
    if let Some(_popup) = ui.begin_popup(&popup_id) {
        open = true;
        let draft = ctx.drafts.entry(key.clone()).or_insert_with(|| defaults.clone());
        ui.set_next_item_width(width);
        for option in &component.options {
            let chosen = draft.contains(&option.value);
            let label = match &option.description {
                Some(description) => format!("{}\n{}##opt_{}", option.label, description, option.value),
                None => format!("{}##opt_{}", option.label, option.value),
            };
            if ui.selectable_config(&label).selected(chosen).size([width - 16.0, 0.0]).build() {
                if max_values == 1 {
                    action = Some(ComponentAction::Pick { custom_id: component.custom_id.clone().unwrap_or_default(), kind: component.kind, values: vec![option.value.clone()] });
                    ui.close_current_popup();
                } else if chosen {
                    draft.retain(|v| v != &option.value);
                } else if draft.len() < max_values {
                    draft.push(option.value.clone());
                }
            }
        }
    }
    if action.is_some() {
        ctx.drafts.remove(&key);
    } else if !open {
        if let Some(draft) = ctx.drafts.remove(&key) {
            let changed = {
                let mut a = draft.clone();
                let mut b = defaults.clone();
                a.sort();
                b.sort();
                a != b
            };
            if max_values > 1 && changed && draft.len() >= min_values {
                action = Some(ComponentAction::Pick { custom_id: component.custom_id.clone().unwrap_or_default(), kind: component.kind, values: draft });
            }
        }
    }
    action
}

fn draw_media(ui: &Ui, ctx: &mut ComponentCtx<'_, '_>, url: &str, bounds: [f32; 2]) {
    match ctx.textures.get(url) {
        Some((texture, natural)) => {
            let scale = (bounds[0] / natural[0].max(1.0)).min(bounds[1] / natural[1].max(1.0)).min(1.0);
            let size = [natural[0] * scale, natural[1] * scale];
            let min = ui.cursor_screen_pos();
            ui.get_window_draw_list().add_image_rounded(texture, min, [min[0] + size[0], min[1] + size[1]], 6.0).build();
            ui.dummy(size);
        }
        None => {
            ctx.textures.request(url, ctx.cmd_tx);
            let min = ui.cursor_screen_pos();
            let size = [bounds[0].min(160.0), bounds[1].min(120.0)];
            ui.get_window_draw_list().add_rect(min, [min[0] + size[0], min[1] + size[1]], ctx.theme.field()).filled(true).rounding(6.0).build();
            ui.dummy(size);
        }
    }
}

fn draw_component(ui: &Ui, ctx: &mut ComponentCtx<'_, '_>, message: &Message, component: &Component, index: &str) -> Option<ComponentAction> {
    let mut action = None;
    let mut take = |next: Option<ComponentAction>| {
        if action.is_none() {
            action = next;
        }
    };
    match component.kind {
        kind::ACTION_ROW => {
            for (i, child) in component.components.iter().enumerate() {
                if i > 0 && child.kind == kind::BUTTON {
                    ui.same_line_with_spacing(0.0, 8.0);
                }
                take(draw_component(ui, ctx, message, child, &format!("{index}.{i}")));
            }
        }
        kind::BUTTON => take(draw_button(ui, ctx, message, component, index)),
        kind::STRING_SELECT | kind::USER_SELECT | kind::ROLE_SELECT | kind::MENTIONABLE_SELECT | kind::CHANNEL_SELECT => {
            take(draw_select(ui, ctx, message, component, index))
        }
        kind::TEXT_DISPLAY => {
            if let Some(content) = &component.content {
                draw_markdown(ui, ctx.fonts, ctx.theme, content, ctx.revealed_spoilers, &format!("{}_{index}", message.id), ctx.textures, ctx.cmd_tx, ctx.mentions);
            }
        }
        kind::SECTION => {
            let start = ui.cursor_pos();
            let accessory_w = match component.accessory.as_deref().map(|a| a.kind) {
                Some(kind::THUMBNAIL) => THUMBNAIL_SIZE + 12.0,
                Some(kind::BUTTON) => 140.0,
                _ => 0.0,
            };
            let text_w = (ui.content_region_avail()[0] - accessory_w).max(120.0);
            ui.group(|| {
                let wrap = ui.push_text_wrap_pos_with_pos(ui.cursor_pos()[0] + text_w);
                for (i, child) in component.components.iter().enumerate() {
                    take(draw_component(ui, ctx, message, child, &format!("{index}.{i}")));
                }
                wrap.end();
            });
            let end_y = ui.cursor_pos()[1];
            if let Some(accessory) = component.accessory.as_deref() {
                ui.set_cursor_pos([start[0] + text_w + 12.0, start[1]]);
                match accessory.kind {
                    kind::THUMBNAIL => {
                        if let Some(media) = &accessory.media {
                            draw_media(ui, ctx, &media.url, [THUMBNAIL_SIZE, THUMBNAIL_SIZE]);
                        }
                    }
                    _ => take(draw_component(ui, ctx, message, accessory, &format!("{index}.a"))),
                }
                ui.set_cursor_pos([start[0], end_y.max(ui.cursor_pos()[1])]);
            }
        }
        kind::CONTAINER => {
            let start = ui.cursor_screen_pos();
            let width = ui.content_region_avail()[0].min(520.0);
            ui.indent_by(14.0);
            ui.dummy([0.0, 4.0]);
            for (i, child) in component.components.iter().enumerate() {
                take(draw_component(ui, ctx, message, child, &format!("{index}.{i}")));
            }
            ui.dummy([0.0, 4.0]);
            ui.unindent_by(14.0);
            let end_y = ui.cursor_screen_pos()[1];
            let bar = match component.accent_color {
                Some(rgb) => [((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, 1.0],
                None => ctx.theme.line(),
            };
            let draw_list = ui.get_window_draw_list();
            draw_list.add_rect(start, [start[0] + width, end_y], ctx.theme.line()).rounding(8.0).build();
            draw_list.add_rect(start, [start[0] + 4.0, end_y], bar).filled(true).rounding(8.0).round_top_right(false).round_bot_right(false).build();
        }
        kind::SEPARATOR => {
            let gap = if component.spacing == Some(2) { 16.0 } else { 8.0 };
            ui.dummy([0.0, gap / 2.0]);
            if component.divider.unwrap_or(true) {
                let y = ui.cursor_screen_pos()[1];
                let x = ui.cursor_screen_pos()[0];
                let width = ui.content_region_avail()[0].min(500.0);
                ui.get_window_draw_list().add_line([x, y], [x + width, y], ctx.theme.line()).build();
            }
            ui.dummy([0.0, gap / 2.0]);
        }
        kind::MEDIA_GALLERY => {
            for (i, item) in component.items.iter().enumerate() {
                if i > 0 {
                    ui.same_line_with_spacing(0.0, 6.0);
                }
                draw_media(ui, ctx, &item.media.url, GALLERY_MAX);
            }
        }
        kind::THUMBNAIL => {
            if let Some(media) = &component.media {
                draw_media(ui, ctx, &media.url, [THUMBNAIL_SIZE, THUMBNAIL_SIZE]);
            }
        }
        kind::FILE => {
            if let Some(file) = &component.file {
                let name = file.url.rsplit('/').next().unwrap_or(&file.url);
                ui.text_colored(ctx.theme.link(), name);
            }
        }
        _ => {}
    }
    action
}

pub(super) fn draw_message_components(ui: &Ui, ctx: &mut ComponentCtx<'_, '_>, message: &Message, error: Option<&str>) -> Option<ComponentAction> {
    let mut action = None;
    if !message.components.is_empty() {
        ui.dummy([0.0, 4.0]);
        for (i, component) in message.components.iter().enumerate() {
            let next = draw_component(ui, ctx, message, component, &i.to_string());
            if action.is_none() {
                action = next;
            }
            if component.kind == kind::ACTION_ROW {
                ui.dummy([0.0, 2.0]);
            }
        }
    }
    let note = |text: &str, color: [f32; 4]| {
        let font = ui.push_font(faces().xs);
        ui.text_colored(color, text);
        font.pop();
    };
    if message.flags & MESSAGE_FLAG_LOADING != 0 && message.content.is_empty() {
        let name = message.author.global_name.clone().unwrap_or_else(|| message.author.username.clone());
        note(&format!("{name} is thinking\u{2026}"), ctx.theme.text_muted);
    }
    if ctx.busy {
        note("Waiting for the app\u{2026}", ctx.theme.text_muted);
    }
    if let Some(error) = error {
        note(error, ctx.theme.danger());
    }
    if message.flags & MESSAGE_FLAG_EPHEMERAL != 0 {
        note("Only you can see this", ctx.theme.text_muted);
    }
    action
}

pub(super) fn dispatch(state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, message: &Message, action: ComponentAction) {
    let guild_id = message.guild_id.clone().or_else(|| state.find_guild_channel(&message.channel_id).and_then(|c| c.guild_id.clone()));
    let target = MessageTarget {
        guild_id,
        channel_id: message.channel_id.clone(),
        message_id: message.id.clone(),
        message_flags: message.flags,
        application_id: message.application_id.clone().unwrap_or_else(|| message.author.id.clone()),
    };
    let nonce = interactions::nonce();
    let body = match &action {
        ComponentAction::Click { custom_id } => interactions::button_click(&target, "", &nonce, custom_id),
        ComponentAction::Pick { custom_id, kind, values } => interactions::select_pick(&target, "", &nonce, custom_id, *kind, values),
    };
    state.start_interaction(nonce, Some(message.id.clone()), body, cmd_tx);
}

fn draw_modal_part(ui: &Ui, theme: &crate::theme::Theme, component: &Component, answers: &mut HashMap<String, ModalAnswer>, show_label: bool) {
    match component.kind {
        kind::TEXT_DISPLAY => {
            if let Some(content) = &component.content {
                ui.text_wrapped(content);
            }
        }
        kind::ACTION_ROW => {
            for child in &component.components {
                draw_modal_part(ui, theme, child, answers, true);
            }
        }
        kind::LABEL => {
            if let Some(label) = &component.label {
                let font = ui.push_font(faces().sm_semibold);
                ui.text(label);
                font.pop();
            }
            if let Some(description) = &component.description {
                ui.text_colored(theme.text_muted, description);
            }
            if let Some(inner) = component.component.as_deref() {
                draw_modal_part(ui, theme, inner, answers, false);
            }
        }
        kind::TEXT_INPUT => {
            let id = component.custom_id.clone().unwrap_or_default();
            if show_label {
                if let Some(label) = &component.label {
                    let font = ui.push_font(faces().sm_semibold);
                    ui.text(label);
                    font.pop();
                }
            }
            let entry = answers.entry(id.clone()).or_insert_with(|| ModalAnswer::Text(component.value.clone().unwrap_or_default()));
            if let ModalAnswer::Text(text) = entry {
                let _id = ui.push_id(id.as_str());
                ui.set_next_item_width(-1.0);
                if component.style == Some(2) {
                    ui.input_text_multiline("##modal_text", text, [-1.0, 110.0]).build();
                } else {
                    let hint = component.placeholder.clone().unwrap_or_default();
                    ui.input_text("##modal_text", text).hint(hint).build();
                }
                if let Some(limit) = component.max_length {
                    if text.chars().count() > limit as usize {
                        *text = text.chars().take(limit as usize).collect();
                    }
                }
            }
            ui.dummy([0.0, 6.0]);
        }
        kind::STRING_SELECT => {
            let id = component.custom_id.clone().unwrap_or_default();
            let max_values = component.max_values.unwrap_or(1).max(1) as usize;
            let entry = answers.entry(id.clone()).or_insert_with(|| ModalAnswer::Picks(component.options.iter().filter(|o| o.default).map(|o| o.value.clone()).collect()));
            if let ModalAnswer::Picks(picks) = entry {
                let _id = ui.push_id(id.as_str());
                for option in &component.options {
                    let mut chosen = picks.contains(&option.value);
                    let label = format!("{}##{}", option.label, option.value);
                    if max_values == 1 {
                        if ui.radio_button_bool(&label, chosen) {
                            *picks = vec![option.value.clone()];
                        }
                    } else if ui.checkbox(&label, &mut chosen) {
                        if chosen && picks.len() < max_values {
                            picks.push(option.value.clone());
                        } else {
                            picks.retain(|v| v != &option.value);
                        }
                    }
                }
            }
            ui.dummy([0.0, 6.0]);
        }
        _ => {}
    }
}

pub(super) fn draw_modal_form(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    let Some(form) = state.modal_form.as_mut() else { return };
    let theme = state.theme.clone();
    let display = ui.io().display_size;
    let mut open = true;
    let mut submit = false;
    let supported = form.modal.is_supported();
    let title = if form.modal.title.is_empty() { "Form".to_string() } else { form.modal.title.clone() };
    ui.window(format!("{title}###bot_modal"))
        .opened(&mut open)
        .size([460.0, 0.0], imgui::Condition::Always)
        .position([display[0] / 2.0, display[1] / 2.0], imgui::Condition::Appearing)
        .position_pivot([0.5, 0.5])
        .collapsible(false)
        .resizable(false)
        .build(|| {
            if let Some(app) = &form.modal.application_name {
                ui.text_colored(theme.text_muted, app);
                ui.dummy([0.0, 4.0]);
            }
            if !supported {
                ui.text_wrapped("This form has parts rustcord can't show yet. Open it in Discord to fill it in.");
                return;
            }
            let components = form.modal.components.clone();
            for component in &components {
                draw_modal_part(ui, &theme, component, &mut form.answers, true);
            }
            if let Some(error) = &form.error {
                ui.text_colored(theme.danger(), error);
            }
            ui.dummy([0.0, 4.0]);
            let sending = form.sending.is_some();
            let disabled = ui.begin_disabled(sending);
            if ui.button(if sending { "Sending\u{2026}" } else { "Submit" }) {
                submit = true;
            }
            drop(disabled);
        });
    if !open {
        if let Some(nonce) = form.sending.take() {
            state.pending_interactions.remove(&nonce);
        }
        state.modal_form = None;
        return;
    }
    if !submit {
        return;
    }
    if let Some(reason) = form.modal.missing_required(&form.answers) {
        form.error = Some(reason);
        return;
    }
    let channel_id = form.modal.channel_id.clone().or_else(|| state.selected_channel.clone());
    let guild_id = form
        .modal
        .guild_id
        .clone()
        .or_else(|| channel_id.as_deref().and_then(|id| state.find_guild_channel(id)).and_then(|c| c.guild_id.clone()));
    let form = state.modal_form.as_mut().expect("form present");
    form.modal.channel_id = channel_id;
    form.modal.guild_id = guild_id;
    let nonce = interactions::nonce();
    let body = form.modal.submission("", &nonce, &form.answers);
    form.error = None;
    form.sending = Some(nonce.clone());
    state.start_interaction(nonce, None, body, cmd_tx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select() -> Component {
        serde_json::from_value(serde_json::json!({
            "type": 3, "custom_id": "pick", "options": [ { "label": "Apple", "value": "a" }, { "label": "Banana", "value": "b" }, { "label": "Cherry", "value": "c" } ]
        }))
        .unwrap()
    }

    #[test]
    fn a_select_summarises_its_picks_in_option_order() {
        assert_eq!(select_summary(&select(), &["c".into(), "a".into()]).as_deref(), Some("Apple, Cherry"));
        assert_eq!(select_summary(&select(), &[]), None);
    }

    #[test]
    fn button_colours_follow_discords_styles() {
        let theme = crate::theme::Theme::default();
        assert_eq!(button_fill(&theme, Some(button_style::PRIMARY)), theme.accent);
        assert_eq!(button_fill(&theme, Some(button_style::DANGER)), theme.danger());
        assert_eq!(button_ink(&theme, Some(button_style::SECONDARY)), theme.text);
        assert_eq!(button_ink(&theme, Some(button_style::SUCCESS)), [1.0, 1.0, 1.0, 1.0]);
    }
}
