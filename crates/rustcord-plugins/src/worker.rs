use crate::event::{self, PluginEvent};
use crate::{
    PluginError, PluginId, PluginLogRecord, PluginManifest, ScriptError, ScriptErrorKind,
    ScriptPhase, Storage,
};
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value, VmState};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) const EVENT_QUEUE_CAPACITY: usize = 32;
pub(crate) const MAX_SUBSCRIPTIONS: usize = 64;
pub(crate) const MAX_PENDING_ERRORS: usize = 256;
const MAX_LOG_BYTES: usize = 4096;
const MAX_PENDING_LOGS: usize = 4096;
const MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const INTERRUPT_LIMIT: u64 = 1_000_000_000;
pub(crate) const CALL_LIMIT: Duration = Duration::from_secs(2);
pub(crate) const SHUTDOWN_LIMIT: Duration = Duration::from_secs(1);
const IDLE_WAIT: Duration = Duration::from_millis(20);

pub(crate) type Logs = Arc<Mutex<Vec<PluginLogRecord>>>;
pub(crate) type Errors = Arc<Mutex<VecDeque<(PluginId, PluginError)>>>;

pub(crate) fn report(errors: &Errors, plugin_id: &PluginId, error: PluginError) {
    if let Ok(mut errors) = errors.lock() {
        if errors.len() >= MAX_PENDING_ERRORS {
            errors.pop_front();
        }
        errors.push_back((plugin_id.clone(), error));
    }
}

pub(crate) enum Envelope {
    Event(PluginEvent),
    Stop,
}

#[derive(Default)]
pub(crate) struct Shared {
    cancel: AtomicBool,
    subscriptions: Mutex<BTreeSet<&'static str>>,
}

impl Shared {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

pub(crate) struct WorkerHandle {
    events: mpsc::SyncSender<Envelope>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    done: mpsc::Receiver<Result<(), PluginError>>,
}

pub(crate) enum Delivery {
    Queued,
    NotSubscribed,
    QueueFull,
    Closed,
}

impl WorkerHandle {
    pub(crate) fn deliver(&self, event: &PluginEvent) -> Delivery {
        let subscribed = self
            .shared
            .subscriptions
            .lock()
            .is_ok_and(|subscriptions| subscriptions.contains(event.name()));
        if !subscribed {
            return Delivery::NotSubscribed;
        }
        match self.events.try_send(Envelope::Event(event.clone())) {
            Ok(()) => Delivery::Queued,
            Err(mpsc::TrySendError::Full(_)) => Delivery::QueueFull,
            Err(mpsc::TrySendError::Disconnected(_)) => Delivery::Closed,
        }
    }

    pub(crate) fn subscriptions(&self) -> Vec<&'static str> {
        self.shared
            .subscriptions
            .lock()
            .map(|subscriptions| subscriptions.iter().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Release);
    }

    pub(crate) fn request_stop(&self) {
        if self.events.try_send(Envelope::Stop).is_err() {
            self.cancel();
        }
    }

    pub(crate) fn exited(&self) -> Option<Result<(), PluginError>> {
        match self.done.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(PluginError::WorkerStopped)),
        }
    }

    pub(crate) fn wait(&self, timeout: Duration) -> bool {
        !matches!(
            self.done.recv_timeout(timeout),
            Err(mpsc::RecvTimeoutError::Timeout)
        )
    }

    pub(crate) fn finish(mut self) -> Result<(), PluginError> {
        self.cancel();
        let joined = match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| PluginError::WorkerPanicked),
            None => Ok(()),
        };
        joined?;
        match self.done.try_recv() {
            Ok(result) => result,
            Err(_) => Ok(()),
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.cancel();
            let _ = thread.join();
        }
    }
}

