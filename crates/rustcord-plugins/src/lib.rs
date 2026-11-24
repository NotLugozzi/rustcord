mod event;
mod worker;

pub use event::{ApplicationEvent, PluginEvent};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_ID_BYTES: usize = 128;
const MAX_STORAGE_BYTES: usize = 256 * 1024;
const MAX_KEY_BYTES: usize = 128;
const MAX_DESCRIPTION_BYTES: usize = 1024;

type Cause = Arc<dyn StdError + Send + Sync>;

#[derive(Clone, Debug)]
pub enum PluginError {
    InvalidId(String),
    Manifest {
        reason: String,
        source: Option<Cause>,
    },
    UnsupportedApi(PluginApiVersion),
    Io {
        action: &'static str,
        source: Arc<std::io::Error>,
    },
    Duplicate(PluginId),
    NotFound(PluginId),
    Runtime(String),
    Script(ScriptError),
    Storage {
        reason: String,
        source: Option<Cause>,
    },
    WorkerStopped,
    WorkerPanicked,
    EventQueueFull {
        plugin_id: PluginId,
        event: &'static str,
    },
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let with_cause = |formatter: &mut fmt::Formatter<'_>,
                          prefix: &str,
                          reason: &str,
                          cause: &Option<Cause>| match cause {
            Some(cause) => write!(formatter, "{prefix}{reason}: {cause}"),
            None => write!(formatter, "{prefix}{reason}"),
        };
        match self {
            Self::InvalidId(id) => write!(
                formatter,
                "invalid plugin id {id:?}: expected dot-separated lowercase ASCII identifiers"
            ),
            Self::Manifest { reason, source } => {
                with_cause(formatter, "invalid manifest: ", reason, source)
            }
            Self::UnsupportedApi(version) => write!(
                formatter,
                "plugin API version {version} is not supported; this host supports {}",
                PluginApiVersion::supported_range()
            ),
            Self::Io { action, source } => write!(formatter, "{action}: {source}"),
            Self::Duplicate(id) => write!(formatter, "plugin {id} is already loaded"),
            Self::NotFound(id) => write!(formatter, "plugin {id} is not loaded"),
            Self::Runtime(message) => {
                write!(formatter, "plugin runtime failed to start: {message}")
            }
            Self::Script(error) => write!(formatter, "{error}"),
            Self::Storage { reason, source } => {
                with_cause(formatter, "plugin storage error: ", reason, source)
            }
            Self::WorkerStopped => formatter.write_str("plugin worker stopped unexpectedly"),
            Self::WorkerPanicked => formatter.write_str("plugin worker panicked"),
            Self::EventQueueFull { plugin_id, event } => write!(
                formatter,
                "dropped {event} for plugin {plugin_id}: its event queue is full"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCategory {
    Manifest,
    UnsupportedApi,
    Io,
    Runtime,
    Script,
    Timeout,
    MemoryLimit,
    Storage,
    Worker,
    Lifecycle,
}

impl ErrorCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Manifest => "Manifest",
            Self::UnsupportedApi => "Unsupported API",
            Self::Io => "File access",
            Self::Runtime => "Runtime",
            Self::Script => "Script error",
            Self::Timeout => "Timeout",
            Self::MemoryLimit => "Memory limit",
            Self::Storage => "Storage",
            Self::Worker => "Worker",
            Self::Lifecycle => "Lifecycle",
        }
    }
}

