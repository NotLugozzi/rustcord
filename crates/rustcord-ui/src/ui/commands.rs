use super::faces;
use crate::app_state::{ActiveCommand, AppState};
use crate::backend::UiCommand;
use imgui::Ui;
use rustcord_client::commands::{self, option_kind, CommandEntry};
use tokio::sync::mpsc::UnboundedSender;

const PICKER_ROW: f32 = 44.0;
const PICKER_MAX_ROWS: usize = 8;
const PICKER_WIDTH: f32 = 460.0;
const FORM_WIDTH: f32 = 460.0;

pub(super) fn picker_query(compose: &str) -> Option<&str> {
    compose.strip_prefix('/')
}

pub(super) struct PickerOutcome {
    pub shown: bool,
    pub suppress_enter: bool,
}

pub(super) fn draw_picker(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, bar_start: [f32; 2]) -> PickerOutcome {
    let none = PickerOutcome { shown: false, suppress_enter: false };
    let Some(channel_id) = state.selected_channel.clone() else { return none };
    let Some(query) = picker_query(&state.compose_buf).map(str::to_string) else {
        state.command_picker_dismissed = false;
        return none;
    };
    if state.command_picker_dismissed || state.active_command.is_some() {
        return none;
    }
    if ui.is_key_pressed(imgui::Key::Escape) {
        state.command_picker_dismissed = true;
        return none;
    }
    state.request_command_indexes(&channel_id, cmd_tx);
    let in_guild = state.find_guild_channel(&channel_id).is_some();
    let (index, loading) = state.commands_for_channel(&channel_id);
    let entries = commands::chat_entries(&index.commands, in_guild);
    let hits: Vec<CommandEntry> = commands::matching(&entries, &query).into_iter().take(PICKER_MAX_ROWS * 4).cloned().collect();
    if hits.is_empty() && !loading {
        return none;
    }
    if hits.is_empty() {
        draw_loading(ui, state, bar_start);
        return PickerOutcome { shown: true, suppress_enter: true };
    }
    let count = hits.len();
    if ui.is_key_pressed(imgui::Key::DownArrow) {
        state.command_picker_selected = (state.command_picker_selected + 1) % count;
    }
    if ui.is_key_pressed(imgui::Key::UpArrow) {
        state.command_picker_selected = (state.command_picker_selected + count - 1) % count;
    }
    state.command_picker_selected = state.command_picker_selected.min(count - 1);
    let confirmed = ui.is_key_pressed(imgui::Key::Tab) || ui.is_key_pressed(imgui::Key::Enter) || ui.is_key_pressed(imgui::Key::KeypadEnter);
    let visible = count.min(PICKER_MAX_ROWS) as f32;
    let height = visible * PICKER_ROW + 36.0;
    let theme = state.theme.clone();
    let mut picked: Option<CommandEntry> = None;
    let selected = state.command_picker_selected;
    let _floating = super::floating_style(ui, &theme);
    ui.window("##command_picker")
        .position([bar_start[0], bar_start[1] - height - 8.0], imgui::Condition::Always)
        .size([PICKER_WIDTH, height], imgui::Condition::Always)
        .no_decoration()
        .focus_on_appearing(false)
        .build(|| {
            let font = ui.push_font(faces().xs);
            ui.text_colored(theme.text_muted, if loading { "COMMANDS (still loading some)" } else { "COMMANDS" });
            font.pop();
            for (row, entry) in hits.iter().enumerate() {
                let is_selected = row == selected;
                let start = ui.cursor_pos();
                let clicked = ui.selectable_config(format!("##command_{row}")).selected(is_selected).size([0.0, PICKER_ROW - 4.0]).build();
                if is_selected {
                    ui.set_scroll_here_y();
                }
                ui.set_cursor_pos([start[0] + 6.0, start[1] + 2.0]);
                let name_font = ui.push_font(faces().semibold);
                ui.text(format!("/{}", entry.label()));
                name_font.pop();
                if let Some(app) = index.application(&entry.command.application_id).filter(|a| !a.name.is_empty()) {
                    ui.same_line();
                    let app_font = ui.push_font(faces().xs);
                    ui.text_colored(theme.text_muted, &app.name);
                    app_font.pop();
                }
                ui.set_cursor_pos([start[0] + 6.0, start[1] + 22.0]);
                let description_font = ui.push_font(faces().sm);
                let description = super::truncate_to_width(ui, &entry.description, PICKER_WIDTH - 24.0).unwrap_or_else(|| entry.description.clone());
                ui.text_colored(theme.text_muted, description);
                description_font.pop();
                ui.set_cursor_pos([start[0], start[1] + PICKER_ROW]);
                if clicked || (is_selected && confirmed) {
                    picked = Some(entry.clone());
                }
            }
        });
    if let Some(entry) = picked {
        let app_name = index.application(&entry.command.application_id).map(|a| a.name.clone());
        state.active_command = Some(ActiveCommand::new(channel_id, entry, app_name));
        state.compose_buf.clear();
        state.command_picker_selected = 0;
    }
    PickerOutcome { shown: true, suppress_enter: true }
}

fn draw_loading(ui: &Ui, state: &AppState, bar_start: [f32; 2]) {
    let height = 40.0;
    let _floating = super::floating_style(ui, &state.theme);
    ui.window("##command_picker")
        .position([bar_start[0], bar_start[1] - height - 8.0], imgui::Condition::Always)
        .size([PICKER_WIDTH, height], imgui::Condition::Always)
        .no_decoration()
        .no_inputs()
        .focus_on_appearing(false)
        .build(|| {
            ui.text_colored(state.theme.text_muted, "Loading commands\u{2026}");
        });
}

