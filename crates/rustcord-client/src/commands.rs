use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CHAT_INPUT: u8 = 1;
pub const APPLICATION_COMMAND_INTERACTION: u8 = 2;

pub mod option_kind {
    pub const SUB_COMMAND: u8 = 1;
    pub const SUB_COMMAND_GROUP: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INTEGER: u8 = 4;
    pub const BOOLEAN: u8 = 5;
    pub const USER: u8 = 6;
    pub const CHANNEL: u8 = 7;
    pub const ROLE: u8 = 8;
    pub const MENTIONABLE: u8 = 9;
    pub const NUMBER: u8 = 10;
    pub const ATTACHMENT: u8 = 11;
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CommandChoice {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CommandOption {
    #[serde(rename = "type")]
    pub kind: u8,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<CommandChoice>,
    #[serde(default)]
    pub options: Vec<CommandOption>,
    #[serde(default)]
    pub channel_types: Vec<u8>,
    #[serde(default)]
    pub min_value: Option<f64>,
    #[serde(default)]
    pub max_value: Option<f64>,
    #[serde(default)]
    pub min_length: Option<u32>,
    #[serde(default)]
    pub max_length: Option<u32>,
    #[serde(default)]
    pub autocomplete: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ApplicationCommand {
    pub id: String,
    pub application_id: String,
    #[serde(default)]
    pub version: String,
    #[serde(rename = "type", default = "chat_input")]
    pub kind: u8,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub options: Vec<CommandOption>,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub contexts: Option<Vec<u8>>,
    #[serde(default)]
    pub dm_permission: Option<bool>,
    #[serde(default)]
    pub raw: Value,
}

fn chat_input() -> u8 {
    CHAT_INPUT
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CommandApplication {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct CommandIndex {
    pub applications: Vec<CommandApplication>,
    pub commands: Vec<ApplicationCommand>,
}

impl CommandIndex {
    pub fn from_response(value: &Value) -> Option<Self> {
        let commands = value.get("application_commands")?.as_array()?;
        let applications = value
            .get("applications")
            .and_then(Value::as_array)
            .map(|apps| apps.iter().filter_map(|a| serde_json::from_value(a.clone()).ok()).collect())
            .unwrap_or_default();
        let commands = commands
            .iter()
            .filter_map(|raw| {
                let mut command: ApplicationCommand = serde_json::from_value(raw.clone()).ok()?;
                command.raw = raw.clone();
                Some(command)
            })
            .collect();
        Some(Self { applications, commands })
    }

    pub fn application(&self, id: &str) -> Option<&CommandApplication> {
        self.applications.iter().find(|a| a.id == id)
    }

    pub fn merged(&self, other: &CommandIndex) -> CommandIndex {
        let mut merged = self.clone();
        for app in &other.applications {
            if merged.application(&app.id).is_none() {
                merged.applications.push(app.clone());
            }
        }
        for command in &other.commands {
            merged.commands.retain(|c| c.id != command.id);
            merged.commands.push(command.clone());
        }
        merged
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandEntry {
    pub command: ApplicationCommand,
    pub path: Vec<String>,
    pub description: String,
    pub options: Vec<CommandOption>,
}

impl CommandEntry {
    pub fn label(&self) -> String {
        std::iter::once(self.command.name.as_str()).chain(self.path.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

pub fn chat_entries(commands: &[ApplicationCommand], in_guild: bool) -> Vec<CommandEntry> {
    let usable_here = |command: &ApplicationCommand| match &command.contexts {
        Some(contexts) => contexts.contains(&if in_guild { 0 } else { 1 }) || (!in_guild && contexts.contains(&2)),
        None => in_guild || command.dm_permission != Some(false),
    };
    let mut entries = Vec::new();
    for command in commands.iter().filter(|c| c.kind == CHAT_INPUT && usable_here(c)) {
        let subcommands: Vec<&CommandOption> = command.options.iter().filter(|o| matches!(o.kind, option_kind::SUB_COMMAND | option_kind::SUB_COMMAND_GROUP)).collect();
        if subcommands.is_empty() {
            entries.push(CommandEntry { command: command.clone(), path: Vec::new(), description: command.description.clone(), options: command.options.clone() });
            continue;
        }
        for sub in subcommands {
            if sub.kind == option_kind::SUB_COMMAND {
                entries.push(CommandEntry { command: command.clone(), path: vec![sub.name.clone()], description: sub.description.clone(), options: sub.options.clone() });
            } else {
                for leaf in sub.options.iter().filter(|o| o.kind == option_kind::SUB_COMMAND) {
                    entries.push(CommandEntry {
                        command: command.clone(),
                        path: vec![sub.name.clone(), leaf.name.clone()],
                        description: leaf.description.clone(),
                        options: leaf.options.clone(),
                    });
                }
            }
        }
    }
    entries
}

pub fn matching<'a>(entries: &'a [CommandEntry], query: &str) -> Vec<&'a CommandEntry> {
    let query = query.trim().to_lowercase();
    let mut hits: Vec<(&CommandEntry, bool)> = entries
        .iter()
        .filter_map(|entry| {
            let label = entry.label().to_lowercase();
            if query.is_empty() || label.starts_with(&query) {
                Some((entry, true))
            } else if label.split(' ').any(|word| word.starts_with(&query)) {
                Some((entry, false))
            } else {
                None
            }
        })
        .collect();
    hits.sort_by_key(|(entry, prefix)| (!prefix, entry.label()));
    hits.into_iter().map(|(entry, _)| entry).collect()
}

pub fn parse_option(option: &CommandOption, text: &str) -> Result<Option<Value>, String> {
    let text = text.trim();
    if text.is_empty() {
        return if option.required { Err(format!("{} is required", option.name)) } else { Ok(None) };
    }
    if !option.choices.is_empty() {
        let choice = option
            .choices
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(text) || c.value.as_str().is_some_and(|v| v == text) || c.value.to_string() == text)
            .ok_or_else(|| format!("{} must be one of its choices", option.name))?;
        return Ok(Some(choice.value.clone()));
    }
    let in_range = |value: f64| -> Result<(), String> {
        if option.min_value.is_some_and(|min| value < min) {
            return Err(format!("{} must be at least {}", option.name, option.min_value.unwrap_or_default()));
        }
        if option.max_value.is_some_and(|max| value > max) {
            return Err(format!("{} must be at most {}", option.name, option.max_value.unwrap_or_default()));
        }
        Ok(())
    };
    let id_of = |text: &str| -> String { text.trim_start_matches(['<', '@', '#', '&', '!']).trim_end_matches('>').to_string() };
    match option.kind {
        option_kind::INTEGER => {
            let value: i64 = text.parse().map_err(|_| format!("{} must be a whole number", option.name))?;
            in_range(value as f64)?;
            Ok(Some(json!(value)))
        }
        option_kind::NUMBER => {
            let value: f64 = text.parse().map_err(|_| format!("{} must be a number", option.name))?;
            in_range(value)?;
            Ok(Some(json!(value)))
        }
        option_kind::BOOLEAN => match text.to_lowercase().as_str() {
            "true" | "yes" | "1" => Ok(Some(json!(true))),
            "false" | "no" | "0" => Ok(Some(json!(false))),
            _ => Err(format!("{} must be True or False", option.name)),
        },
        option_kind::USER | option_kind::CHANNEL | option_kind::ROLE | option_kind::MENTIONABLE => {
            let id = id_of(text);
            if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
                Ok(Some(json!(id)))
            } else {
                Err(format!("{} needs a mention or an id", option.name))
            }
        }
        option_kind::ATTACHMENT => Err(format!("{} takes a file, which rustcord can't send yet", option.name)),
        _ => {
            let length = text.chars().count() as u32;
            if option.min_length.is_some_and(|min| length < min) {
                return Err(format!("{} must be at least {} characters", option.name, option.min_length.unwrap_or(0)));
            }
            if option.max_length.is_some_and(|max| length > max) {
                return Err(format!("{} must be at most {} characters", option.name, option.max_length.unwrap_or(0)));
            }
            Ok(Some(json!(text)))
        }
    }
}

pub fn invoke(entry: &CommandEntry, values: &[(CommandOption, Value)], guild_id: Option<&str>, channel_id: &str, nonce: &str) -> Value {
    let mut options: Vec<Value> = values.iter().map(|(option, value)| json!({ "type": option.kind, "name": option.name, "value": value })).collect();
    let group_kind = |depth: usize| if entry.path.len() == 2 && depth == 0 { option_kind::SUB_COMMAND_GROUP } else { option_kind::SUB_COMMAND };
    for (depth, name) in entry.path.iter().enumerate().rev() {
        options = vec![json!({ "type": group_kind(depth), "name": name, "options": options })];
    }
    let mut data = json!({
        "version": entry.command.version,
        "id": entry.command.id,
        "name": entry.command.name,
        "type": CHAT_INPUT,
        "options": options,
        "application_command": entry.command.raw,
        "attachments": [],
    });
    if let Some(command_guild) = &entry.command.guild_id {
        data["guild_id"] = json!(command_guild);
    }
    json!({
        "type": APPLICATION_COMMAND_INTERACTION,
        "application_id": entry.command.application_id,
        "guild_id": guild_id,
        "channel_id": channel_id,
        "session_id": "",
        "nonce": nonce,
        "analytics_location": "slash_ui",
        "data": data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> CommandIndex {
        CommandIndex::from_response(&json!({
            "applications": [ { "id": "app", "name": "Helper", "bot_id": "bot" } ],
            "application_commands": [
                { "id": "1", "application_id": "app", "version": "v1", "type": 1, "name": "ping", "description": "Pong" },
                { "id": "2", "application_id": "app", "version": "v1", "type": 1, "name": "config", "description": "Settings", "options": [
                    { "type": 1, "name": "show", "description": "Show settings" },
                    { "type": 2, "name": "role", "description": "Roles", "options": [
                        { "type": 1, "name": "add", "description": "Add a role", "options": [ { "type": 8, "name": "role", "description": "Which", "required": true } ] }
                    ] }
                ] },
                { "id": "3", "application_id": "app", "version": "v1", "type": 3, "name": "Report" },
                { "id": "4", "application_id": "app", "version": "v1", "type": 1, "name": "dmonly", "contexts": [1, 2] },
                { "broken": true }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn the_index_flattens_subcommands_and_keeps_only_slash_commands_usable_here() {
        let index = index();
        assert_eq!(index.commands.len(), 4);
        let labels: Vec<String> = chat_entries(&index.commands, true).iter().map(CommandEntry::label).collect();
        assert_eq!(labels, ["ping", "config show", "config role add"]);
        let in_dm: Vec<String> = chat_entries(&index.commands, false).iter().map(CommandEntry::label).collect();
        assert!(in_dm.contains(&"dmonly".to_string()));
    }

    #[test]
    fn matching_prefers_name_prefixes_then_any_word() {
        let entries = chat_entries(&index().commands, true);
        let labels: Vec<String> = matching(&entries, "con").iter().map(|e| e.label()).collect();
        assert_eq!(labels, ["config role add", "config show"]);
        let labels: Vec<String> = matching(&entries, "add").iter().map(|e| e.label()).collect();
        assert_eq!(labels, ["config role add"]);
        assert_eq!(matching(&entries, "").len(), 3);
    }

    #[test]
    fn options_are_checked_and_typed_before_sending() {
        let option = |kind: u8, extra: Value| -> CommandOption {
            let mut value = json!({ "type": kind, "name": "n", "required": true });
            value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(value).unwrap()
        };
        assert_eq!(parse_option(&option(4, json!({ "min_value": 1, "max_value": 10 })), "7"), Ok(Some(json!(7))));
        assert!(parse_option(&option(4, json!({ "min_value": 1, "max_value": 10 })), "11").is_err());
        assert!(parse_option(&option(4, json!({})), "1.5").is_err());
        assert_eq!(parse_option(&option(10, json!({})), "1.5"), Ok(Some(json!(1.5))));
        assert_eq!(parse_option(&option(5, json!({})), "yes"), Ok(Some(json!(true))));
        assert_eq!(parse_option(&option(6, json!({})), "<@!1234>"), Ok(Some(json!("1234"))));
        assert!(parse_option(&option(3, json!({ "max_length": 3 })), "toolong").is_err());
        assert_eq!(parse_option(&option(3, json!({ "choices": [ { "name": "Red", "value": "r" } ] })), "red"), Ok(Some(json!("r"))));
        assert_eq!(parse_option(&option(3, json!({})), " "), Err("n is required".into()));
        let mut optional = option(3, json!({}));
        optional.required = false;
        assert_eq!(parse_option(&optional, ""), Ok(None));
    }

    #[test]
    fn an_invoke_nests_options_under_its_subcommand_path() {
        let entries = chat_entries(&index().commands, true);
        let entry = entries.iter().find(|e| e.label() == "config role add").unwrap();
        let role = entry.options[0].clone();
        let body = invoke(entry, &[(role, json!("555"))], Some("g"), "c", "n");
        assert_eq!(body["type"], 2);
        assert_eq!(body["application_id"], "app");
        assert_eq!(body["analytics_location"], "slash_ui");
        assert_eq!(body["data"]["application_command"]["name"], "config");
        assert_eq!(body["data"]["options"], json!([
            { "type": 2, "name": "role", "options": [ { "type": 1, "name": "add", "options": [ { "type": 8, "name": "role", "value": "555" } ] } ] }
        ]));
        let ping = entries.iter().find(|e| e.label() == "ping").unwrap();
        assert_eq!(invoke(ping, &[], None, "c", "n")["data"]["options"], json!([]));
    }

    #[test]
    fn the_accounts_own_commands_win_a_merge() {
        let place = index();
        let mut own = CommandIndex::default();
        let mut ping = place.commands[0].clone();
        ping.description = "mine".into();
        own.commands.push(ping);
        let merged = place.merged(&own);
        assert_eq!(merged.commands.iter().filter(|c| c.id == "1").count(), 1);
        assert_eq!(merged.commands.iter().find(|c| c.id == "1").unwrap().description, "mine");
    }
}
