use crate::PluginId;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationEvent {
    Started,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "name")]
pub enum PluginEvent {
    #[serde(rename = "application.started")]
    ApplicationStarted,
    #[serde(rename = "application.shutdown")]
    ApplicationShutdown,
    #[serde(rename = "plugin.enabled")]
    PluginEnabled { plugin_id: PluginId },
    #[serde(rename = "plugin.disabled")]
    PluginDisabled { plugin_id: PluginId },
}

impl PluginEvent {
    pub const NAMES: [&'static str; 4] = [
        "application.started",
        "application.shutdown",
        "plugin.enabled",
        "plugin.disabled",
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Self::ApplicationStarted => Self::NAMES[0],
            Self::ApplicationShutdown => Self::NAMES[1],
            Self::PluginEnabled { .. } => Self::NAMES[2],
            Self::PluginDisabled { .. } => Self::NAMES[3],
        }
    }
}

impl From<ApplicationEvent> for PluginEvent {
    fn from(event: ApplicationEvent) -> Self {
        match event {
            ApplicationEvent::Started => Self::ApplicationStarted,
            ApplicationEvent::Shutdown => Self::ApplicationShutdown,
        }
    }
}

pub(crate) fn supported_name(name: &str) -> Result<&'static str, String> {
    let well_formed = !name.is_empty()
        && name.len() <= 64
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        });
    if !well_formed {
        return Err(format!("invalid event name {name:?}"));
    }
    PluginEvent::NAMES
        .into_iter()
        .find(|supported| *supported == name)
        .ok_or_else(|| format!("unsupported event {name:?}"))
}