fn option_label(option: &commands::CommandOption) -> String {
    let kind = match option.kind {
        option_kind::INTEGER => " (whole number)",
        option_kind::NUMBER => " (number)",
        option_kind::USER => " (user)",
        option_kind::CHANNEL => " (channel)",
        option_kind::ROLE => " (role)",
        option_kind::MENTIONABLE => " (user or role)",
        option_kind::ATTACHMENT => " (file)",
        _ => "",
    };
    format!("{}{}{}", option.name, if option.required { " *" } else { "" }, kind)
}

pub(super) fn draw_active_command(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>, bar_start: [f32; 2], composer_enter: bool) -> bool {
    let Some(active) = state.active_command.as_ref() else { return false };
    if state.selected_channel.as_deref() != Some(active.channel_id.as_str()) {
        state.active_command = None;
        return false;
    }
    let theme = state.theme.clone();
    let rows = active.entry.options.len() as f32;
    let height = 78.0 + rows * 56.0 + if active.error.is_some() { 22.0 } else { 0.0 };
    let mut submit = composer_enter;
    let mut cancel = ui.is_key_pressed(imgui::Key::Escape);
    let active = state.active_command.as_mut().expect("active command");
    let _floating = super::floating_style(ui, &theme);
    ui.window("##active_command")
        .position([bar_start[0], bar_start[1] - height - 8.0], imgui::Condition::Always)
        .size([FORM_WIDTH, height], imgui::Condition::Always)
        .no_decoration()
        .build(|| {
            let title = ui.push_font(faces().semibold);
            ui.text(format!("/{}", active.entry.label()));
            title.pop();
            if let Some(app) = &active.app_name {
                ui.same_line();
                ui.text_colored(theme.text_muted, app);
            }
            if !active.entry.description.is_empty() {
                let font = ui.push_font(faces().sm);
                ui.text_colored(theme.text_muted, &active.entry.description);
                font.pop();
            }
            let options = active.entry.options.clone();
            for (i, option) in options.iter().enumerate() {
                let _id = ui.push_id_usize(i);
                let label_font = ui.push_font(faces().sm_semibold);
                ui.text(option_label(option));
                label_font.pop();
                if ui.is_item_hovered() && !option.description.is_empty() {
                    ui.tooltip_text(&option.description);
                }
                ui.set_next_item_width(-1.0);
                if std::mem::take(&mut active.focus_first) && i == 0 {
                    ui.set_keyboard_focus_here();
                }
                let value = &mut active.values[i];
                if !option.choices.is_empty() {
                    let preview = if value.is_empty() { "Choose\u{2026}".to_string() } else { value.clone() };
                    if let Some(_combo) = ui.begin_combo("##choice", &preview) {
                        for choice in &option.choices {
                            if ui.selectable_config(&choice.name).selected(*value == choice.name).build() {
                                *value = choice.name.clone();
                            }
                        }
                    }
                } else if option.kind == option_kind::BOOLEAN {
                    let mut checked = value == "true";
                    if ui.checkbox("##bool", &mut checked) {
                        *value = checked.to_string();
                    }
                } else {
                    let hint = if option.description.is_empty() { option.name.clone() } else { option.description.clone() };
                    if ui.input_text("##value", value).hint(hint).enter_returns_true(true).build() {
                        submit = true;
                    }
                }
            }
            if let Some(error) = &active.error {
                ui.text_colored(theme.danger(), error);
            }
            if ui.button("Send") {
                submit = true;
            }
            ui.same_line();
            if ui.button("Cancel") {
                cancel = true;
            }
        });
    if cancel {
        state.active_command = None;
        state.focus_compose = true;
    } else if submit {
        state.submit_active_command(cmd_tx);
        state.focus_compose = true;
    }
    true
}

pub(super) fn draw_pending_commands(ui: &Ui, state: &mut AppState, bar_start: [f32; 2]) {
    let Some(channel_id) = state.selected_channel.clone() else { return };
    let here: Vec<(String, String, Option<String>)> = state
        .pending_commands
        .iter()
        .filter(|p| p.channel_id == channel_id)
        .map(|p| (p.nonce.clone(), p.label.clone(), p.error.clone()))
        .collect();
    if here.is_empty() {
        return;
    }
    let theme = state.theme.clone();
    let height = here.len() as f32 * 28.0 + 12.0;
    let mut dismiss: Option<String> = None;
    let _floating = super::floating_style(ui, &theme);
    ui.window("##pending_commands")
        .position([bar_start[0], bar_start[1] - height - 8.0], imgui::Condition::Always)
        .size([FORM_WIDTH, height], imgui::Condition::Always)
        .no_decoration()
        .focus_on_appearing(false)
        .build(|| {
            for (nonce, label, error) in &here {
                let _id = ui.push_id(nonce.as_str());
                match error {
                    None => ui.text_colored(theme.text_muted, format!("Sending /{label}\u{2026}")),
                    Some(reason) => {
                        ui.text_colored(theme.danger(), format!("/{label}: {reason}"));
                        ui.same_line();
                        if ui.small_button("Dismiss") {
                            dismiss = Some(nonce.clone());
                        }
                    }
                }
            }
        });
    if let Some(nonce) = dismiss {
        state.pending_commands.retain(|p| p.nonce != nonce);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_leading_slash_opens_the_picker() {
        assert_eq!(picker_query("/con"), Some("con"));
        assert_eq!(picker_query("/"), Some(""));
        assert_eq!(picker_query("hello /con"), None);
    }

    #[test]
    fn option_labels_mark_required_and_typed_options() {
        let option: commands::CommandOption = serde_json::from_value(serde_json::json!({ "type": 4, "name": "count", "required": true })).unwrap();
        assert_eq!(option_label(&option), "count * (whole number)");
    }
}
