use mlua::{Lua, LuaOptions, LuaSerdeExt, StdLib, Value, VmState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

pub const API_VERSION: u32 = 1;
const MAX_STORAGE_BYTES: usize = 256 * 1024;
const MAX_KEY_BYTES: usize = 128;
const MAX_LOG_BYTES: usize = 4096;
const MAX_PENDING_LOGS: usize = 4096;
const MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const INTERRUPT_LIMIT: u64 = 5_000_000;
const EXECUTION_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid manifest: {0}")]
    Manifest(String),
    #[error("unsupported plugin API version {0}")]
    UnsupportedApi(u32),
    #[error("plugin {0} is already loaded")]
    Duplicate(String),
    #[error("plugin {0} is not loaded")]
    NotFound(String),
    #[error("plugin {0} is not enabled")]
    NotEnabled(String),
    #[error("Lua runtime error: {0}")]
    Lua(#[from] mlua::Error),
    #[error("plugin worker has stopped")]
    WorkerStopped,
    #[error("plugin worker panicked")]
    WorkerPanicked,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("plugin execution failed: {0}")]
    Execution(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub entry: String,
}

impl PluginManifest {
    pub fn read(plugin_dir: &Path) -> Result<Self, PluginError> {
        let manifest_path = plugin_dir.join("manifest.json");
        let contents = fs::read_to_string(manifest_path)?;
        let manifest: Self = serde_json::from_str(&contents)
            .map_err(|error| PluginError::Manifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), PluginError> {
        if !valid_id(&self.id) {
            return Err(PluginError::Manifest(
                "plugin id must contain dot-separated lowercase ASCII identifiers".into(),
            ));
        }
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(PluginError::Manifest(
                "name and version must not be empty".into(),
            ));
        }
        if self.api_version != API_VERSION {
            return Err(PluginError::UnsupportedApi(self.api_version));
        }
        let entry = Path::new(&self.entry);
        if self.entry.is_empty()
            || entry.is_absolute()
            || entry
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(PluginError::Manifest(
                "entry must be a relative path without traversal components".into(),
            ));
        }
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.split('.').all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'
                        || byte == b'_'
                })
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginLog {
    pub plugin_id: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginState {
    Discovered,
    Loaded,
    Enabled,
    Failed(String),
}

struct RunningPlugin {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

struct PluginRecord {
    manifest: PluginManifest,
    source: String,
    storage: Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
    running: Option<RunningPlugin>,
    failure: Option<String>,
}

pub struct PluginManager {
    storage_dir: PathBuf,
    plugins: BTreeMap<String, PluginRecord>,
    logs: Arc<Mutex<Vec<PluginLog>>>,
}

impl PluginManager {
    pub fn new(storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            storage_dir: storage_dir.into(),
            plugins: BTreeMap::new(),
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn discover(&self, plugins_dir: &Path) -> Result<Vec<PluginManifest>, PluginError> {
        let mut manifests = Vec::new();
        for entry in fs::read_dir(plugins_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && entry.path().join("manifest.json").is_file() {
                manifests.push(PluginManifest::read(&entry.path())?);
            }
        }
        manifests.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(manifests)
    }

    pub fn load(&mut self, plugin_dir: &Path) -> Result<String, PluginError> {
        let manifest = PluginManifest::read(plugin_dir)?;
        if self.plugins.contains_key(&manifest.id) {
            return Err(PluginError::Duplicate(manifest.id));
        }
        let root = fs::canonicalize(plugin_dir)?;
        let entry = fs::canonicalize(root.join(&manifest.entry))?;
        if !entry.starts_with(&root) || !entry.is_file() {
            return Err(PluginError::Manifest(
                "entry must resolve to a file inside the plugin directory".into(),
            ));
        }
        let source = fs::read_to_string(entry)?;
        let storage_path = self.storage_dir.join(format!("{}.json", manifest.id));
        let storage = if storage_path.exists() {
            let bytes = fs::read(&storage_path)?;
            if bytes.len() > MAX_STORAGE_BYTES {
                return Err(PluginError::Storage(
                    "stored namespace exceeds the size limit".into(),
                ));
            }
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        let id = manifest.id.clone();
        self.plugins.insert(
            id.clone(),
            PluginRecord {
                manifest,
                source,
                storage: Arc::new(Mutex::new(storage)),
                running: None,
                failure: None,
            },
        );
        Ok(id)
    }

    pub fn enable(&mut self, id: &str) -> Result<(), PluginError> {
        let record = self
            .plugins
            .get_mut(id)
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        if record.running.is_some() {
            return Ok(());
        }
        let manifest = record.manifest.clone();
        let source = record.source.clone();
        let storage = Arc::clone(&record.storage);
        let logs = Arc::clone(&self.logs);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(format!("luna-{}", id))
            .spawn(move || {
                let result =
                    execute_plugin(&manifest, &source, storage, logs, Arc::clone(&worker_stop));
                let _ = ready_tx.send(result.map_err(|error| error.to_string()));
                while !worker_stop.load(Ordering::Acquire) {
                    thread::park_timeout(Duration::from_millis(20));
                }
            })?;
        match ready_rx.recv().map_err(|_| PluginError::WorkerStopped)? {
            Ok(()) => {
                record.failure = None;
                record.running = Some(RunningPlugin {
                    stop,
                    worker: Some(worker),
                });
                Ok(())
            }
            Err(error) => {
                stop.store(true, Ordering::Release);
                worker.join().map_err(|_| PluginError::WorkerPanicked)?;
                record.failure = Some(error.clone());
                Err(PluginError::Execution(error))
            }
        }
    }

    pub fn disable(&mut self, id: &str) -> Result<(), PluginError> {
        let record = self
            .plugins
            .get_mut(id)
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        let Some(mut running) = record.running.take() else {
            return Ok(());
        };
        running.stop.store(true, Ordering::Release);
        if let Some(worker) = running.worker.take() {
            worker.join().map_err(|_| PluginError::WorkerPanicked)?;
        }
        persist_storage(&self.storage_dir, id, &record.storage)
    }

    pub fn unload(&mut self, id: &str) -> Result<(), PluginError> {
        if !self.plugins.contains_key(id) {
            return Err(PluginError::NotFound(id.into()));
        }
        self.disable(id)?;
        self.plugins.remove(id);
        Ok(())
    }

    pub fn state(&self, id: &str) -> Result<PluginState, PluginError> {
        let record = self
            .plugins
            .get(id)
            .ok_or_else(|| PluginError::NotFound(id.into()))?;
        Ok(if record.running.is_some() {
            PluginState::Enabled
        } else if let Some(error) = &record.failure {
            PluginState::Failed(error.clone())
        } else {
            PluginState::Loaded
        })
    }

    pub fn manifest(&self, id: &str) -> Result<&PluginManifest, PluginError> {
        self.plugins
            .get(id)
            .map(|record| &record.manifest)
            .ok_or_else(|| PluginError::NotFound(id.into()))
    }

    pub fn take_logs(&self) -> Vec<PluginLog> {
        self.logs
            .lock()
            .map(|mut logs| std::mem::take(&mut *logs))
            .unwrap_or_default()
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        for (id, record) in &mut self.plugins {
            if let Some(mut running) = record.running.take() {
                running.stop.store(true, Ordering::Release);
                if let Some(worker) = running.worker.take() {
                    let _ = worker.join();
                }
                let _ = persist_storage(&self.storage_dir, id, &record.storage);
            }
        }
    }
}

fn persist_storage(
    root: &Path,
    id: &str,
    storage: &Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
) -> Result<(), PluginError> {
    let data = storage
        .lock()
        .map_err(|_| PluginError::Storage("storage lock poisoned".into()))?;
    let bytes =
        serde_json::to_vec(&*data).map_err(|error| PluginError::Storage(error.to_string()))?;
    if bytes.len() > MAX_STORAGE_BYTES {
        return Err(PluginError::Storage(
            "namespace exceeds the size limit".into(),
        ));
    }
    fs::create_dir_all(root)?;
    fs::write(root.join(format!("{id}.json")), bytes)?;
    Ok(())
}

fn execute_plugin(
    manifest: &PluginManifest,
    source: &str,
    storage: Arc<Mutex<BTreeMap<String, serde_json::Value>>>,
    logs: Arc<Mutex<Vec<PluginLog>>>,
    stop: Arc<AtomicBool>,
) -> Result<(), PluginError> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::new(),
    )?;
    lua.set_memory_limit(MEMORY_LIMIT_BYTES)?;
    let globals = lua.globals();
    let luna = lua.create_table()?;
    let log_id = manifest.id.clone();
    let log_stop = Arc::clone(&stop);
    luna.set(
        "log",
        lua.create_function(move |_, message: String| {
            if !log_stop.load(Ordering::Acquire) {
                if message.len() > MAX_LOG_BYTES {
                    return Err(mlua::Error::RuntimeError(
                        "log message exceeds the size limit".into(),
                    ));
                }
                let mut logs = logs
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("logger unavailable".into()))?;
                if logs.len() >= MAX_PENDING_LOGS {
                    return Err(mlua::Error::RuntimeError(
                        "pending log limit exceeded".into(),
                    ));
                }
                logs.push(PluginLog {
                    plugin_id: log_id.clone(),
                    message,
                });
            }
            Ok(())
        })?,
    )?;
    let metadata = lua.create_table()?;
    metadata.set("id", manifest.id.as_str())?;
    metadata.set("name", manifest.name.as_str())?;
    metadata.set("version", manifest.version.as_str())?;
    metadata.set("api_version", manifest.api_version)?;
    luna.set("plugin", metadata)?;
    let storage_api = lua.create_table()?;
    let set_storage = Arc::clone(&storage);
    let set_stop = Arc::clone(&stop);
    storage_api.set(
        "set",
        lua.create_function(move |lua, (key, value): (String, Value)| {
            if set_stop.load(Ordering::Acquire) {
                return Err(mlua::Error::RuntimeError("plugin is stopping".into()));
            }
            if key.is_empty()
                || key.len() > MAX_KEY_BYTES
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
            {
                return Err(mlua::Error::RuntimeError(
                    "storage key length is invalid".into(),
                ));
            }
            let json: serde_json::Value = lua.from_value(value)?;
            let mut entries = set_storage
                .lock()
                .map_err(|_| mlua::Error::RuntimeError("storage unavailable".into()))?;
            let old = entries.insert(key.clone(), json);
            let size = serde_json::to_vec(&*entries)
                .map_err(mlua::Error::external)?
                .len();
            if size > MAX_STORAGE_BYTES {
                if let Some(old) = old {
                    entries.insert(key, old);
                } else {
                    entries.remove(&key);
                }
                return Err(mlua::Error::RuntimeError(
                    "plugin storage size limit exceeded".into(),
                ));
            }
            Ok(())
        })?,
    )?;
    let get_storage = Arc::clone(&storage);
    storage_api.set(
        "get",
        lua.create_function(move |lua, key: String| {
            let entries = get_storage
                .lock()
                .map_err(|_| mlua::Error::RuntimeError("storage unavailable".into()))?;
            match entries.get(&key) {
                Some(value) => lua.to_value(value),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    luna.set("storage", storage_api)?;
    globals.set("luna", luna)?;
    lua.sandbox(true)?;
    let started = Instant::now();
    let hook_stop = Arc::clone(&stop);
    let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    lua.set_interrupt(move |_| {
        if hook_stop.load(Ordering::Acquire) {
            return Err(mlua::Error::RuntimeError("plugin stopped".into()));
        }
        if started.elapsed() >= EXECUTION_LIMIT
            || count.fetch_add(1, Ordering::Relaxed) >= INTERRUPT_LIMIT
        {
            return Err(mlua::Error::RuntimeError(
                "plugin execution limit exceeded".into(),
            ));
        }
        Ok(VmState::Continue)
    });
    lua.load(source).set_name(&manifest.entry).exec()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

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
            fs::write(plugin.join("manifest.json"), r#"{"id":"example.hello","name":"Hello","version":"1.0.0","api_version":1,"entry":"main.lua"}"#).unwrap();
            fs::write(plugin.join("main.lua"), source).unwrap();
            Self {
                root,
                plugin,
                storage,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn executes_logs_and_exposes_metadata_and_storage() {
        let fixture = Fixture::new(
            r#"luna.log(luna.plugin.name); luna.storage.set("count", 5); assert(luna.storage.get("count") == 5)"#,
        );
        let mut manager = PluginManager::new(&fixture.storage);
        manager.load(&fixture.plugin).unwrap();
        assert_eq!(manager.state("example.hello").unwrap(), PluginState::Loaded);
        manager.enable("example.hello").unwrap();
        assert_eq!(
            manager.state("example.hello").unwrap(),
            PluginState::Enabled
        );
        assert_eq!(manager.take_logs()[0].message, "Hello");
        manager.disable("example.hello").unwrap();
        assert_eq!(manager.state("example.hello").unwrap(), PluginState::Loaded);
    }

    #[test]
    fn validates_manifest_and_entry_paths() {
        let mut manifest = PluginManifest {
            id: "example.hello".into(),
            name: "Hello".into(),
            version: "1".into(),
            api_version: 1,
            entry: "../main.lua".into(),
        };
        assert!(manifest.validate().is_err());
        manifest.entry = "main.lua".into();
        manifest.id = "../bad".into();
        assert!(manifest.validate().is_err());
        manifest.id = "example.hello".into();
        manifest.api_version = 2;
        assert!(matches!(
            manifest.validate(),
            Err(PluginError::UnsupportedApi(2))
        ));
    }

    #[test]
    fn rejects_duplicate_ids_and_isolates_storage() {
        let first = Fixture::new(r#"luna.storage.set("secret", 42)"#);
        let second = Fixture::new(r#"assert(luna.storage.get("secret") == nil)"#);
        let mut manager = PluginManager::new(&first.storage);
        manager.load(&first.plugin).unwrap();
        assert!(matches!(
            manager.load(&first.plugin),
            Err(PluginError::Duplicate(_))
        ));
        manager.enable("example.hello").unwrap();
        manager.disable("example.hello").unwrap();
        let plugin2 = second.plugin.join("manifest.json");
        let contents = fs::read_to_string(plugin2)
            .unwrap()
            .replace("example.hello", "example.other");
        fs::write(second.plugin.join("manifest.json"), contents).unwrap();
        manager.load(&second.plugin).unwrap();
        manager.enable("example.other").unwrap();
        manager.disable("example.other").unwrap();
    }

    #[test]
    fn reports_script_errors_and_restricts_libraries() {
        let fixture =
            Fixture::new("assert(io == nil and os == nil and package == nil and debug == nil)");
        let mut manager = PluginManager::new(&fixture.storage);
        manager.load(&fixture.plugin).unwrap();
        manager.enable("example.hello").unwrap();
        manager.disable("example.hello").unwrap();
        let broken = Fixture::new("error('broken')");
        let mut broken_manager = PluginManager::new(&broken.storage);
        broken_manager.load(&broken.plugin).unwrap();
        assert!(matches!(
            broken_manager.enable("example.hello"),
            Err(PluginError::Execution(message)) if message.contains("broken")
        ));
        assert!(matches!(
            broken_manager.state("example.hello").unwrap(),
            PluginState::Failed(_)
        ));
    }

    #[test]
    fn stops_infinite_loops_and_joins_worker() {
        let fixture = Fixture::new("while true do end");
        let mut manager = PluginManager::new(&fixture.storage);
        manager.load(&fixture.plugin).unwrap();
        assert!(manager.enable("example.hello").is_err());
        assert!(matches!(
            manager.state("example.hello").unwrap(),
            PluginState::Failed(_)
        ));
    }
}
