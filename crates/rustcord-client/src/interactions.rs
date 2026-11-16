use crate::models::{component_kind, Component};
use serde_json::{json, Value};
use std::collections::HashMap;

const DISCORD_EPOCH_MS: u128 = 1_420_070_400_000;
pub const COMPONENT_INTERACTION: u8 = 3;
pub const MODAL_SUBMIT_INTERACTION: u8 = 5;

pub fn nonce() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let entropy = u128::from(std::process::id() & 0x3f) << 16 | u128::from(rand_low_bits());
    ((now.saturating_sub(DISCORD_EPOCH_MS) << 22) | entropy).to_string()
}

fn rand_low_bits() -> u16 {
    static COUNTER: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageTarget {
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub message_flags: u64,
    pub application_id: String,
}

fn component_envelope(target: &MessageTarget, session_id: &str, nonce: &str, data: Value) -> Value {
    json!({
        "type": COMPONENT_INTERACTION,
        "nonce": nonce,
        "guild_id": target.guild_id,
        "channel_id": target.channel_id,
        "message_flags": target.message_flags,
        "message_id": target.message_id,
        "application_id": target.application_id,
        "session_id": session_id,
        "data": data,
    })
}

pub fn button_click(target: &MessageTarget, session_id: &str, nonce: &str, custom_id: &str) -> Value {
    component_envelope(target, session_id, nonce, json!({ "component_type": component_kind::BUTTON, "custom_id": custom_id }))
}

pub fn select_pick(target: &MessageTarget, session_id: &str, nonce: &str, custom_id: &str, kind: u8, values: &[String]) -> Value {
    component_envelope(target, session_id, nonce, json!({ "component_type": kind, "custom_id": custom_id, "type": kind, "values": values }))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Modal {
    pub interaction_id: String,
    pub nonce: Option<String>,
    pub application_id: String,
    pub application_name: Option<String>,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub custom_id: String,
    pub title: String,
    pub components: Vec<Component>,
}

impl Modal {
    pub fn parse(data: &Value) -> Option<Self> {
        let application = data.get("application");
        let application_id = application
            .and_then(|a| a.get("id"))
            .or_else(|| data.get("application_id"))
            .and_then(Value::as_str)?
            .to_string();
        let components = data
            .get("components")
            .and_then(Value::as_array)
            .map(|raw| raw.iter().filter_map(|c| serde_json::from_value(c.clone()).ok()).collect())
            .unwrap_or_default();
        Some(Self {
            interaction_id: data.get("id")?.as_str()?.to_string(),
            nonce: data.get("nonce").and_then(Value::as_str).map(str::to_string),
            application_id,
            application_name: application.and_then(|a| a.get("name")).and_then(Value::as_str).map(str::to_string),
            guild_id: data.get("guild_id").and_then(Value::as_str).map(str::to_string),
            channel_id: data.get("channel_id").and_then(Value::as_str).map(str::to_string),
            custom_id: data.get("custom_id")?.as_str()?.to_string(),
            title: data.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
            components,
        })
    }

    pub fn inputs(&self) -> Vec<&Component> {
        fn walk<'a>(components: &'a [Component], out: &mut Vec<&'a Component>) {
            for component in components {
                match component.kind {
                    component_kind::TEXT_INPUT | component_kind::STRING_SELECT => out.push(component),
                    _ => {}
                }
                walk(&component.components, out);
                if let Some(inner) = &component.component {
                    walk(std::slice::from_ref(inner), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.components, &mut out);
        out
    }

    pub fn is_supported(&self) -> bool {
        fn supported(component: &Component) -> bool {
            use component_kind::*;
            let own = matches!(component.kind, ACTION_ROW | TEXT_INPUT | STRING_SELECT | TEXT_DISPLAY | LABEL);
            own && component.components.iter().all(supported) && component.component.as_deref().is_none_or(supported)
        }
        !self.components.is_empty() && self.components.iter().all(supported)
    }

    pub fn submission(&self, session_id: &str, nonce: &str, answers: &HashMap<String, ModalAnswer>) -> Value {
        fn submit(component: &Component, answers: &HashMap<String, ModalAnswer>) -> Option<Value> {
            use component_kind::*;
            let custom_id = component.custom_id.clone().unwrap_or_default();
            match component.kind {
                ACTION_ROW => Some(json!({ "type": ACTION_ROW, "components": component.components.iter().filter_map(|c| submit(c, answers)).collect::<Vec<_>>() })),
                LABEL => {
                    let inner = component.component.as_deref().and_then(|c| submit(c, answers))?;
                    Some(json!({ "type": LABEL, "component": inner }))
                }
                TEXT_DISPLAY => Some(json!({ "type": TEXT_DISPLAY })),
                TEXT_INPUT => {
                    let value = match answers.get(&custom_id) {
                        Some(ModalAnswer::Text(text)) => text.clone(),
                        _ => component.value.clone().unwrap_or_default(),
                    };
                    Some(json!({ "type": TEXT_INPUT, "custom_id": custom_id, "value": value }))
                }
                STRING_SELECT => {
                    let values = match answers.get(&custom_id) {
                        Some(ModalAnswer::Picks(picks)) => picks.clone(),
                        _ => component.options.iter().filter(|o| o.default).map(|o| o.value.clone()).collect(),
                    };
                    Some(json!({ "type": STRING_SELECT, "custom_id": custom_id, "values": values }))
                }
                _ => None,
            }
        }
        json!({
            "type": MODAL_SUBMIT_INTERACTION,
            "nonce": nonce,
            "guild_id": self.guild_id,
            "channel_id": self.channel_id,
            "application_id": self.application_id,
            "session_id": session_id,
            "data": {
                "id": self.interaction_id,
                "custom_id": self.custom_id,
                "components": self.components.iter().filter_map(|c| submit(c, answers)).collect::<Vec<_>>(),
            },
        })
    }

    pub fn missing_required(&self, answers: &HashMap<String, ModalAnswer>) -> Option<String> {
        self.inputs().into_iter().find_map(|input| {
            let required = input.required.unwrap_or(true);
            let id = input.custom_id.clone().unwrap_or_default();
            let empty = match answers.get(&id) {
                Some(ModalAnswer::Text(text)) => text.trim().is_empty(),
                Some(ModalAnswer::Picks(picks)) => picks.is_empty(),
                None => input.value.as_deref().is_none_or(|v| v.trim().is_empty()) && !input.options.iter().any(|o| o.default),
            };
            let too_short = match (answers.get(&id), input.min_length) {
                (Some(ModalAnswer::Text(text)), Some(min)) => !text.is_empty() && (text.chars().count() as u32) < min,
                _ => false,
            };
            let label = input.label.clone().unwrap_or_else(|| "This field".into());
            if required && empty {
                Some(format!("{label} is required"))
            } else if too_short {
                Some(format!("{label} must be at least {} characters", input.min_length.unwrap_or(0)))
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModalAnswer {
    Text(String),
    Picks(Vec<String>),
}

pub fn failure_text(error: &crate::error::ClientError) -> String {
    match error {
        crate::error::ClientError::Api { status: 429, .. } => "Slow down! You're clicking too fast.".into(),
        crate::error::ClientError::Api { .. } => "This interaction failed".into(),
        other => format!("This interaction couldn't be sent: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> MessageTarget {
        MessageTarget { guild_id: Some("g".into()), channel_id: "c".into(), message_id: "m".into(), message_flags: 0, application_id: "app".into() }
    }

    #[test]
    fn a_button_click_names_the_message_and_the_control() {
        let body = button_click(&target(), "sess", "123", "yes");
        assert_eq!(body["type"], 3);
        assert_eq!(body["application_id"], "app");
        assert_eq!(body["session_id"], "sess");
        assert_eq!(body["data"], json!({ "component_type": 2, "custom_id": "yes" }));
    }

    #[test]
    fn a_select_pick_carries_its_kind_and_values() {
        let body = select_pick(&target(), "sess", "1", "pick", 3, &["a".into(), "b".into()]);
        assert_eq!(body["data"], json!({ "component_type": 3, "custom_id": "pick", "type": 3, "values": ["a", "b"] }));
    }

    #[test]
    fn nonces_are_unique_snowflakes() {
        let (a, b) = (nonce(), nonce());
        assert_ne!(a, b);
        assert!(a.parse::<u64>().is_ok());
    }

    fn modal() -> Modal {
        Modal::parse(&json!({
            "id": "int", "nonce": "n", "channel_id": "c", "guild_id": "g", "custom_id": "form", "title": "Report",
            "application": { "id": "app", "name": "Helper" },
            "components": [
                { "type": 1, "components": [ { "type": 4, "custom_id": "why", "label": "Why", "style": 2, "min_length": 3 } ] },
                { "type": 18, "label": "Severity", "component": { "type": 3, "custom_id": "sev", "options": [ { "label": "Low", "value": "low", "default": true }, { "label": "High", "value": "high" } ] } },
                { "type": 10, "content": "Thanks!" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn a_modal_parses_and_lists_its_inputs() {
        let modal = modal();
        assert_eq!(modal.application_name.as_deref(), Some("Helper"));
        assert!(modal.is_supported());
        let ids: Vec<_> = modal.inputs().iter().map(|c| c.custom_id.clone().unwrap()).collect();
        assert_eq!(ids, ["why", "sev"]);
    }

    #[test]
    fn a_modal_submission_mirrors_the_forms_layout() {
        let modal = modal();
        let mut answers = HashMap::new();
        answers.insert("why".to_string(), ModalAnswer::Text("spam".into()));
        let body = modal.submission("sess", "n2", &answers);
        assert_eq!(body["type"], 5);
        assert_eq!(body["data"]["id"], "int");
        assert_eq!(body["data"]["custom_id"], "form");
        assert_eq!(body["data"]["components"], json!([
            { "type": 1, "components": [ { "type": 4, "custom_id": "why", "value": "spam" } ] },
            { "type": 18, "component": { "type": 3, "custom_id": "sev", "values": ["low"] } },
            { "type": 10 }
        ]));
    }

    #[test]
    fn required_and_short_answers_are_caught_before_sending() {
        let modal = modal();
        let mut answers = HashMap::new();
        assert_eq!(modal.missing_required(&answers).as_deref(), Some("Why is required"));
        answers.insert("why".to_string(), ModalAnswer::Text("no".into()));
        assert_eq!(modal.missing_required(&answers).as_deref(), Some("Why must be at least 3 characters"));
        answers.insert("why".to_string(), ModalAnswer::Text("spam".into()));
        assert_eq!(modal.missing_required(&answers), None);
    }

    #[test]
    fn a_modal_with_unknown_parts_is_not_supported() {
        let mut modal = modal();
        modal.components.push(serde_json::from_value(json!({ "type": 19 })).unwrap());
        assert!(!modal.is_supported());
    }
}
