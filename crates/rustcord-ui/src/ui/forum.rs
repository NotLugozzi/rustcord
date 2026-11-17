use super::{draw_icon_at, faces, format_relative, mix, open_thread, truncate_to_width, with_alpha};
use crate::app_state::{AppState, NewPost, MAX_POST_TAGS};
use crate::backend::UiCommand;
use crate::images::TextureCache;
use imgui::Ui;
use rustcord_client::models::{Channel, Message};
use tokio::sync::mpsc::UnboundedSender;

const CARD_MAX_WIDTH: f32 = 760.0;
const CARD_HEIGHT: f32 = 104.0;
const CARD_GAP: f32 = 8.0;
const THUMBNAIL: f32 = 72.0;
const TITLE_LIMIT: usize = 100;
const DEFAULT_AUTO_ARCHIVE_MINUTES: u32 = 4320;
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

fn snowflake_unix(id: &str) -> i64 {
    let ms = (id.parse::<u64>().unwrap_or(0) >> 22) + DISCORD_EPOCH_MS;
    (ms / 1000) as i64
}

fn post_thumbnail(message: &Message) -> Option<String> {
    let attachment = message.attachments.iter().find(|a| {
        a.content_type.as_deref().is_some_and(|t| t.starts_with("image/")) || [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|ext| a.filename.to_lowercase().ends_with(ext))
    });
    attachment
        .map(|a| a.url.clone())
        .or_else(|| message.embeds.iter().find_map(|e| e.thumbnail.as_ref().or(e.image.as_ref()).map(|i| i.url.clone())))
}

fn tag_names(forum: &Channel, post: &Channel) -> Vec<String> {
    post.forum
        .applied_tags
        .iter()
        .filter_map(|id| forum.forum.available_tags.iter().find(|t| &t.id == id))
        .map(|tag| match &tag.emoji_name {
            Some(emoji) if !emoji.is_empty() => format!("{emoji} {}", tag.name),
            _ => tag.name.clone(),
        })
        .collect()
}

fn draw_tag_chip(ui: &Ui, theme: &crate::theme::Theme, at: [f32; 2], text: &str) -> f32 {
    let size = ui.calc_text_size(text);
    let max = [at[0] + size[0] + 12.0, at[1] + size[1] + 4.0];
    let draw_list = ui.get_window_draw_list();
    draw_list.add_rect(at, max, mix(theme.field(), theme.text, 0.10)).filled(true).rounding((size[1] + 4.0) / 2.0).build();
    draw_list.add_text([at[0] + 6.0, at[1] + 2.0], theme.text, text);
    max[0] - at[0] + 4.0
}

