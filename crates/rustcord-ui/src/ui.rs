use crate::app_state::AppState;
use crate::backend::UiCommand;
use imgui::Ui;
use tokio::sync::mpsc::UnboundedSender;

pub fn draw(ui: &Ui, state: &mut AppState, cmd_tx: &UnboundedSender<UiCommand>) {
    if !state.logged_in {
        draw_login(ui, state, cmd_tx);
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
                let label = guild.name.chars().take(2).collect::<String>();
                if ui.button_with_size(&label, [48.0, 48.0]) {
                    let _ = cmd_tx.send(UiCommand::SelectGuild { guild_id: guild.id.clone() });
                    state.selected_guild = Some(guild.id.clone());
                    state.selected_channel = None;
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

    ui.window("chat")
        .position([264.0, 0.0], imgui::Condition::Always)
        .size([display_w - 264.0, display_h], imgui::Condition::Always)
        .no_decoration()
        .draw_background(true)
        .build(|| {
            let status_height = if state.status_message.is_some() { 24.0 } else { 0.0 };
            let messages = state.active_messages().to_vec();
            ui.child_window("history")
                .size([0.0, display_h - 60.0 - status_height])
                .build(|| {
                    if messages.is_empty() {
                        ui.text_disabled("No messages loaded yet.");
                    }
                    for msg in &messages {
                        let author = msg.author.global_name.clone().unwrap_or_else(|| msg.author.username.clone());
                        ui.text_colored(state.theme.accent, &author);
                        ui.same_line();
                        ui.text_wrapped(&msg.content);
                    }
                });

            if let Some(status) = state.status_message.clone() {
                let token = ui.push_style_color(imgui::StyleColor::Text, [1.0, 0.6, 0.3, 1.0]);
                if ui.selectable(format!("{status}  (click to dismiss)")) {
                    state.status_message = None;
                }
                token.pop();
            }

            ui.separator();
            let enter_pressed = ui
                .input_text("##compose", &mut state.compose_buf)
                .enter_returns_true(true)
                .build();
            if enter_pressed {
                if let Some(channel_id) = state.selected_channel.clone() {
                    if !state.compose_buf.trim().is_empty() {
                        let content = std::mem::take(&mut state.compose_buf);
                        let _ = cmd_tx.send(UiCommand::SendMessage { channel_id, content });
                    }
                }
            }
        });
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
