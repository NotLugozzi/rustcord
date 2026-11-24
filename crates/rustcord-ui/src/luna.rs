use rustcord_plugins::{ApplicationEvent, PluginApiVersion, PluginError, PluginId, PluginManager, PluginState};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use crate::backend::BackendEvent;

const REQUEST_CAPACITY: usize = 32;
const MAX_LOG_LINES: usize = 200;
const MAX_ERROR_LINES: usize = 100;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub enum LunaRequest {
    SetRuntimeEnabled(bool),
    Rescan,
    Load(PathBuf),
    Unload(PluginId),
    Enable(PluginId),
    Disable(PluginId),
    ClearDiagnostics,
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum ApiStatus {
    Compatible(PluginApiVersion),
    Unsupported(PluginApiVersion),
    Invalid(String),
}

#[derive(Clone, Debug)]
pub enum PluginStatus {
    NotLoaded,
    Loaded,
    Enabled,
    Failed(PluginError),
}

impl PluginStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotLoaded => "Not loaded",
            Self::Loaded => "Loaded",
            Self::Enabled => "Enabled",
            Self::Failed(_) => "Failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PluginView {
    pub directory: PathBuf,
    pub id: Option<PluginId>,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub api: ApiStatus,
    pub status: PluginStatus,
    pub subscriptions: Vec<&'static str>,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub at: chrono::DateTime<chrono::Local>,
    pub plugin_id: PluginId,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ErrorLine {
    pub at: chrono::DateTime<chrono::Local>,
    pub plugin_id: PluginId,
    pub error: PluginError,
}

#[derive(Clone, Debug)]
pub struct ActionOutcome {
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct LunaSnapshot {
    pub available: bool,
    pub runtime_enabled: bool,
    pub plugins_dir: Option<PathBuf>,
    pub scan_error: Option<String>,
    pub plugins: Vec<PluginView>,
    pub logs: VecDeque<LogLine>,
    pub errors: VecDeque<ErrorLine>,
    pub last_action: Option<ActionOutcome>,
}

impl LunaSnapshot {
    pub fn enabled_count(&self) -> usize {
        self.plugins.iter().filter(|plugin| matches!(plugin.status, PluginStatus::Enabled)).count()
    }
}

#[derive(Clone)]
pub struct LunaClient(mpsc::SyncSender<LunaRequest>);

impl LunaClient {
    pub fn send(&self, request: LunaRequest) -> bool {
        self.0.try_send(request).is_ok()
    }
}

pub struct LunaConfig {
    pub plugins_dir: Option<PathBuf>,
    pub storage_dir: Option<PathBuf>,
    pub runtime_enabled: bool,
    pub enabled_plugins: Vec<String>,
}

impl LunaConfig {
    pub fn from_paths(runtime_enabled: bool, enabled_plugins: Vec<String>) -> Self {
        let config = crate::paths::config_dir();
        Self {
            plugins_dir: config.as_ref().map(|dir| dir.join("plugins")),
            storage_dir: config.map(|dir| dir.join("plugin-data")),
            runtime_enabled,
            enabled_plugins,
        }
    }
}

pub struct LunaHost {
    client: LunaClient,
    thread: Option<JoinHandle<()>>,
}

impl LunaHost {
    pub fn start(config: LunaConfig, events: UnboundedSender<BackendEvent>) -> Self {
        let (tx, rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("luna-host".into())
            .spawn(move || Host::new(config, events).run(rx))
            .map_err(|error| tracing::warn!("could not start Luna: {error}"))
            .ok();
        Self { client: LunaClient(tx), thread }
    }

    pub fn client(&self) -> LunaClient {
        self.client.clone()
    }

    pub fn shutdown(mut self) {
        let _ = self.client.0.send(LunaRequest::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Host {
    manager: Option<PluginManager>,
    events: UnboundedSender<BackendEvent>,
    plugins_dir: Option<PathBuf>,
    wanted: BTreeSet<String>,
    directories: BTreeMap<PluginId, PathBuf>,
    snapshot: LunaSnapshot,
}

impl Host {
    fn new(config: LunaConfig, events: UnboundedSender<BackendEvent>) -> Self {
        if let Some(dir) = &config.plugins_dir {
            if let Err(error) = std::fs::create_dir_all(dir) {
                tracing::warn!("could not create the Luna plugins folder {}: {error}", dir.display());
            }
        }
        let manager = config.storage_dir.map(PluginManager::new);
        Self {
            snapshot: LunaSnapshot {
                available: manager.is_some(),
                runtime_enabled: config.runtime_enabled,
                plugins_dir: config.plugins_dir.clone(),
                ..Default::default()
            },
            manager,
            events,
            plugins_dir: config.plugins_dir,
            wanted: config.enabled_plugins.into_iter().collect(),
            directories: BTreeMap::new(),
        }
    }

    fn run(mut self, requests: mpsc::Receiver<LunaRequest>) {
        self.rescan();
        if self.snapshot.runtime_enabled {
            self.start_wanted();
            if let Some(manager) = &mut self.manager {
                manager.dispatch(ApplicationEvent::Started);
            }
        }
        self.publish();
        loop {
            let mut changed = match requests.recv_timeout(POLL_INTERVAL) {
                Ok(LunaRequest::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(request) => {
                    self.handle(request);
                    true
                }
                Err(mpsc::RecvTimeoutError::Timeout) => false,
            };
            changed |= self.collect();
            if changed {
                self.refresh_states();
                self.publish();
            }
        }
        self.stop_all(true);
    }

    fn handle(&mut self, request: LunaRequest) {
        let outcome = match request {
            LunaRequest::SetRuntimeEnabled(enabled) => {
                self.snapshot.runtime_enabled = enabled;
                if enabled {
                    self.start_wanted();
                    ok("Luna is on.")
                } else {
                    self.stop_all(false);
                    ok("Luna is off. All plugins were disabled.")
                }
            }
            LunaRequest::Rescan => {
                self.rescan();
                ok(format!("Found {} plugins.", self.snapshot.plugins.len()))
            }
            LunaRequest::Load(directory) => match self.load(&directory) {
                Ok(id) => ok(format!("Loaded {id}.")),
                Err(error) => self.failed("Could not load the plugin", None, error),
            },
            LunaRequest::Unload(id) => {
                self.wanted.remove(id.as_str());
                let result = self.manager().and_then(|manager| manager.unload(&id));
                self.directories.remove(&id);
                match result {
                    Ok(()) => ok(format!("Unloaded {id}.")),
                    Err(error) => self.failed("Unloading reported an error", Some(&id), error),
                }
            }
            LunaRequest::Enable(id) => {
                self.wanted.insert(id.to_string());
                if !self.snapshot.runtime_enabled {
                    ActionOutcome { ok: false, message: "Turn Luna on to enable plugins.".into() }
                } else {
                    match self.enable(&id) {
                        Ok(()) => ok(format!("Enabled {id}.")),
                        Err(error) => self.failed("Could not enable the plugin", Some(&id), error),
                    }
                }
            }
            LunaRequest::Disable(id) => {
                self.wanted.remove(id.as_str());
                match self.manager().and_then(|manager| manager.disable(&id)) {
                    Ok(()) => ok(format!("Disabled {id}.")),
                    Err(error) => self.failed("Disabling reported an error", Some(&id), error),
                }
            }
            LunaRequest::ClearDiagnostics => {
                self.snapshot.logs.clear();
                self.snapshot.errors.clear();
                ok("Cleared the plugin logs and errors.")
            }
            LunaRequest::Shutdown => return,
        };
        self.snapshot.last_action = Some(outcome);
    }

    fn manager(&mut self) -> Result<&mut PluginManager, PluginError> {
        self.manager.as_mut().ok_or(PluginError::Runtime("no folder is available for plugin data".into()))
    }

    fn failed(&mut self, what: &str, id: Option<&PluginId>, error: PluginError) -> ActionOutcome {
        let message = format!("{what}: {error}");
        if let Some(id) = id {
            self.record_error(id.clone(), error);
        }
        ActionOutcome { ok: false, message }
    }

    fn load(&mut self, directory: &Path) -> Result<PluginId, PluginError> {
        let id = self.manager()?.load(directory)?;
        self.directories.insert(id.clone(), directory.to_path_buf());
        Ok(id)
    }

    fn enable(&mut self, id: &PluginId) -> Result<(), PluginError> {
        let loaded = self.manager.as_ref().is_some_and(|manager| manager.state(id).is_ok());
        if !loaded {
            let directory = self
                .snapshot
                .plugins
                .iter()
                .find(|plugin| plugin.id.as_ref() == Some(id))
                .map(|plugin| plugin.directory.clone())
                .ok_or_else(|| PluginError::NotFound(id.clone()))?;
            self.load(&directory)?;
        }
        self.manager()?.enable(id)
    }

    fn start_wanted(&mut self) {
        let wanted: Vec<PluginId> = self.wanted.iter().filter_map(|id| PluginId::new(id.as_str()).ok()).collect();
        for id in wanted {
            if let Err(error) = self.enable(&id) {
                tracing::warn!("Luna could not enable {id}: {error}");
                self.record_error(id, error);
            }
        }
        self.refresh_states();
    }

    fn stop_all(&mut self, application_exit: bool) {
        let Some(manager) = &mut self.manager else { return };
        if application_exit {
            manager.dispatch(ApplicationEvent::Shutdown);
        }
        for (id, error) in manager.shutdown(SHUTDOWN_GRACE) {
            tracing::warn!("Luna plugin {id} reported an error while stopping: {error}");
            self.record_error(id, error);
        }
        self.collect();
        self.refresh_states();
    }

    fn collect(&mut self) -> bool {
        let Some(manager) = &mut self.manager else { return false };
        let failures = manager.poll();
        let logs = manager.take_logs();
        let changed = !failures.is_empty() || !logs.is_empty();
        let now = chrono::Local::now();
        for log in logs {
            push_bounded(&mut self.snapshot.logs, LogLine { at: now, plugin_id: log.plugin_id().clone(), message: log.message().to_string() }, MAX_LOG_LINES);
        }
        for (id, error) in failures {
            self.record_error(id, error);
        }
        changed
    }

    fn record_error(&mut self, plugin_id: PluginId, error: PluginError) {
        push_bounded(&mut self.snapshot.errors, ErrorLine { at: chrono::Local::now(), plugin_id, error }, MAX_ERROR_LINES);
    }

    fn rescan(&mut self) {
        self.snapshot.scan_error = None;
        let found = match &self.plugins_dir {
            Some(dir) => match PluginManager::discover(dir) {
                Ok(found) => found,
                Err(error) => {
                    self.snapshot.scan_error = Some(error.to_string());
                    Vec::new()
                }
            },
            None => {
                self.snapshot.scan_error = Some("There is no plugins folder on this system.".into());
                Vec::new()
            }
        };
        let mut plugins: Vec<PluginView> = found.into_iter().map(|(directory, manifest)| describe(directory, manifest)).collect();
        for (id, directory) in &self.directories {
            if !plugins.iter().any(|plugin| plugin.id.as_ref() == Some(id)) {
                if let Some(manifest) = self.manager.as_ref().and_then(|manager| manager.manifest(id).ok()) {
                    plugins.push(describe(directory.clone(), Ok(manifest.clone())));
                }
            }
        }
        self.snapshot.plugins = plugins;
        self.refresh_states();
    }

    fn refresh_states(&mut self) {
        let Some(manager) = &self.manager else { return };
        for plugin in &mut self.snapshot.plugins {
            let Some(id) = &plugin.id else { continue };
            plugin.status = match manager.state(id) {
                Ok(PluginState::Loaded) => PluginStatus::Loaded,
                Ok(PluginState::Enabled) => PluginStatus::Enabled,
                Ok(PluginState::Failed(error)) => PluginStatus::Failed(error),
                Err(_) => PluginStatus::NotLoaded,
            };
            plugin.subscriptions = manager.subscriptions(id).unwrap_or_default();
        }
    }

    fn publish(&self) {
        let _ = self.events.send(BackendEvent::Luna(Box::new(self.snapshot.clone())));
    }
}

fn ok(message: impl Into<String>) -> ActionOutcome {
    ActionOutcome { ok: true, message: message.into() }
}

fn push_bounded<T>(lines: &mut VecDeque<T>, line: T, limit: usize) {
    if lines.len() >= limit {
        lines.pop_front();
    }
    lines.push_back(line);
}

fn describe(directory: PathBuf, manifest: Result<rustcord_plugins::PluginManifest, PluginError>) -> PluginView {
    let folder = directory.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    match manifest {
        Ok(manifest) => {
            let metadata = manifest.metadata();
            PluginView {
                directory,
                id: Some(metadata.id().clone()),
                name: metadata.name().to_string(),
                version: Some(metadata.version().to_string()),
                description: metadata.description().map(str::to_string),
                api: ApiStatus::Compatible(metadata.api_version()),
                status: PluginStatus::NotLoaded,
                subscriptions: Vec::new(),
            }
        }
        Err(PluginError::UnsupportedApi(version)) => PluginView {
            directory,
            id: None,
            name: folder,
            version: None,
            description: None,
            api: ApiStatus::Unsupported(version),
            status: PluginStatus::NotLoaded,
            subscriptions: Vec::new(),
        },
        Err(error) => PluginView {
            directory,
            id: None,
            name: folder,
            version: None,
            description: None,
            api: ApiStatus::Invalid(error.to_string()),
            status: PluginStatus::NotLoaded,
            subscriptions: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder(PathBuf);

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn folder(name: &str) -> Folder {
        let root = std::env::temp_dir().join(format!("rustcord-luna-host-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plugin = root.join("plugins/greeter");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("manifest.json"),
            r#"{"id":"test.greeter","name":"Greeter","version":"2.0.0","api_version":1,"entry":"main.lua","description":"Says hi"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("main.lua"),
            r#"luna.on("application.started", function() luna.log("hi") end)
local plugin = {}
function plugin.shutdown() luna.log("bye") end
return plugin"#,
        )
        .unwrap();
        let future = root.join("plugins/future");
        std::fs::create_dir_all(&future).unwrap();
        std::fs::write(future.join("manifest.json"), r#"{"api_version":99}"#).unwrap();
        Folder(root)
    }

    fn start(root: &Path, enabled: bool, plugins: &[&str]) -> (LunaHost, tokio::sync::mpsc::UnboundedReceiver<BackendEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let config = LunaConfig {
            plugins_dir: Some(root.join("plugins")),
            storage_dir: Some(root.join("data")),
            runtime_enabled: enabled,
            enabled_plugins: plugins.iter().map(|id| id.to_string()).collect(),
        };
        (LunaHost::start(config, tx), rx)
    }

    fn wait_for(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BackendEvent>, done: impl Fn(&LunaSnapshot) -> bool) -> LunaSnapshot {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            while let Ok(event) = rx.try_recv() {
                if let BackendEvent::Luna(snapshot) = event {
                    if done(&snapshot) {
                        return *snapshot;
                    }
                }
            }
            assert!(std::time::Instant::now() < deadline, "Luna never reached the expected state");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn greeter(snapshot: &LunaSnapshot) -> &PluginView {
        snapshot.plugins.iter().find(|plugin| plugin.name == "Greeter").unwrap()
    }

    #[test]
    fn startup_lists_plugins_enables_the_saved_ones_and_sends_started() {
        let root = folder("startup");
        let (host, mut rx) = start(&root.0, true, &["test.greeter"]);
        let snapshot = wait_for(&mut rx, |snapshot| snapshot.logs.iter().any(|line| line.message == "hi"));
        assert_eq!(snapshot.plugins.len(), 2);
        let view = greeter(&snapshot);
        assert!(matches!(view.status, PluginStatus::Enabled));
        assert_eq!(view.version.as_deref(), Some("2.0.0"));
        assert_eq!(view.description.as_deref(), Some("Says hi"));
        assert_eq!(view.subscriptions, vec!["application.started"]);
        let future = snapshot.plugins.iter().find(|plugin| plugin.name == "future").unwrap();
        assert!(matches!(future.api, ApiStatus::Unsupported(version) if version.get() == 99));
        assert_eq!(snapshot.enabled_count(), 1);
        host.shutdown();
    }

    #[test]
    fn requests_report_their_outcome_and_update_the_plugin_state() {
        let root = folder("requests");
        let (host, mut rx) = start(&root.0, true, &[]);
        let client = host.client();
        wait_for(&mut rx, |snapshot| !snapshot.plugins.is_empty());
        let id = PluginId::new("test.greeter").unwrap();
        assert!(client.send(LunaRequest::Enable(id.clone())));
        let snapshot = wait_for(&mut rx, |snapshot| matches!(greeter(snapshot).status, PluginStatus::Enabled));
        assert!(snapshot.last_action.as_ref().is_some_and(|action| action.ok));
        assert!(client.send(LunaRequest::Disable(id.clone())));
        let snapshot = wait_for(&mut rx, |snapshot| matches!(greeter(snapshot).status, PluginStatus::Loaded));
        assert!(snapshot.logs.iter().any(|line| line.message == "bye"));
        assert!(client.send(LunaRequest::Unload(id.clone())));
        wait_for(&mut rx, |snapshot| matches!(greeter(snapshot).status, PluginStatus::NotLoaded));
        assert!(client.send(LunaRequest::Enable(PluginId::new("test.missing").unwrap())));
        let snapshot = wait_for(&mut rx, |snapshot| snapshot.last_action.as_ref().is_some_and(|action| !action.ok));
        assert!(snapshot.errors.iter().any(|line| matches!(line.error, PluginError::NotFound(_))));
        host.shutdown();
    }

    #[test]
    fn turning_luna_off_stops_plugins_and_blocks_enabling() {
        let root = folder("toggle");
        let (host, mut rx) = start(&root.0, true, &["test.greeter"]);
        let client = host.client();
        wait_for(&mut rx, |snapshot| matches!(greeter(snapshot).status, PluginStatus::Enabled));
        assert!(client.send(LunaRequest::SetRuntimeEnabled(false)));
        let snapshot = wait_for(&mut rx, |snapshot| !snapshot.runtime_enabled);
        assert!(matches!(greeter(&snapshot).status, PluginStatus::Loaded));
        assert!(client.send(LunaRequest::Enable(PluginId::new("test.greeter").unwrap())));
        let snapshot = wait_for(&mut rx, |snapshot| snapshot.last_action.as_ref().is_some_and(|action| !action.ok));
        assert!(matches!(greeter(&snapshot).status, PluginStatus::Loaded));
        assert!(client.send(LunaRequest::SetRuntimeEnabled(true)));
        wait_for(&mut rx, |snapshot| matches!(greeter(snapshot).status, PluginStatus::Enabled));
        host.shutdown();
    }

    #[test]
    fn a_disabled_runtime_starts_nothing() {
        let root = folder("disabled");
        let (host, mut rx) = start(&root.0, false, &["test.greeter"]);
        let snapshot = wait_for(&mut rx, |snapshot| !snapshot.plugins.is_empty());
        assert!(!snapshot.runtime_enabled);
        assert!(matches!(greeter(&snapshot).status, PluginStatus::NotLoaded));
        host.shutdown();
    }
}