pub(crate) fn spawn(
    manifest: PluginManifest,
    source: String,
    storage: Arc<Mutex<Storage>>,
    logs: Logs,
    errors: Errors,
) -> Result<WorkerHandle, PluginError> {
    let (events_tx, events_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    let shared = Arc::new(Shared::default());
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name(format!("luna-{}", manifest.id()))
        .spawn(move || {
            let result = run(
                &manifest,
                &source,
                storage,
                logs,
                errors,
                worker_shared,
                events_rx,
                ready_tx,
            );
            let _ = done_tx.send(result);
        })
        .map_err(|error| PluginError::Runtime(format!("spawning worker: {error}")))?;
    let handle = WorkerHandle {
        events: events_tx,
        shared,
        thread: Some(thread),
        done: done_rx,
    };
    match ready_rx.recv() {
        Ok(()) => Ok(handle),
        Err(_) => match handle.finish() {
            Ok(()) => Err(PluginError::WorkerStopped),
            Err(error) => Err(error),
        },
    }
}

fn run(
    manifest: &PluginManifest,
    source: &str,
    storage: Arc<Mutex<Storage>>,
    logs: Logs,
    errors: Errors,
    shared: Arc<Shared>,
    events: mpsc::Receiver<Envelope>,
    ready: mpsc::Sender<()>,
) -> Result<(), PluginError> {
    let id = manifest.id().clone();
    let runtime = Runtime::new(manifest, storage, logs, Arc::clone(&shared))
        .map_err(|error| PluginError::Runtime(error.to_string()))?;
    let entry = runtime
        .lua
        .load(source)
        .set_name(format!("@{}", manifest.entry()))
        .into_function()
        .map_err(|error| {
            PluginError::Script(ScriptError::new(
                ScriptErrorKind::Error,
                ScriptPhase::Load,
                describe(&error),
            ))
        })?;
    let exported = runtime
        .call(ScriptPhase::Load, &entry, None)
        .map_err(Failure::into_error)?;
    let plugin = match exported {
        Value::Nil => None,
        Value::Table(table) => Some(table),
        other => {
            return Err(PluginError::Script(ScriptError::new(
                ScriptErrorKind::Error,
                ScriptPhase::Load,
                format!(
                    "the entry script must return a table or nothing, not a {}",
                    other.type_name()
                ),
            )))
        }
    };
    if let Some(init) = hook(plugin.as_ref(), "init")? {
        runtime
            .call(ScriptPhase::Init, &init, None)
            .map_err(Failure::into_error)?;
    }
    let _ = ready.send(());
    loop {
        if shared.cancelled() {
            break;
        }
        match events.recv_timeout(IDLE_WAIT) {
            Ok(Envelope::Event(event)) => {
                runtime.deliver(&event, |error| report(&errors, &id, error))
            }
            Ok(Envelope::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    drop(events);
    if let Some(shutdown) = hook(plugin.as_ref(), "shutdown")? {
        if let Err(Failure::Error(error)) = runtime.call(ScriptPhase::Shutdown, &shutdown, None) {
            report(&errors, &id, error);
        }
    }
    Ok(())
}

fn hook(plugin: Option<&Table>, name: &str) -> Result<Option<Function>, PluginError> {
    let Some(plugin) = plugin else {
        return Ok(None);
    };
    match plugin.raw_get::<Value>(name) {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::Function(function)) => Ok(Some(function)),
        Ok(other) => Err(PluginError::Script(ScriptError::new(
            ScriptErrorKind::Error,
            ScriptPhase::Load,
            format!(
                "plugin.{name} must be a function, not a {}",
                other.type_name()
            ),
        ))),
        Err(error) => Err(PluginError::Script(ScriptError::new(
            ScriptErrorKind::Error,
            ScriptPhase::Load,
            describe(&error),
        ))),
    }
}

enum Failure {
    Cancelled,
    Error(PluginError),
}

impl Failure {
    fn into_error(self) -> PluginError {
        match self {
            Self::Cancelled => PluginError::WorkerStopped,
            Self::Error(error) => error,
        }
    }
}

#[derive(Clone)]
struct Location {
    source: String,
    line: u32,
}

enum Trip {
    Cancelled,
    Timeout(Option<Location>),
}

struct Watchdog {
    shared: Arc<Shared>,
    debug_info: Function,
    deadline: Cell<Option<Instant>>,
    honour_cancel: Cell<bool>,
    ticks: Cell<u64>,
    trip: RefCell<Option<Trip>>,
}

impl Watchdog {
    fn arm(&self, limit: Duration, honour_cancel: bool) {
        self.deadline.set(Some(Instant::now() + limit));
        self.honour_cancel.set(honour_cancel);
        self.ticks.set(0);
        self.trip.replace(None);
    }

    fn disarm(&self) -> Option<Trip> {
        self.deadline.set(None);
        self.trip.take()
    }

    fn check(&self, lua: &Lua) -> mlua::Result<VmState> {
        if self.trip.borrow().is_some() {
            return Err(mlua::Error::RuntimeError("plugin stopped".into()));
        }
        if self.honour_cancel.get() && self.shared.cancelled() {
            self.trip.replace(Some(Trip::Cancelled));
            return Err(mlua::Error::RuntimeError("plugin cancelled".into()));
        }
        let Some(deadline) = self.deadline.get() else {
            return Ok(VmState::Continue);
        };
        let ticks = self.ticks.get() + 1;
        self.ticks.set(ticks);
        if ticks >= INTERRUPT_LIMIT || Instant::now() >= deadline {
            let location = locate(&self.debug_info, &lua.current_thread());
            self.trip.replace(Some(Trip::Timeout(location)));
            return Err(mlua::Error::RuntimeError(
                "execution time limit exceeded".into(),
            ));
        }
        Ok(VmState::Continue)
    }
}

fn locate(debug_info: &Function, thread: &mlua::Thread) -> Option<Location> {
    for level in 0..32 {
        let (source, line) = debug_info
            .call::<(Option<String>, Option<i64>)>((thread.clone(), level, "sl"))
            .ok()?;
        let (Some(source), Some(line)) = (source, line) else {
            return None;
        };
        if line > 0 && source != "[C]" {
            return Some(Location {
                source,
                line: u32::try_from(line).ok()?,
            });
        }
    }
    None
}

fn innermost(error: &mlua::Error) -> &mlua::Error {
    match error {
        mlua::Error::CallbackError { cause, .. } => innermost(cause),
        mlua::Error::WithContext { cause, .. } => innermost(cause),
        other => other,
    }
}

fn describe(error: &mlua::Error) -> String {
    match innermost(error) {
        mlua::Error::RuntimeError(message) => message
            .split_once("\nstack traceback:")
            .map_or(message.as_str(), |(message, _)| message)
            .to_string(),
        mlua::Error::MemoryError(message) => message.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::ExternalError(external) => external.to_string(),
        other => other.to_string(),
    }
}

struct Runtime {
    lua: Lua,
    debug_info: Function,
    handlers: Table,
    watchdog: Rc<Watchdog>,
    shared: Arc<Shared>,
}

impl Runtime {
    fn new(
        manifest: &PluginManifest,
        storage: Arc<Mutex<Storage>>,
        logs: Logs,
        shared: Arc<Shared>,
    ) -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE
                | StdLib::STRING
                | StdLib::MATH
                | StdLib::UTF8
                | StdLib::COROUTINE
                | StdLib::DEBUG,
            LuaOptions::new(),
        )?;
        lua.set_memory_limit(MEMORY_LIMIT_BYTES)?;
        let luna = lua.create_table()?;
        let log_id = manifest.id().clone();
        luna.set(
            "log",
            lua.create_function(move |_, message: String| {
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
                logs.push(PluginLogRecord {
                    plugin_id: log_id.clone(),
                    message,
                });
                Ok(())
            })?,
        )?;
        let metadata = lua.create_table()?;
        let plugin = manifest.metadata();
        metadata.set("id", plugin.id().as_str())?;
        metadata.set("name", plugin.name())?;
        metadata.set("version", plugin.version())?;
        metadata.set("api_version", plugin.api_version().get())?;
        luna.set("plugin", metadata)?;
        let storage_api = lua.create_table()?;
        let set_storage = Arc::clone(&storage);
        storage_api.set(
            "set",
            lua.create_function(move |lua, (key, value): (String, Value)| {
                let json: serde_json::Value = lua.from_value(value)?;
                set_storage
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("storage unavailable".into()))?
                    .set(key, json)
                    .map_err(|error| mlua::Error::RuntimeError(error.to_string()))
            })?,
        )?;
        storage_api.set(
            "get",
            lua.create_function(move |lua, key: String| {
                let entries = storage
                    .lock()
                    .map_err(|_| mlua::Error::RuntimeError("storage unavailable".into()))?;
                match entries.get(&key) {
                    Some(value) => lua.to_value(value),
                    None => Ok(Value::Nil),
                }
            })?,
        )?;
        luna.set("storage", storage_api)?;
        let handlers = lua.create_table()?;
        let registered = Rc::new(Cell::new(0usize));
        let subscribe_handlers = handlers.clone();
        let subscribe_shared = Arc::clone(&shared);
        luna.set(
            "on",
            lua.create_function(move |lua, (name, callback): (String, Function)| {
                let name = event::supported_name(&name).map_err(mlua::Error::RuntimeError)?;
                if registered.get() >= MAX_SUBSCRIPTIONS {
                    return Err(mlua::Error::RuntimeError(format!(
                        "a plugin can register at most {MAX_SUBSCRIPTIONS} event handlers"
                    )));
                }
                let list = match subscribe_handlers.raw_get::<Value>(name)? {
                    Value::Table(list) => list,
                    _ => {
                        let list = lua.create_table()?;
                        subscribe_handlers.raw_set(name, &list)?;
                        list
                    }
                };
                list.raw_set(list.raw_len() + 1, callback)?;
                registered.set(registered.get() + 1);
                if let Ok(mut subscriptions) = subscribe_shared.subscriptions.lock() {
                    subscriptions.insert(name);
                }
                Ok(())
            })?,
        )?;
        lua.globals().set("luna", luna)?;
        let debug_info: Function = lua.globals().get::<Table>("debug")?.get("info")?;
        lua.globals().raw_set("debug", Value::Nil)?;
        lua.sandbox(true)?;
        let watchdog = Rc::new(Watchdog {
            shared: Arc::clone(&shared),
            debug_info: debug_info.clone(),
            deadline: Cell::new(None),
            honour_cancel: Cell::new(true),
            ticks: Cell::new(0),
            trip: RefCell::new(None),
        });
        let interrupt_watchdog = Rc::clone(&watchdog);
        lua.set_interrupt(move |lua| interrupt_watchdog.check(lua));
        Ok(Self {
            lua,
            debug_info,
            handlers,
            watchdog,
            shared,
        })
    }

    fn call(
        &self,
        phase: ScriptPhase,
        function: &Function,
        argument: Option<Value>,
    ) -> Result<Value, Failure> {
        let limit = if phase == ScriptPhase::Shutdown {
            SHUTDOWN_LIMIT
        } else {
            CALL_LIMIT
        };
        self.watchdog.arm(limit, phase != ScriptPhase::Shutdown);
        let thread = self.lua.create_thread(function.clone());
        let result = match &thread {
            Ok(thread) => match argument {
                Some(argument) => thread.resume::<Value>(argument),
                None => thread.resume::<Value>(()),
            },
            Err(error) => Err(error.clone()),
        };
        let trip = self.watchdog.disarm();
        let mut location = thread
            .as_ref()
            .ok()
            .and_then(|thread| locate(&self.debug_info, thread));
        if let Some(Trip::Timeout(Some(tripped))) = &trip {
            location = Some(tripped.clone());
        }
        let failure = |kind, message: String| {
            let mut error = ScriptError::new(kind, phase, message);
            if let Some(location) = location.clone() {
                error.source = Some(location.source);
                error.line = Some(location.line);
            }
            Failure::Error(PluginError::Script(error))
        };
        match result {
            Ok(value) if thread.as_ref().is_ok_and(|thread| thread.is_finished()) => Ok(value),
            Ok(_) => Err(failure(
                ScriptErrorKind::Error,
                "plugin callbacks must not yield".into(),
            )),
            Err(error) => Err(match trip {
                Some(Trip::Cancelled) => Failure::Cancelled,
                Some(Trip::Timeout(_)) => failure(
                    ScriptErrorKind::Timeout,
                    "execution time limit exceeded".into(),
                ),
                None if matches!(innermost(&error), mlua::Error::MemoryError(_)) => {
                    let _ = self.lua.gc_collect();
                    failure(ScriptErrorKind::MemoryLimit, describe(&error))
                }
                None => failure(ScriptErrorKind::Error, describe(&error)),
            }),
        }
    }

    fn deliver(&self, event: &PluginEvent, mut report: impl FnMut(PluginError)) {
        let name = event.name();
        let Ok(Value::Table(list)) = self.handlers.raw_get::<Value>(name) else {
            return;
        };
        for index in 1..=list.raw_len() {
            if self.shared.cancelled() {
                return;
            }
            let Ok(Value::Function(handler)) = list.raw_get::<Value>(index) else {
                continue;
            };
            let payload = match self.lua.to_value(event) {
                Ok(payload) => payload,
                Err(error) => {
                    report(PluginError::Runtime(format!(
                        "building the {name} payload: {error}"
                    )));
                    return;
                }
            };
            match self.call(ScriptPhase::Event(name), &handler, Some(payload)) {
                Ok(_) => {}
                Err(Failure::Cancelled) => return,
                Err(Failure::Error(error)) => report(error),
            }
        }
    }
}
