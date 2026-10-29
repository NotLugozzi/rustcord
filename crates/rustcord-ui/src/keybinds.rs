use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ToggleMute,
    ToggleDeafen,
    FocusCompose,
    LeaveVoiceChannel,
}

impl Action {
    pub const ALL: [Action; 4] = [Action::ToggleMute, Action::ToggleDeafen, Action::FocusCompose, Action::LeaveVoiceChannel];

    pub fn label(self) -> &'static str {
        match self {
            Action::ToggleMute => "Toggle Mute",
            Action::ToggleDeafen => "Toggle Deafen",
            Action::FocusCompose => "Focus Message Box",
            Action::LeaveVoiceChannel => "Leave Voice Channel",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Action::ToggleMute => "toggle_mute",
            Action::ToggleDeafen => "toggle_deafen",
            Action::FocusCompose => "focus_compose",
            Action::LeaveVoiceChannel => "leave_voice_channel",
        }
    }
}

pub fn key_name(key: imgui::Key) -> String {
    format!("{key:?}")
}

pub fn key_from_name(name: &str) -> Option<imgui::Key> {
    imgui::Key::VARIANTS.into_iter().find(|k| key_name(*k) == name)
}

pub struct Keybinds {
    binds: HashMap<Action, imgui::Key>,
}

impl Default for Keybinds {
    fn default() -> Self {
        let mut binds = HashMap::new();
        binds.insert(Action::ToggleMute, imgui::Key::F1);
        binds.insert(Action::ToggleDeafen, imgui::Key::F2);
        Self { binds }
    }
}

impl Keybinds {
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        let mut binds = HashMap::new();
        for action in Action::ALL {
            if let Some(key) = map.get(action.id()).and_then(|name| key_from_name(name)) {
                binds.insert(action, key);
            }
        }
        if binds.is_empty() {
            return Self::default();
        }
        Self { binds }
    }

    pub fn to_map(&self) -> HashMap<String, String> {
        self.binds.iter().map(|(action, key)| (action.id().to_string(), key_name(*key))).collect()
    }

    pub fn get(&self, action: Action) -> Option<imgui::Key> {
        self.binds.get(&action).copied()
    }

    pub fn set(&mut self, action: Action, key: Option<imgui::Key>) {
        match key {
            Some(key) => {
                self.binds.insert(action, key);
            }
            None => {
                self.binds.remove(&action);
            }
        }
    }
}