fn draw_post_card(ui: &Ui, state: &AppState, textures: &mut TextureCache, cmd_tx: &UnboundedSender<UiCommand>, forum: &Channel, post: &Channel) -> bool {
    let theme = &state.theme;
    let width = ui.content_region_avail()[0].min(CARD_MAX_WIDTH);
    let min = ui.cursor_screen_pos();
    let max = [min[0] + width, min[1] + CARD_HEIGHT];
    let clicked = ui.invisible_button(format!("##post_{}", post.id), [width, CARD_HEIGHT]);
    let hovered = ui.is_item_hovered();
    if hovered {
        ui.set_mouse_cursor(Some(imgui::MouseCursor::Hand));
    }
    let fill = if hovered { mix(theme.field(), theme.text, 0.05) } else { theme.field() };
    ui.get_window_draw_list().add_rect(min, max, fill).filled(true).rounding(8.0).build();

    let view = state.forum_view.as_ref();
    let first = view.and_then(|v| v.first_messages.get(&post.id));
    let thumbnail = first.and_then(post_thumbnail);
    let text_right = max[0] - 14.0 - if thumbnail.is_some() { THUMBNAIL + 12.0 } else { 0.0 };
    let mut x = min[0] + 14.0;
    let mut y = min[1] + 10.0;

    let tags = tag_names(forum, post);
    if post.is_pinned_post() || !tags.is_empty() {
        let font = ui.push_font(faces().xs);
        if post.is_pinned_post() {
            draw_icon_at(ui, textures, crate::icons::Icon::Pin, [x, y + 1.0], 14.0, theme.text_muted);
            x += 20.0;
        }
        for tag in tags.iter().take(3) {
            x += draw_tag_chip(ui, theme, [x, y], tag);
        }
        if tags.len() > 3 {
            draw_tag_chip(ui, theme, [x, y], &format!("+{}", tags.len() - 3));
        }
        font.pop();
        x = min[0] + 14.0;
        y += 22.0;
    }

    let unread = state.channel_shows_unread(forum.guild_id.as_deref(), &post.id, post.last_message_id.as_deref());
    let title = post.name.clone().unwrap_or_else(|| "Untitled post".into());
    let title_font = ui.push_font(faces().semibold);
    let shown = truncate_to_width(ui, &title, text_right - x).unwrap_or(title);
    ui.get_window_draw_list().add_text([x, y], if unread { theme.text } else { mix(theme.text, theme.text_muted, 0.35) }, &shown);
    title_font.pop();
    y += 22.0;

    let preview_font = ui.push_font(faces().sm);
    match first {
        Some(message) => {
            let guild_id = forum.guild_id.clone().unwrap_or_default();
            let author = message.author.global_name.clone().unwrap_or_else(|| message.author.username.clone());
            let author_color = state.member_color(&guild_id, &message.author.id).unwrap_or(theme.text);
            let author_label = format!("{author}: ");
            let author_w = ui.calc_text_size(&author_label)[0];
            let body = message.content.lines().next().unwrap_or("").to_string();
            let body = if body.is_empty() && !message.attachments.is_empty() { "Sent an attachment".to_string() } else { body };
            let body = truncate_to_width(ui, &body, (text_right - x - author_w).max(20.0)).unwrap_or(body);
            let draw_list = ui.get_window_draw_list();
            draw_list.add_text([x, y], author_color, &author_label);
            draw_list.add_text([x + author_w, y], theme.text_muted, &body);
        }
        None => {
            ui.get_window_draw_list().add_text([x, y], theme.text_faint(), "\u{2026}");
        }
    }
    preview_font.pop();
    y += 22.0;

    let footer_font = ui.push_font(faces().xs);
    draw_icon_at(ui, textures, crate::icons::Icon::MessageCircle, [x, y + 1.0], 14.0, theme.text_muted);
    let replies = post.thread.message_count.unwrap_or(0);
    let activity = post.last_message_id.as_deref().unwrap_or(&post.id);
    let footer = format!("{replies}  \u{2022}  {}", format_relative(snowflake_unix(activity)));
    ui.get_window_draw_list().add_text([x + 20.0, y], theme.text_muted, &footer);
    footer_font.pop();

    if let Some(url) = thumbnail {
        let thumb_min = [max[0] - 14.0 - THUMBNAIL, min[1] + (CARD_HEIGHT - THUMBNAIL) / 2.0];
        let thumb_max = [thumb_min[0] + THUMBNAIL, thumb_min[1] + THUMBNAIL];
        match textures.get_still(&url) {
            Some((texture, _)) => ui.get_window_draw_list().add_image_rounded(texture, thumb_min, thumb_max, 6.0).build(),
            None => {
                textures.request(&url, cmd_tx);
                ui.get_window_draw_list().add_rect(thumb_min, thumb_max, with_alpha(theme.text_muted, 0.15)).filled(true).rounding(6.0).build();
            }
        }
    }
    ui.set_cursor_screen_pos([min[0], max[1] + CARD_GAP]);
    ui.dummy([0.0, 0.0]);
    clicked
}

fn draw_new_post(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, forum: &Channel) {
    let theme = state.theme.clone();
    let guild_id = forum.guild_id.clone().unwrap_or_default();
    let can_moderate = state.can_manage_threads(&guild_id, forum);
    let Some(draft) = state.forum_view.as_mut().and_then(|v| v.new_post.as_mut()) else { return };
    let width = ui.content_region_avail()[0].min(CARD_MAX_WIDTH);
    let mut post = false;
    let mut cancel = false;
    ui.child_window("new_post").size([width, 0.0]).border(true).always_auto_resize(true).build(|| {
        let font = ui.push_font(faces().title);
        ui.text("New Post");
        font.pop();
        ui.set_next_item_width(-1.0);
        ui.input_text("##post_title", &mut draft.title).hint("Post title").build();
        if draft.title.chars().count() > TITLE_LIMIT {
            draft.title = draft.title.chars().take(TITLE_LIMIT).collect();
        }
        let tags: Vec<_> = forum.forum.available_tags.iter().filter(|t| can_moderate || !t.moderated).cloned().collect();
        if !tags.is_empty() {
            ui.text_colored(theme.text_muted, format!("Tags (up to {MAX_POST_TAGS})"));
            for (i, tag) in tags.iter().enumerate() {
                if i > 0 && ui.content_region_avail()[0] > 140.0 {
                    ui.same_line();
                }
                let mut on = draft.tags.contains(&tag.id);
                let label = match &tag.emoji_name {
                    Some(emoji) if !emoji.is_empty() => format!("{emoji} {}##tag_{}", tag.name, tag.id),
                    _ => format!("{}##tag_{}", tag.name, tag.id),
                };
                if ui.checkbox(&label, &mut on) {
                    if on && draft.tags.len() < MAX_POST_TAGS {
                        draft.tags.push(tag.id.clone());
                    } else {
                        draft.tags.retain(|t| t != &tag.id);
                    }
                }
            }
        }
        if let Some(guidelines) = forum.topic.as_deref().filter(|t| !t.trim().is_empty()) {
            ui.text_colored(theme.text_muted, "Post Guidelines");
            ui.text_wrapped(guidelines);
        }
        ui.input_text_multiline("##post_body", &mut draft.body, [-1.0, 120.0]).build();
        if let Some(error) = &draft.error {
            ui.text_colored(theme.danger(), error);
        }
        if ui.button("Post") {
            post = true;
        }
        ui.same_line();
        if ui.button("Cancel") {
            cancel = true;
        }
    });
    if cancel {
        if let Some(view) = state.forum_view.as_mut() {
            view.new_post = None;
        }
        return;
    }
    if !post {
        return;
    }
    let Some(draft) = state.forum_view.as_mut().and_then(|v| v.new_post.as_mut()) else { return };
    if let Err(reason) = draft.validate(forum) {
        draft.error = Some(reason.to_string());
        return;
    }
    let draft = state.forum_view.as_mut().and_then(|v| v.new_post.take()).expect("draft");
    let _ = cmd_tx.send(UiCommand::CreateForumPost {
        forum_id: forum.id.clone(),
        name: draft.title.trim().to_string(),
        tags: draft.tags,
        content: draft.body,
        auto_archive_minutes: forum.thread.default_auto_archive_duration.unwrap_or(DEFAULT_AUTO_ARCHIVE_MINUTES),
    });
}