impl PluginError {
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::InvalidId(_) | Self::Manifest { .. } => ErrorCategory::Manifest,
            Self::UnsupportedApi(_) => ErrorCategory::UnsupportedApi,
            Self::Io { .. } => ErrorCategory::Io,
            Self::Runtime(_) => ErrorCategory::Runtime,
            Self::Script(error) => match error.kind() {
                ScriptErrorKind::Error => ErrorCategory::Script,
                ScriptErrorKind::Timeout => ErrorCategory::Timeout,
                ScriptErrorKind::MemoryLimit => ErrorCategory::MemoryLimit,
            },
            Self::Storage { .. } => ErrorCategory::Storage,
            Self::WorkerStopped | Self::WorkerPanicked => ErrorCategory::Worker,
            Self::Duplicate(_) | Self::NotFound(_) | Self::EventQueueFull { .. } => {
                ErrorCategory::Lifecycle
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptErrorKind {
    Error,
    Timeout,
    MemoryLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptPhase {
    Load,
    Init,
    Event(&'static str),
    Shutdown,
}

impl fmt::Display for ScriptPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load => formatter.write_str("loading"),
            Self::Init => formatter.write_str("init"),
            Self::Event(name) => write!(formatter, "the {name} handler"),
            Self::Shutdown => formatter.write_str("shutdown"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    kind: ScriptErrorKind,
    phase: ScriptPhase,
    message: String,
    source: Option<String>,
    line: Option<u32>,
}

impl ScriptError {
    fn new(kind: ScriptErrorKind, phase: ScriptPhase, message: impl Into<String>) -> Self {
        Self {
            kind,
            phase,
            message: message.into(),
            source: None,
            line: None,
        }
    }

    pub fn kind(&self) -> ScriptErrorKind {
        self.kind
    }

    pub fn phase(&self) -> ScriptPhase {
        self.phase
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn line(&self) -> Option<u32> {
        self.line
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            ScriptErrorKind::Error => "script error",
            ScriptErrorKind::Timeout => "script timed out",
            ScriptErrorKind::MemoryLimit => "script ran out of memory",
        };
        write!(formatter, "{what} in {}", self.phase)?;
        if let (Some(source), Some(line)) = (&self.source, self.line) {
            write!(formatter, " at {source}:{line}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl StdError for PluginError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Manifest { source, .. } | Self::Storage { source, .. } => source
                .as_deref()
                .map(|cause| cause as &(dyn StdError + 'static)),
            Self::Io { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl PluginError {
    fn io(action: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Io {
            action,
            source: Arc::new(source),
        }
    }

    fn manifest(reason: impl Into<String>) -> Self {
        Self::Manifest {
            reason: reason.into(),
            source: None,
        }
    }

    fn manifest_caused(
        reason: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Manifest {
            reason: reason.into(),
            source: Some(Arc::new(source)),
        }
    }

    fn storage(reason: impl Into<String>) -> Self {
        Self::Storage {
            reason: reason.into(),
            source: None,
        }
    }

    fn storage_caused(
        reason: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Storage {
            reason: reason.into(),
            source: Some(Arc::new(source)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginApiVersion(u32);

impl PluginApiVersion {
    pub const CURRENT: Self = Self(1);
    pub const OLDEST_SUPPORTED: Self = Self(1);

    pub const fn new(version: u32) -> Self {
        Self(version)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub fn is_supported(self) -> bool {
        (Self::OLDEST_SUPPORTED..=Self::CURRENT).contains(&self)
    }

    fn supported_range() -> String {
        if Self::OLDEST_SUPPORTED == Self::CURRENT {
            format!("version {}", Self::CURRENT)
        } else {
            format!("versions {} to {}", Self::OLDEST_SUPPORTED, Self::CURRENT)
        }
    }
}

impl fmt::Display for PluginApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PluginId(String);

impl PluginId {
    pub fn new(id: impl Into<String>) -> Result<Self, PluginError> {
        let id = id.into();
        let valid = !id.is_empty()
            && id.len() <= MAX_ID_BYTES
            && id.split('.').all(|part| {
                !part.is_empty()
                    && part.as_bytes()[0].is_ascii_lowercase()
                    && part.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || byte == b'-'
                            || byte == b'_'
                    })
            });
        if valid {
            Ok(Self(id))
        } else {
            Err(PluginError::InvalidId(id))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PluginId {
    type Error = PluginError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl From<PluginId> for String {
    fn from(id: PluginId) -> Self {
        id.0
    }
}

impl FromStr for PluginId {
    type Err = PluginError;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::new(id)
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PluginMetadata {
    id: PluginId,
    name: String,
    version: String,
    api_version: PluginApiVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl PluginMetadata {
    pub fn id(&self) -> &PluginId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn api_version(&self) -> PluginApiVersion {
        self.api_version
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ManifestFile")]
pub struct PluginManifest {
    #[serde(flatten)]
    metadata: PluginMetadata,
    entry: String,
}

#[derive(Deserialize)]
struct ManifestFile {
    id: String,
    name: String,
    version: String,
    api_version: PluginApiVersion,
    entry: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct ManifestVersion {
    api_version: PluginApiVersion,
}

impl PluginManifest {
    pub fn from_json(json: &str) -> Result<Self, PluginError> {
        let version: ManifestVersion = serde_json::from_str(json).map_err(|error| {
            PluginError::manifest_caused("missing or malformed api_version", error)
        })?;
        if !version.api_version.is_supported() {
            return Err(PluginError::UnsupportedApi(version.api_version));
        }
        let file: ManifestFile = serde_json::from_str(json)
            .map_err(|error| PluginError::manifest_caused("malformed manifest", error))?;
        Self::try_from(file)
    }

    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    pub fn id(&self) -> &PluginId {
        &self.metadata.id
    }

    pub fn entry(&self) -> &str {
        &self.entry
    }

    fn read(plugin_dir: &Path) -> Result<Self, PluginError> {
        let contents = fs::read_to_string(plugin_dir.join("manifest.json"))
            .map_err(PluginError::io("reading plugin manifest"))?;
        Self::from_json(&contents)
    }
}

impl TryFrom<ManifestFile> for PluginManifest {
    type Error = PluginError;

    fn try_from(file: ManifestFile) -> Result<Self, Self::Error> {
        if !file.api_version.is_supported() {
            return Err(PluginError::UnsupportedApi(file.api_version));
        }
        let id = PluginId::new(file.id)
            .map_err(|error| PluginError::manifest_caused("invalid id", error))?;
        if file.name.trim().is_empty() || file.version.trim().is_empty() {
            return Err(PluginError::manifest("name and version must not be empty"));
        }
        let description = file
            .description
            .map(|description| description.trim().to_string())
            .filter(|description| !description.is_empty());
        if description
            .as_ref()
            .is_some_and(|description| description.len() > MAX_DESCRIPTION_BYTES)
        {
            return Err(PluginError::manifest("description is too long"));
        }
        let entry = Path::new(&file.entry);
        if file.entry.is_empty()
            || entry.is_absolute()
            || entry
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(PluginError::manifest(
                "entry must be a relative path without traversal components",
            ));
        }
        Ok(Self {
            metadata: PluginMetadata {
                id,
                name: file.name,
                version: file.version,
                api_version: file.api_version,
                description,
            },
            entry: file.entry,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginLogRecord {
    plugin_id: PluginId,
    message: String,
}

impl PluginLogRecord {
    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug)]
pub enum PluginState {
    Loaded,
    Enabled,
    Failed(PluginError),
}

impl PluginState {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Default, PartialEq)]
struct Storage {
    entries: BTreeMap<String, serde_json::Value>,
}

impl Storage {
    fn from_bytes(bytes: &[u8]) -> Result<Self, PluginError> {
        if bytes.len() > MAX_STORAGE_BYTES {
            return Err(PluginError::storage(
                "stored namespace exceeds the size limit",
            ));
        }
        serde_json::from_slice(bytes)
            .map(|entries| Self { entries })
            .map_err(|error| PluginError::storage_caused("stored namespace is invalid", error))
    }

    fn to_bytes(&self) -> Result<Vec<u8>, PluginError> {
        let bytes = serde_json::to_vec(&self.entries)
            .map_err(|error| PluginError::storage_caused("serializing namespace", error))?;
        if bytes.len() > MAX_STORAGE_BYTES {
            return Err(PluginError::storage("namespace exceeds the size limit"));
        }
        Ok(bytes)
    }

    fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.entries.get(key)
    }

    fn set(&mut self, key: String, value: serde_json::Value) -> Result<(), PluginError> {
        if key.is_empty()
            || key.len() > MAX_KEY_BYTES
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            return Err(PluginError::storage("storage key is invalid"));
        }
        let old = self.entries.insert(key.clone(), value);
        if let Err(error) = self.to_bytes() {
            if let Some(old) = old {
                self.entries.insert(key, old);
            } else {
                self.entries.remove(&key);
            }
            return Err(error);
        }
        Ok(())
    }
}

struct PluginRecord {
    manifest: PluginManifest,
    source: String,
    storage: Arc<Mutex<Storage>>,
    worker: Option<worker::WorkerHandle>,
    failure: Option<PluginError>,
}

pub struct PluginManager {
    storage_dir: PathBuf,
    plugins: BTreeMap<PluginId, PluginRecord>,
    logs: worker::Logs,
    errors: worker::Errors,
}

impl PluginManager {
    pub fn new(storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            storage_dir: storage_dir.into(),
            plugins: BTreeMap::new(),
            logs: Arc::new(Mutex::new(Vec::new())),
            errors: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn discover(
        plugins_dir: &Path,
    ) -> Result<Vec<(PathBuf, Result<PluginManifest, PluginError>)>, PluginError> {
        let mut found = Vec::new();
        for entry in
            fs::read_dir(plugins_dir).map_err(PluginError::io("reading plugins directory"))?
        {
            let entry = entry.map_err(PluginError::io("reading plugins directory"))?;
            let is_dir = entry
                .file_type()
                .map_err(PluginError::io("reading plugins directory"))?
                .is_dir();
            if is_dir && entry.path().join("manifest.json").is_file() {
                found.push((entry.path(), PluginManifest::read(&entry.path())));
            }
        }
        found.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(found)
    }

    pub fn load(&mut self, plugin_dir: &Path) -> Result<PluginId, PluginError> {
        let manifest = PluginManifest::read(plugin_dir)?;
        if self.plugins.contains_key(manifest.id()) {
            return Err(PluginError::Duplicate(manifest.id().clone()));
        }
        let root =
            fs::canonicalize(plugin_dir).map_err(PluginError::io("resolving plugin directory"))?;
        let entry = fs::canonicalize(root.join(&manifest.entry))
            .map_err(PluginError::io("resolving plugin entry"))?;
        if !entry.starts_with(&root) || !entry.is_file() {
            return Err(PluginError::manifest(
                "entry must resolve to a file inside the plugin directory",
            ));
        }
        let source = fs::read_to_string(entry).map_err(PluginError::io("reading plugin entry"))?;
        let storage_path = self.storage_dir.join(format!("{}.json", manifest.id()));
        let storage = if storage_path.exists() {
            let bytes = fs::read(&storage_path)
                .map_err(|error| PluginError::storage_caused("reading namespace", error))?;
            Storage::from_bytes(&bytes)?
        } else {
            Storage::default()
        };
        let id = manifest.id().clone();
        self.plugins.insert(
            id.clone(),
            PluginRecord {
                manifest,
                source,
                storage: Arc::new(Mutex::new(storage)),
                worker: None,
                failure: None,
            },
        );
        Ok(id)
    }

    pub fn enable(&mut self, id: &PluginId) -> Result<(), PluginError> {
        let record = self
            .plugins
            .get_mut(id)
            .ok_or_else(|| PluginError::NotFound(id.clone()))?;
        if record.worker.is_some() {
            return Ok(());
        }
        let spawned = worker::spawn(
            record.manifest.clone(),
            record.source.clone(),
            Arc::clone(&record.storage),
            Arc::clone(&self.logs),
            Arc::clone(&self.errors),
        );
        match spawned {
            Ok(handle) => {
                record.failure = None;
                record.worker = Some(handle);
                self.broadcast(
                    &PluginEvent::PluginEnabled {
                        plugin_id: id.clone(),
                    },
                    None,
                );
                Ok(())
            }
            Err(error) => {
                record.failure = Some(error.clone());
                let _ = persist_storage(&self.storage_dir, id, &record.storage);
                Err(error)
            }
        }
    }

    pub fn dispatch(&mut self, event: ApplicationEvent) {
        self.broadcast(&event.into(), None);
    }

    fn broadcast(&self, event: &PluginEvent, except: Option<&PluginId>) {
        for (id, record) in &self.plugins {
            if Some(id) == except {
                continue;
            }
            let Some(worker) = &record.worker else {
                continue;
            };
            if let worker::Delivery::QueueFull = worker.deliver(event) {
                worker::report(
                    &self.errors,
                    id,
                    PluginError::EventQueueFull {
                        plugin_id: id.clone(),
                        event: event.name(),
                    },
                );
            }
        }
    }

    pub fn poll(&mut self) -> Vec<(PluginId, PluginError)> {
        let mut stopped = Vec::new();
        for (id, record) in &mut self.plugins {
            let Some(exit) = record
                .worker
                .as_ref()
                .and_then(worker::WorkerHandle::exited)
            else {
                continue;
            };
            let finished = record
                .worker
                .take()
                .map_or(Ok(()), worker::WorkerHandle::finish);
            let error = match (exit, finished) {
                (_, Err(error)) | (Err(error), _) => error,
                (Ok(()), Ok(())) => PluginError::WorkerStopped,
            };
            record.failure = Some(error.clone());
            if let Err(storage_error) = persist_storage(&self.storage_dir, id, &record.storage) {
                stopped.push((id.clone(), storage_error));
            }
            stopped.push((id.clone(), error));
        }
        let mut failures: Vec<_> = self
            .errors
            .lock()
            .map(|mut errors| errors.drain(..).collect())
            .unwrap_or_default();
        failures.extend(stopped);
        failures
    }

    pub fn disable(&mut self, id: &PluginId) -> Result<(), PluginError> {
        if !self.plugins.contains_key(id) {
            return Err(PluginError::NotFound(id.clone()));
        }
        if self.plugins[id].worker.is_none() {
            return Ok(());
        }
        self.broadcast(
            &PluginEvent::PluginDisabled {
                plugin_id: id.clone(),
            },
            Some(id),
        );
        let Some(record) = self.plugins.get_mut(id) else {
            return Err(PluginError::NotFound(id.clone()));
        };
        let finished = record
            .worker
            .take()
            .map_or(Ok(()), worker::WorkerHandle::finish);
        let persisted = persist_storage(&self.storage_dir, id, &record.storage);
        if let Err(error) = &finished {
            record.failure = Some(error.clone());
        }
        finished.and(persisted)
    }

    pub fn shutdown(&mut self, grace: Duration) -> Vec<(PluginId, PluginError)> {
        for record in self.plugins.values() {
            if let Some(worker) = &record.worker {
                worker.request_stop();
            }
        }
        let deadline = Instant::now() + grace;
        for record in self.plugins.values() {
            if let Some(worker) = &record.worker {
                if !worker.wait(deadline.saturating_duration_since(Instant::now())) {
                    worker.cancel();
                }
            }
        }
        let mut failures = Vec::new();
        for (id, record) in &mut self.plugins {
            let Some(worker) = record.worker.take() else {
                continue;
            };
            if let Err(error) = worker.finish() {
                record.failure = Some(error.clone());
                failures.push((id.clone(), error));
            }
            if let Err(error) = persist_storage(&self.storage_dir, id, &record.storage) {
                failures.push((id.clone(), error));
            }
        }
        let mut reported = self.poll();
        reported.extend(failures);
        reported
    }

    pub fn unload(&mut self, id: &PluginId) -> Result<(), PluginError> {
        if !self.plugins.contains_key(id) {
            return Err(PluginError::NotFound(id.clone()));
        }
        let disabled = self.disable(id);
        self.plugins.remove(id);
        disabled
    }

    pub fn state(&self, id: &PluginId) -> Result<PluginState, PluginError> {
        let record = self
            .plugins
            .get(id)
            .ok_or_else(|| PluginError::NotFound(id.clone()))?;
        Ok(if record.worker.is_some() {
            PluginState::Enabled
        } else if let Some(error) = &record.failure {
            PluginState::Failed(error.clone())
        } else {
            PluginState::Loaded
        })
    }

    pub fn manifest(&self, id: &PluginId) -> Result<&PluginManifest, PluginError> {
        self.plugins
            .get(id)
            .map(|record| &record.manifest)
            .ok_or_else(|| PluginError::NotFound(id.clone()))
    }

    pub fn subscriptions(&self, id: &PluginId) -> Result<Vec<&'static str>, PluginError> {
        let record = self
            .plugins
            .get(id)
            .ok_or_else(|| PluginError::NotFound(id.clone()))?;
        Ok(record
            .worker
            .as_ref()
            .map(worker::WorkerHandle::subscriptions)
            .unwrap_or_default())
    }

    pub fn take_logs(&self) -> Vec<PluginLogRecord> {
        self.logs
            .lock()
            .map(|mut logs| std::mem::take(&mut *logs))
            .unwrap_or_default()
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        for (id, record) in &mut self.plugins {
            if let Some(worker) = record.worker.take() {
                let _ = worker.finish();
                let _ = persist_storage(&self.storage_dir, id, &record.storage);
            }
        }
    }
}

fn persist_storage(
    root: &Path,
    id: &PluginId,
    storage: &Mutex<Storage>,
) -> Result<(), PluginError> {
    let bytes = storage
        .lock()
        .map_err(|_| PluginError::storage("storage lock poisoned"))?
        .to_bytes()?;
    fs::create_dir_all(root)
        .and_then(|()| fs::write(root.join(format!("{id}.json")), bytes))
        .map_err(|error| PluginError::storage_caused("writing namespace", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    const MANIFEST: &str = r#"{"id":"example.hello","name":"Hello","version":"1.0.0","api_version":1,"entry":"main.lua"}"#;

    struct Fixture {
        root: PathBuf,
        plugin: PathBuf,
        storage: PathBuf,
    }

    impl Fixture {
        fn new(source: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "rustcord-luna-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            let plugin = root.join("plugins/hello");
            let storage = root.join("storage");
            fs::create_dir_all(&plugin).unwrap();
            fs::write(plugin.join("manifest.json"), MANIFEST).unwrap();
            fs::write(plugin.join("main.lua"), source).unwrap();
            Self {
                root,
                plugin,
                storage,
            }
        }

        fn rename(&self, id: &str) {
            let manifest = MANIFEST.replace("example.hello", id);
            fs::write(self.plugin.join("manifest.json"), manifest).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn id(id: &str) -> PluginId {
        PluginId::new(id).unwrap()
    }

    fn manifest_json(field: &str, value: &str) -> String {
        let mut json: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
        json[field] = serde_json::from_str(value).unwrap();
        json.to_string()
    }

    fn enabled(source: &str) -> (Fixture, PluginManager, PluginId) {
        let fixture = Fixture::new(source);
        let mut manager = PluginManager::new(&fixture.storage);
        let plugin = manager.load(&fixture.plugin).unwrap();
        manager.enable(&plugin).unwrap();
        (fixture, manager, plugin)
    }

    fn enable_error(source: &str) -> (PluginError, PluginState) {
        let fixture = Fixture::new(source);
        let mut manager = PluginManager::new(&fixture.storage);
        let plugin = manager.load(&fixture.plugin).unwrap();
        let error = manager.enable(&plugin).unwrap_err();
        (error, manager.state(&plugin).unwrap())
    }

    fn script(error: &PluginError) -> &ScriptError {
        match error {
            PluginError::Script(script) => script,
            other => panic!("expected a script error, got {other:?}"),
        }
    }

    #[derive(Default)]
    struct Seen {
        logs: Vec<String>,
        errors: Vec<(PluginId, PluginError)>,
    }

    fn watch(manager: &mut PluginManager, timeout: Duration, done: impl Fn(&Seen) -> bool) -> Seen {
        let deadline = Instant::now() + timeout;
        let mut seen = Seen::default();
        loop {
            seen.errors.extend(manager.poll());
            seen.logs.extend(
                manager
                    .take_logs()
                    .into_iter()
                    .map(|log| format!("{}: {}", log.plugin_id(), log.message())),
            );
            if done(&seen) {
                return seen;
            }
            assert!(
                Instant::now() < deadline,
                "timed out with logs {:?} and errors {:?}",
                seen.logs,
                seen.errors
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn plugin_ids_are_validated() {
        for valid in ["example", "example.hello", "a1.b-c.d_e"] {
            assert_eq!(id(valid).as_str(), valid);
        }
        let too_long = format!("a{}", "b".repeat(MAX_ID_BYTES));
        for invalid in [
            "",
            ".",
            "example.",
            "example..hello",
            "Example.hello",
            "1example",
            "example.-hello",
            "example/hello",
            "../example",
            "example hello",
            too_long.as_str(),
        ] {
            assert!(
                matches!(PluginId::new(invalid), Err(PluginError::InvalidId(ref rejected)) if rejected == invalid),
                "{invalid:?} should be rejected"
            );
        }
        assert_eq!(
            "example.hello".parse::<PluginId>().unwrap(),
            id("example.hello")
        );
        assert_eq!(id("example.hello").to_string(), "example.hello");
        assert_eq!(String::from(id("example.hello")), "example.hello");
    }

    #[test]
    fn plugin_ids_serialize_as_validated_strings() {
        assert_eq!(
            serde_json::to_string(&id("example.hello")).unwrap(),
            r#""example.hello""#
        );
        assert_eq!(
            serde_json::from_str::<PluginId>(r#""example.hello""#).unwrap(),
            id("example.hello")
        );
        assert!(serde_json::from_str::<PluginId>(r#""../escape""#).is_err());
    }

    #[test]
    fn manifests_round_trip_and_expose_validated_fields() {
        let manifest = PluginManifest::from_json(MANIFEST).unwrap();
        assert_eq!(manifest.id(), &id("example.hello"));
        assert_eq!(manifest.metadata().id(), &id("example.hello"));
        assert_eq!(manifest.metadata().name(), "Hello");
        assert_eq!(manifest.metadata().version(), "1.0.0");
        assert_eq!(manifest.metadata().api_version(), PluginApiVersion::CURRENT);
        assert_eq!(manifest.metadata().description(), None);
        assert_eq!(manifest.entry(), "main.lua");
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(PluginManifest::from_json(&json).unwrap(), manifest);
        assert_eq!(
            serde_json::from_str::<PluginManifest>(&json).unwrap(),
            manifest
        );
        let described =
            PluginManifest::from_json(&manifest_json("description", r#""  Says hello  ""#))
                .unwrap();
        assert_eq!(described.metadata().description(), Some("Says hello"));
        let json = serde_json::to_string(&described).unwrap();
        assert_eq!(PluginManifest::from_json(&json).unwrap(), described);
        let blank = PluginManifest::from_json(&manifest_json("description", r#""   ""#)).unwrap();
        assert_eq!(blank.metadata().description(), None);
    }

    #[test]
    fn manifests_ignore_unknown_fields() {
        let json = manifest_json("homepage", r#""added by a later host""#);
        assert!(PluginManifest::from_json(&json).is_ok());
    }

    #[test]
    fn manifests_reject_invalid_fields() {
        let long_description = format!("{:?}", "d".repeat(MAX_DESCRIPTION_BYTES + 1));
        for (field, value) in [
            ("id", r#""../bad""#),
            ("id", "5"),
            ("name", r#""  ""#),
            ("version", r#""""#),
            ("entry", r#""../main.lua""#),
            ("entry", r#""/main.lua""#),
            ("entry", r#""./main.lua""#),
            ("entry", r#""""#),
            ("description", "5"),
            ("description", long_description.as_str()),
        ] {
            let json = manifest_json(field, value);
            assert!(
                matches!(
                    PluginManifest::from_json(&json),
                    Err(PluginError::Manifest { .. })
                ),
                "{field} = {value} should be rejected"
            );
            assert!(serde_json::from_str::<PluginManifest>(&json).is_err());
        }
        assert!(matches!(
            PluginManifest::from_json("{"),
            Err(PluginError::Manifest { .. })
        ));
        assert!(matches!(
            PluginManifest::from_json(
                r#"{"id":"example.hello","name":"Hello","version":"1","entry":"main.lua"}"#
            ),
            Err(PluginError::Manifest { .. })
        ));
    }

    #[test]
    fn unsupported_api_versions_are_reported_before_schema_errors() {
        for version in [0, 2, u32::MAX] {
            let json = manifest_json("api_version", &version.to_string());
            assert!(matches!(
                PluginManifest::from_json(&json),
                Err(PluginError::UnsupportedApi(rejected)) if rejected.get() == version
            ));
        }
        assert!(matches!(
            PluginManifest::from_json(r#"{"api_version":2,"id":5,"entrypoints":[]}"#),
            Err(PluginError::UnsupportedApi(rejected)) if rejected.get() == 2
        ));
        assert_eq!(
            PluginError::UnsupportedApi(PluginApiVersion::new(2)).to_string(),
            "plugin API version 2 is not supported; this host supports version 1"
        );
    }

    #[test]
    fn api_versions_follow_the_supported_range() {
        assert!(PluginApiVersion::CURRENT.is_supported());
        assert!(PluginApiVersion::OLDEST_SUPPORTED.is_supported());
        assert!(PluginApiVersion::OLDEST_SUPPORTED <= PluginApiVersion::CURRENT);
        assert!(!PluginApiVersion::new(0).is_supported());
        assert!(!PluginApiVersion::new(PluginApiVersion::CURRENT.get() + 1).is_supported());
        assert_eq!(PluginApiVersion::new(7).to_string(), "7");
        assert_eq!(
            serde_json::to_string(&PluginApiVersion::new(7)).unwrap(),
            "7"
        );
        assert_eq!(
            serde_json::from_str::<PluginApiVersion>("7").unwrap(),
            PluginApiVersion::new(7)
        );
        assert!(serde_json::from_str::<PluginApiVersion>(r#""1""#).is_err());
        assert!(serde_json::from_str::<PluginApiVersion>("-1").is_err());
    }

    #[test]
    fn manifests_serialize_as_a_flat_document() {
        let manifest = PluginManifest::from_json(MANIFEST).unwrap();
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            json,
            serde_json::from_str::<serde_json::Value>(MANIFEST).unwrap()
        );
        let metadata = serde_json::to_value(manifest.metadata()).unwrap();
        assert!(metadata.get("entry").is_none());
        assert_eq!(metadata["api_version"], 1);
    }

    #[test]
    fn discovery_reports_each_plugin_separately() {
        let fixture = Fixture::new("return nil");
        let broken = fixture.root.join("plugins/broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(
            broken.join("manifest.json"),
            manifest_json("api_version", "9"),
        )
        .unwrap();
        fs::create_dir_all(fixture.root.join("plugins/not-a-plugin")).unwrap();
        let found = PluginManager::discover(&fixture.root.join("plugins")).unwrap();
        assert_eq!(found.len(), 2);
        assert!(matches!(
            &found[0],
            (path, Err(PluginError::UnsupportedApi(_))) if path.ends_with("broken")
        ));
        assert!(matches!(
            &found[1],
            (path, Ok(manifest)) if path.ends_with("hello") && manifest.id() == &id("example.hello")
        ));
        assert!(matches!(
            PluginManager::discover(&fixture.root.join("missing")),
            Err(PluginError::Io { .. })
        ));
    }

    #[test]
    fn storage_round_trips_and_enforces_limits() {
        let mut storage = Storage::default();
        storage
            .set("count".into(), serde_json::json!({ "value": 5 }))
            .unwrap();
        let bytes = storage.to_bytes().unwrap();
        assert_eq!(bytes, br#"{"count":{"value":5}}"#);
        assert_eq!(Storage::from_bytes(&bytes).unwrap(), storage);
        for key in ["", "has space", "slash/key", &"k".repeat(MAX_KEY_BYTES + 1)] {
            assert!(matches!(
                storage.set(key.into(), serde_json::Value::Null),
                Err(PluginError::Storage { .. })
            ));
        }
        let oversized = serde_json::Value::String("x".repeat(MAX_STORAGE_BYTES));
        assert!(storage.set("count".into(), oversized.clone()).is_err());
        assert_eq!(
            storage.get("count"),
            Some(&serde_json::json!({ "value": 5 }))
        );
        assert!(storage.set("other".into(), oversized).is_err());
        assert_eq!(storage.get("other"), None);
        assert!(matches!(
            Storage::from_bytes(b"[1, 2]"),
            Err(PluginError::Storage { .. })
        ));
        assert!(matches!(
            Storage::from_bytes(&vec![b' '; MAX_STORAGE_BYTES + 1]),
            Err(PluginError::Storage { .. })
        ));
    }

    #[test]
    fn corrupt_storage_fails_load_without_being_overwritten() {
        let fixture = Fixture::new("return nil");
        fs::create_dir_all(&fixture.storage).unwrap();
        let path = fixture.storage.join("example.hello.json");
        fs::write(&path, "not json").unwrap();
        let mut manager = PluginManager::new(&fixture.storage);
        assert!(matches!(
            manager.load(&fixture.plugin),
            Err(PluginError::Storage { .. })
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), "not json");
    }

    #[test]
    fn io_errors_keep_their_source() {
        let fixture = Fixture::new("return nil");
        let mut manager = PluginManager::new(&fixture.storage);
        let error = manager.load(&fixture.root.join("missing")).unwrap_err();
        let PluginError::Io { action, source } = &error else {
            panic!("expected an I/O error, got {error:?}");
        };
        assert_eq!(*action, "reading plugin manifest");
        assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(error.category(), ErrorCategory::Io);
        assert!(std::error::Error::source(&error)
            .and_then(|cause| cause.downcast_ref::<std::io::Error>())
            .is_some());
    }

    #[test]
    fn manifest_errors_keep_their_cause() {
        let error = PluginManifest::from_json(&manifest_json("id", r#""../bad""#)).unwrap_err();
        let cause = std::error::Error::source(&error).expect("invalid id should be the cause");
        assert!(cause.to_string().contains("../bad"));
        assert!(error
            .to_string()
            .starts_with("invalid manifest: invalid id: "));
        assert_eq!(error.category(), ErrorCategory::Manifest);
        let malformed = PluginManifest::from_json("{").unwrap_err();
        assert!(std::error::Error::source(&malformed).is_some());
        let missing = PluginManifest::from_json(r#"{"id":"example.hello"}"#).unwrap_err();
        assert!(matches!(missing, PluginError::Manifest { .. }));
        assert!(missing.to_string().contains("api_version"));
    }

    #[test]
    fn storage_errors_keep_their_cause() {
        let corrupt = Storage::from_bytes(b"not json").unwrap_err();
        assert!(matches!(corrupt, PluginError::Storage { .. }));
        assert_eq!(corrupt.category(), ErrorCategory::Storage);
        assert!(std::error::Error::source(&corrupt).is_some());
        assert!(corrupt
            .to_string()
            .starts_with("plugin storage error: stored namespace is invalid: "));
        let invalid_key = Storage::default()
            .set("bad key".into(), serde_json::Value::Null)
            .unwrap_err();
        assert!(std::error::Error::source(&invalid_key).is_none());
        assert_eq!(
            invalid_key.to_string(),
            "plugin storage error: storage key is invalid"
        );
        let fixture = Fixture::new("return nil");
        fs::write(
            &fixture.storage,
            "a file where the storage directory should be",
        )
        .unwrap();
        let storage = Mutex::new(Storage::default());
        let error = persist_storage(&fixture.storage, &id("example.hello"), &storage).unwrap_err();
        assert!(matches!(error, PluginError::Storage { .. }));
        assert!(std::error::Error::source(&error)
            .and_then(|cause| cause.downcast_ref::<std::io::Error>())
            .is_some());
    }

    #[test]
    fn lifecycle_transitions() {
        let fixture = Fixture::new(
            r#"if luna.storage.get("ran") then return end; luna.storage.set("ran", true); error("first run")"#,
        );
        let hello = id("example.hello");
        let mut manager = PluginManager::new(&fixture.storage);
        assert!(matches!(
            manager.state(&hello),
            Err(PluginError::NotFound(_))
        ));
        assert!(matches!(
            manager.enable(&hello),
            Err(PluginError::NotFound(_))
        ));
        assert_eq!(manager.load(&fixture.plugin).unwrap(), hello);
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
        manager.disable(&hello).unwrap();
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
        let error = manager.enable(&hello).unwrap_err();
        assert!(script(&error).message().contains("first run"));
        assert!(
            matches!(manager.state(&hello).unwrap(), PluginState::Failed(PluginError::Script(failure)) if failure.message().contains("first run"))
        );
        manager.enable(&hello).unwrap();
        assert!(manager.state(&hello).unwrap().is_enabled());
        manager.enable(&hello).unwrap();
        manager.disable(&hello).unwrap();
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
        manager.unload(&hello).unwrap();
        assert!(matches!(
            manager.state(&hello),
            Err(PluginError::NotFound(_))
        ));
        assert!(matches!(
            manager.unload(&hello),
            Err(PluginError::NotFound(_))
        ));
    }

    #[test]
    fn executes_logs_and_exposes_metadata_and_storage() {
        let (_fixture, mut manager, hello) = enabled(
            r#"luna.log(luna.plugin.name); luna.storage.set("count", 5); assert(luna.storage.get("count") == 5)"#,
        );
        assert!(manager.state(&hello).unwrap().is_enabled());
        assert_eq!(
            manager.take_logs(),
            vec![PluginLogRecord {
                plugin_id: hello.clone(),
                message: "Hello".into()
            }]
        );
        manager.disable(&hello).unwrap();
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
    }

    #[test]
    fn log_records_expose_their_plugin_and_message() {
        let (_fixture, manager, hello) = enabled(r#"luna.log("api " .. luna.plugin.api_version)"#);
        let logs = manager.take_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].plugin_id(), &hello);
        assert_eq!(logs[0].message(), "api 1");
        assert!(manager.take_logs().is_empty());
    }

    #[test]
    fn rejects_duplicate_ids_and_isolates_storage() {
        let first = Fixture::new(r#"luna.storage.set("secret", 42)"#);
        let second = Fixture::new(r#"assert(luna.storage.get("secret") == nil)"#);
        second.rename("example.other");
        let hello = id("example.hello");
        let other = id("example.other");
        let mut manager = PluginManager::new(&first.storage);
        manager.load(&first.plugin).unwrap();
        assert!(matches!(
            manager.load(&first.plugin),
            Err(PluginError::Duplicate(duplicate)) if duplicate == hello
        ));
        manager.enable(&hello).unwrap();
        manager.load(&second.plugin).unwrap();
        manager.enable(&other).unwrap();
        manager.disable(&other).unwrap();
        manager.disable(&hello).unwrap();
    }

    #[test]
    fn storage_survives_manager_restart_and_remains_plugin_scoped() {
        let writer = Fixture::new(r#"luna.storage.set("persisted", { value = 73 })"#);
        let reader = Fixture::new(r#"assert(luna.storage.get("persisted") == nil)"#);
        reader.rename("example.reader");
        let hello = id("example.hello");
        let reader_id = id("example.reader");
        {
            let mut manager = PluginManager::new(&writer.storage);
            manager.load(&writer.plugin).unwrap();
            manager.enable(&hello).unwrap();
            manager.unload(&hello).unwrap();
        }
        let verifier = Fixture::new(r#"assert(luna.storage.get("persisted").value == 73)"#);
        let mut manager = PluginManager::new(&writer.storage);
        manager.load(&verifier.plugin).unwrap();
        manager.load(&reader.plugin).unwrap();
        manager.enable(&hello).unwrap();
        manager.enable(&reader_id).unwrap();
        assert!(manager.poll().is_empty());
    }

    #[test]
    fn scripts_cannot_reach_restricted_libraries() {
        let (_fixture, mut manager, _) =
            enabled("assert(io == nil and os == nil and package == nil and debug == nil)");
        assert!(manager.poll().is_empty());
    }

    #[test]
    fn script_errors_carry_their_phase_source_and_line() {
        let (error, state) = enable_error("local x = 1\n\nerror('broken')");
        let failure = script(&error);
        assert_eq!(failure.kind(), ScriptErrorKind::Error);
        assert_eq!(failure.phase(), ScriptPhase::Load);
        assert_eq!(failure.source(), Some("main.lua"));
        assert_eq!(failure.line(), Some(3));
        assert!(failure.message().contains("broken"));
        assert_eq!(error.category(), ErrorCategory::Script);
        assert!(error
            .to_string()
            .starts_with("script error in loading at main.lua:3: "));
        assert!(matches!(state, PluginState::Failed(PluginError::Script(_))));
        let (error, _) = enable_error(
            "local plugin = {}\nfunction plugin.init()\n  local t = nil\n  return t.field\nend\nreturn plugin",
        );
        let failure = script(&error);
        assert_eq!(failure.phase(), ScriptPhase::Init);
        assert_eq!(failure.line(), Some(4));
        let (error, _) = enable_error("local = ");
        let failure = script(&error);
        assert_eq!(failure.kind(), ScriptErrorKind::Error);
        assert_eq!(failure.phase(), ScriptPhase::Load);
        assert_eq!(failure.line(), None);
        let (error, _) = enable_error("return 5");
        assert!(script(&error).message().contains("must return a table"));
        let (error, _) = enable_error("return { init = 5 }");
        assert!(script(&error)
            .message()
            .contains("plugin.init must be a function"));
    }

    #[test]
    fn script_storage_errors_surface_as_script_failures() {
        let (error, state) = enable_error("\nluna.storage.set(\"bad key\", 1)");
        let failure = script(&error);
        assert!(failure.message().contains("storage key is invalid"));
        assert_eq!(failure.line(), Some(2));
        let PluginState::Failed(state_error) = state else {
            panic!("plugin should have failed");
        };
        assert_eq!(state_error.to_string(), error.to_string());
        assert!(!PluginState::Failed(state_error).is_enabled());
    }

    #[test]
    fn a_runaway_entry_script_times_out_even_inside_pcall() {
        let started = Instant::now();
        let (error, state) = enable_error("\npcall(function()\n  while true do end\nend)");
        assert!(started.elapsed() < worker::CALL_LIMIT + Duration::from_secs(2));
        let failure = script(&error);
        assert_eq!(failure.kind(), ScriptErrorKind::Timeout);
        assert_eq!(failure.line(), Some(3));
        assert_eq!(error.category(), ErrorCategory::Timeout);
        assert!(matches!(state, PluginState::Failed(_)));
    }

    #[test]
    fn callbacks_may_not_yield() {
        let (error, _) = enable_error("coroutine.yield()");
        assert!(script(&error).message().contains("must not yield"));
    }

    #[test]
    fn running_out_of_memory_is_reported_as_such() {
        let (error, _) = enable_error(
            "local plugin = {}\nfunction plugin.init()\n  local t = {}\n  for i = 1, 1e9 do t[i] = string.rep('x', 4096) .. i end\nend\nreturn plugin",
        );
        let failure = script(&error);
        assert_eq!(failure.kind(), ScriptErrorKind::MemoryLimit);
        assert_eq!(failure.phase(), ScriptPhase::Init);
        assert_eq!(error.category(), ErrorCategory::MemoryLimit);
    }

    #[test]
    fn event_names_are_validated() {
        assert_eq!(
            event::supported_name("application.started"),
            Ok("application.started")
        );
        for invalid in [
            "",
            "Application.Started",
            "application..started",
            "a b",
            ".",
        ] {
            assert!(event::supported_name(invalid)
                .unwrap_err()
                .starts_with("invalid event name"));
        }
        assert!(event::supported_name("message.created")
            .unwrap_err()
            .starts_with("unsupported event"));
        let (error, _) = enable_error(r#"luna.on("Message Created", function() end)"#);
        assert!(script(&error).message().contains("invalid event name"));
        let (error, _) = enable_error(r#"luna.on("message.created", function() end)"#);
        assert!(script(&error).message().contains("unsupported event"));
        let (error, _) = enable_error(r#"luna.on("application.started", 5)"#);
        assert_eq!(script(&error).kind(), ScriptErrorKind::Error);
    }

    #[test]
    fn event_registration_is_bounded() {
        let (_fixture, manager, hello) = enabled(&format!(
            "for i = 1, {} do luna.on('application.started', function() end) end",
            worker::MAX_SUBSCRIPTIONS
        ));
        assert_eq!(
            manager.subscriptions(&hello).unwrap(),
            vec!["application.started"]
        );
        let (error, _) = enable_error(&format!(
            "for i = 1, {} do luna.on('application.started', function() end) end",
            worker::MAX_SUBSCRIPTIONS + 1
        ));
        assert!(script(&error).message().contains("at most"));
    }

    #[test]
    fn events_reach_handlers_registered_in_init_with_their_payload() {
        let (_fixture, mut manager, hello) = enabled(
            r#"
local plugin = {}
function plugin.init()
  luna.on("application.started", function(event)
    luna.log("got " .. event.name)
  end)
end
return plugin
"#,
        );
        assert_eq!(
            manager.subscriptions(&hello).unwrap(),
            vec!["application.started"]
        );
        manager.dispatch(ApplicationEvent::Started);
        let seen = watch(&mut manager, Duration::from_secs(2), |seen| {
            !seen.logs.is_empty()
        });
        assert_eq!(seen.logs, vec!["example.hello: got application.started"]);
        assert!(seen.errors.is_empty());
        manager.dispatch(ApplicationEvent::Shutdown);
        manager.disable(&hello).unwrap();
        assert!(manager.subscriptions(&hello).unwrap().is_empty());
        assert!(manager.take_logs().is_empty());
    }

    #[test]
    fn plugins_hear_each_other_enable_and_disable() {
        let watcher = Fixture::new(
            r#"
luna.on("plugin.enabled", function(event) luna.log("enabled " .. event.plugin_id) end)
luna.on("plugin.disabled", function(event) luna.log("disabled " .. event.plugin_id) end)
"#,
        );
        let other = Fixture::new("return nil");
        other.rename("example.other");
        let mut manager = PluginManager::new(&watcher.storage);
        let hello = manager.load(&watcher.plugin).unwrap();
        let other_id = manager.load(&other.plugin).unwrap();
        manager.enable(&hello).unwrap();
        manager.enable(&other_id).unwrap();
        manager.disable(&other_id).unwrap();
        let seen = watch(&mut manager, Duration::from_secs(2), |seen| {
            seen.logs.len() == 3
        });
        assert_eq!(
            seen.logs,
            vec![
                "example.hello: enabled example.hello",
                "example.hello: enabled example.other",
                "example.hello: disabled example.other",
            ]
        );
    }

    #[test]
    fn independent_plugins_each_receive_events_and_one_failing_does_not_stop_the_other() {
        let failing = Fixture::new(
            "luna.on('application.started', function()\n  error('handler broke')\nend)",
        );
        let healthy = Fixture::new(
            r#"luna.on("application.started", function() luna.log("still fine") end)"#,
        );
        healthy.rename("example.healthy");
        let mut manager = PluginManager::new(&failing.storage);
        let failing_id = manager.load(&failing.plugin).unwrap();
        let healthy_id = manager.load(&healthy.plugin).unwrap();
        manager.enable(&failing_id).unwrap();
        manager.enable(&healthy_id).unwrap();
        manager.dispatch(ApplicationEvent::Started);
        manager.dispatch(ApplicationEvent::Started);
        let seen = watch(&mut manager, Duration::from_secs(2), |seen| {
            seen.logs.len() == 2 && seen.errors.len() == 2
        });
        assert!(seen
            .logs
            .iter()
            .all(|log| log == "example.healthy: still fine"));
        for (plugin, error) in &seen.errors {
            assert_eq!(plugin, &failing_id);
            let failure = script(error);
            assert_eq!(failure.kind(), ScriptErrorKind::Error);
            assert_eq!(failure.phase(), ScriptPhase::Event("application.started"));
            assert_eq!(failure.line(), Some(2));
            assert!(failure.message().contains("handler broke"));
        }
        assert!(manager.state(&failing_id).unwrap().is_enabled());
        assert!(manager.state(&healthy_id).unwrap().is_enabled());
    }

    #[test]
    fn a_runaway_handler_times_out_and_the_plugin_keeps_working() {
        let (_fixture, mut manager, hello) = enabled(
            "local calls = 0\nluna.on('application.started', function()\n  calls = calls + 1\n  if calls == 1 then\n    while true do end\n  end\n  luna.log('recovered')\nend)",
        );
        manager.dispatch(ApplicationEvent::Started);
        manager.dispatch(ApplicationEvent::Started);
        let seen = watch(
            &mut manager,
            worker::CALL_LIMIT + Duration::from_secs(3),
            |seen| !seen.logs.is_empty() && !seen.errors.is_empty(),
        );
        let failure = script(&seen.errors[0].1);
        assert_eq!(failure.kind(), ScriptErrorKind::Timeout);
        assert_eq!(failure.phase(), ScriptPhase::Event("application.started"));
        assert_eq!(failure.line(), Some(5));
        assert_eq!(seen.logs, vec!["example.hello: recovered"]);
        assert!(manager.state(&hello).unwrap().is_enabled());
    }

    #[test]
    fn event_queues_are_bounded_and_disable_drops_pending_events() {
        let (_fixture, mut manager, hello) = enabled(
            "luna.on('application.started', function() luna.log('started'); while true do end end)",
        );
        manager.dispatch(ApplicationEvent::Started);
        watch(&mut manager, Duration::from_secs(2), |seen| {
            !seen.logs.is_empty()
        });
        for _ in 0..worker::EVENT_QUEUE_CAPACITY + 5 {
            manager.dispatch(ApplicationEvent::Started);
        }
        let errors = manager.poll();
        assert_eq!(errors.len(), 5);
        assert!(errors.iter().all(|(plugin, error)| plugin == &hello
            && matches!(
                error,
                PluginError::EventQueueFull {
                    event: "application.started",
                    ..
                }
            )
            && error.category() == ErrorCategory::Lifecycle));
        let started = Instant::now();
        manager.disable(&hello).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
        assert!(manager.take_logs().is_empty());
        assert!(manager.poll().is_empty());
    }

    #[test]
    fn disabling_runs_the_shutdown_callback_and_saves_storage() {
        let fixture = Fixture::new(
            r#"
local plugin = {}
function plugin.shutdown()
  luna.storage.set("stopped", true)
  luna.log("bye")
end
return plugin
"#,
        );
        let mut manager = PluginManager::new(&fixture.storage);
        let hello = manager.load(&fixture.plugin).unwrap();
        manager.enable(&hello).unwrap();
        manager.disable(&hello).unwrap();
        assert_eq!(
            manager
                .take_logs()
                .iter()
                .map(PluginLogRecord::message)
                .collect::<Vec<_>>(),
            vec!["bye"]
        );
        let saved = fs::read_to_string(fixture.storage.join("example.hello.json")).unwrap();
        assert_eq!(saved, r#"{"stopped":true}"#);
    }

    #[test]
    fn a_runaway_shutdown_callback_cannot_block_disable() {
        let (_fixture, mut manager, hello) = enabled(
            "local plugin = {}\nfunction plugin.shutdown()\n  while true do end\nend\nreturn plugin",
        );
        let started = Instant::now();
        manager.disable(&hello).unwrap();
        assert!(started.elapsed() < worker::SHUTDOWN_LIMIT + Duration::from_secs(1));
        let errors = manager.poll();
        assert!(matches!(
            errors.as_slice(),
            [(_, PluginError::Script(failure))] if failure.kind() == ScriptErrorKind::Timeout
                && failure.phase() == ScriptPhase::Shutdown
                && failure.line() == Some(3)
        ));
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
    }

    #[test]
    fn application_shutdown_is_delivered_before_the_shutdown_callback() {
        let (_fixture, mut manager, hello) = enabled(
            r#"
local plugin = {}
function plugin.init()
  luna.on("application.shutdown", function() luna.log("application is closing") end)
end
function plugin.shutdown() luna.log("plugin stopped") end
return plugin
"#,
        );
        manager.dispatch(ApplicationEvent::Shutdown);
        assert!(manager.shutdown(Duration::from_secs(2)).is_empty());
        assert_eq!(
            manager
                .take_logs()
                .iter()
                .map(PluginLogRecord::message)
                .collect::<Vec<_>>(),
            vec!["application is closing", "plugin stopped"]
        );
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
    }

    #[test]
    fn shutdown_gives_up_on_a_busy_plugin_after_the_grace_period() {
        let (_fixture, mut manager, hello) =
            enabled("luna.on('application.shutdown', function() while true do end end)");
        manager.dispatch(ApplicationEvent::Shutdown);
        thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        manager.shutdown(Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            manager.state(&hello).unwrap(),
            PluginState::Loaded
        ));
    }

    #[test]
    fn re_enabling_starts_with_fresh_subscriptions() {
        let (_fixture, mut manager, hello) =
            enabled(r#"luna.on("application.started", function() luna.log("once") end)"#);
        manager.disable(&hello).unwrap();
        manager.enable(&hello).unwrap();
        manager.dispatch(ApplicationEvent::Started);
        let seen = watch(&mut manager, Duration::from_secs(2), |seen| {
            !seen.logs.is_empty()
        });
        thread::sleep(Duration::from_millis(50));
        let mut logs = seen.logs;
        logs.extend(
            manager
                .take_logs()
                .into_iter()
                .map(|log| format!("{}: {}", log.plugin_id(), log.message())),
        );
        assert_eq!(logs, vec!["example.hello: once"]);
    }

    #[test]
    fn events_serialize_to_owned_payloads() {
        assert_eq!(
            serde_json::to_value(PluginEvent::PluginEnabled {
                plugin_id: id("example.hello")
            })
            .unwrap(),
            serde_json::json!({ "name": "plugin.enabled", "plugin_id": "example.hello" })
        );
        assert_eq!(
            serde_json::to_value(PluginEvent::from(ApplicationEvent::Started)).unwrap(),
            serde_json::json!({ "name": "application.started" })
        );
        for event in [
            PluginEvent::ApplicationStarted,
            PluginEvent::ApplicationShutdown,
            PluginEvent::PluginEnabled { plugin_id: id("a") },
            PluginEvent::PluginDisabled { plugin_id: id("a") },
        ] {
            assert_eq!(serde_json::to_value(&event).unwrap()["name"], event.name());
        }
    }
}