pub(super) fn draw_forum(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, textures: &mut TextureCache, forum: &Channel) {
    state.request_forum_post_data(&forum.id, cmd_tx);
    let theme = state.theme.clone();
    ui.child_window(format!("forum_{}", forum.id)).size([0.0, 0.0]).build(|| {
        ui.dummy([0.0, 6.0]);
        if let Some(view) = state.forum_view.as_mut() {
            ui.set_next_item_width(260.0);
            ui.input_text("##forum_search", &mut view.search).hint("Search posts").build();
            ui.same_line();
            if ui.button("New Post") && view.new_post.is_none() {
                view.new_post = Some(NewPost::default());
            }
        }
        ui.dummy([0.0, 6.0]);
        if state.forum_view.as_ref().is_some_and(|v| v.new_post.is_some()) {
            draw_new_post(ui, state, cmd_tx, forum);
            ui.dummy([0.0, 8.0]);
        }
        let (active, archived) = state.forum_posts(&forum.id);
        let (loading, has_more, loaded) = state
            .forum_view
            .as_ref()
            .map(|v| (v.loading_archived, v.has_more, v.archived.len() as u32))
            .unwrap_or_default();
        if active.is_empty() && archived.is_empty() {
            ui.text_disabled(if loading { "Loading posts\u{2026}" } else { "No posts here yet." });
        }
        let mut opened: Option<Channel> = None;
        for post in &active {
            if draw_post_card(ui, state, textures, cmd_tx, forum, post) {
                opened = Some(post.clone());
            }
        }
        if !archived.is_empty() {
            ui.dummy([0.0, 6.0]);
            let font = ui.push_font(faces().xs);
            ui.text_colored(theme.text_muted, "OLDER POSTS");
            font.pop();
            for post in &archived {
                if draw_post_card(ui, state, textures, cmd_tx, forum, post) {
                    opened = Some(post.clone());
                }
            }
        }
        if loading && !(active.is_empty() && archived.is_empty()) {
            ui.text_disabled("Loading older posts\u{2026}");
        } else if has_more && ui.button("Load more") {
            let _ = cmd_tx.send(UiCommand::LoadArchivedThreads { channel_id: forum.id.clone(), guild_id: forum.guild_id.clone().unwrap_or_default(), offset: loaded });
            if let Some(view) = state.forum_view.as_mut() {
                view.loading_archived = true;
            }
        }
        if let Some(post) = opened {
            open_thread(state, cmd_tx, &post, forum.guild_id.as_deref());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_snowflake_reads_as_its_time() {
        let unix = 1_700_000_000i64;
        let id = (((unix as u64) * 1000 - DISCORD_EPOCH_MS) << 22).to_string();
        assert_eq!(snowflake_unix(&id), unix);
    }

    #[test]
    fn a_posts_tags_resolve_by_id_with_their_emoji() {
        let forum: Channel = serde_json::from_value(serde_json::json!({
            "id": "f", "type": 15, "available_tags": [ { "id": "1", "name": "Bug", "emoji_name": "🐛" }, { "id": "2", "name": "Idea" } ]
        }))
        .unwrap();
        let post: Channel = serde_json::from_value(serde_json::json!({ "id": "p", "type": 11, "applied_tags": ["2", "1", "9"] })).unwrap();
        assert_eq!(tag_names(&forum, &post), ["Idea", "🐛 Bug"]);
    }

    #[test]
    fn a_thumbnail_comes_from_an_image_attachment_first() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "m", "channel_id": "p", "content": "", "timestamp": "2026-11-08T12:00:00+00:00", "edited_timestamp": null,
            "author": { "id": "a", "username": "a", "discriminator": "0" },
            "attachments": [ { "id": "1", "filename": "notes.txt", "url": "https://cdn/notes.txt", "size": 1 }, { "id": "2", "filename": "shot.PNG", "url": "https://cdn/shot.png", "size": 1 } ]
        }))
        .unwrap();
        assert_eq!(post_thumbnail(&message).as_deref(), Some("https://cdn/shot.png"));
    }
}
