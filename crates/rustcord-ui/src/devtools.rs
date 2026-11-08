use imgui::Ui;
use rustcord_client::devtools::{self as capture, Direction, HttpSummary, SocketKind, SocketSummary};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

const LOG_CAPACITY: usize = 5000;
const DETAIL_FRACTION: f32 = 0.45;
const ACCENT: [f32; 4] = [0.26, 0.59, 0.98, 1.0];
const MUTED: [f32; 4] = [0.6, 0.6, 0.6, 1.0];

static LOGS: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub seq: u64,
    pub at: SystemTime,
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
}

pub struct LogCapture;

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: Vec<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let mut message = visitor.message;
        if !visitor.fields.is_empty() {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&visitor.fields.join(" "));
        }
        let entry = LogEntry {
            seq: LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1,
            at: SystemTime::now(),
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            message,
        };
        let mut logs = LOGS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if logs.len() >= LOG_CAPACITY {
            logs.pop_front();
        }
        logs.push_back(entry);
    }
}

fn logs_since(seq: u64) -> Vec<LogEntry> {
    let logs = LOGS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let start = logs.partition_point(|entry| entry.seq <= seq);
    logs.range(start..).cloned().collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Logs,
    Network,
    Sockets,
    Runtime,
}

const CLOCK_TICKS_PER_SEC: f64 = 100.0;
const SAMPLE_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSample {
    pub acquire_ms: f32,
    pub build_ms: f32,
    pub submit_ms: f32,
    pub present_ms: f32,
    pub vertices: u32,
    pub indices: u32,
    pub draw_lists: u32,
    pub commands: u32,
}

impl FrameSample {
    pub fn total_ms(&self) -> f32 {
        self.acquire_ms + self.build_ms + self.submit_ms + self.present_ms
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TextureStats {
    pub images: usize,
    pub animated: usize,
    pub videos: usize,
    pub live_videos: usize,
    pub icons: usize,
    pub pending: usize,
    pub failed: usize,
    pub bytes: usize,
    pub image_bytes: usize,
    pub animated_bytes: usize,
    pub video_bytes: usize,
    pub live_video_bytes: usize,
    pub icon_bytes: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CallSnapshot {
    pub label: &'static str,
    pub current: rustcord_client::voice_gateway::VoiceStats,
    pub history: Vec<rustcord_client::voice_gateway::VoiceStats>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeSnapshot {
    pub frames: Vec<(std::time::Instant, FrameSample)>,
    pub devtools_frames: Vec<(std::time::Instant, FrameSample)>,
    pub adapter: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub surface: [u32; 2],
    pub format: String,
    pub present_mode: String,
    pub textures: TextureStats,
    pub cached_messages: usize,
    pub calls: Vec<CallSnapshot>,
}

pub fn parse_task_stat(text: &str) -> Option<(String, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?.to_string();
    let fields: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((name, utime + stime))
}

pub fn parse_status_rss(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrmClient {
    pub id: String,
    pub driver: String,
    pub engines: Vec<(String, u64)>,
    pub memory: Vec<(String, u64)>,
}

fn parse_kib(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let number: u64 = parts.next()?.parse().ok()?;
    let scale = match parts.next() {
        Some("KiB") | Some("kB") => 1024,
        Some("MiB") => 1024 * 1024,
        Some("GiB") => 1024 * 1024 * 1024,
        _ => 1,
    };
    Some(number * scale)
}

pub fn parse_drm_fdinfo(text: &str) -> Option<DrmClient> {
    let mut client = DrmClient::default();
    let mut legacy_memory = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if key == "drm-client-id" {
            client.id = value.to_string();
        } else if key == "drm-driver" {
            client.driver = value.to_string();
        } else if let Some(engine) = key.strip_prefix("drm-engine-") {
            if let Some(ns) = value.strip_suffix("ns").and_then(|v| v.trim().parse().ok()) {
                client.engines.push((engine.to_string(), ns));
            }
        } else if let Some(region) = key.strip_prefix("drm-resident-") {
            if let Some(bytes) = parse_kib(value).filter(|&b| b > 0) {
                client.memory.push((region.to_string(), bytes));
            }
        } else if let Some(region) = key.strip_prefix("drm-memory-") {
            if let Some(bytes) = parse_kib(value).filter(|&b| b > 0) {
                legacy_memory.push((region.to_string(), bytes));
            }
        }
    }
    if client.memory.is_empty() {
        client.memory = legacy_memory;
    }
    (!client.id.is_empty()).then_some(client)
}

fn read_threads() -> Vec<(String, u32, u64)> {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return Vec::new() };
    tasks
        .flatten()
        .filter_map(|task| {
            let tid = task.file_name().to_string_lossy().parse().ok()?;
            let text = std::fs::read_to_string(task.path().join("stat")).ok()?;
            let (name, ticks) = parse_task_stat(&text)?;
            Some((name, tid, ticks))
        })
        .collect()
}

fn read_drm_clients() -> Vec<DrmClient> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fdinfo") else { return Vec::new() };
    let mut clients: Vec<DrmClient> = Vec::new();
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        if let Some(client) = parse_drm_fdinfo(&text) {
            if !clients.iter().any(|c| c.id == client.id && c.driver == client.driver) {
                clients.push(client);
            }
        }
    }
    clients
}

#[derive(Default)]
struct UsageSampler {
    last: Option<std::time::Instant>,
    thread_ticks: HashMap<u32, u64>,
    engine_ns: HashMap<(String, String), u64>,
    threads: Vec<(String, usize, f32)>,
    process_cpu: f32,
    rss: Option<u64>,
    engines: Vec<(String, f32)>,
    gpu_memory: Vec<(String, u64)>,
    gpu_driver: Option<String>,
}

pub fn group_thread_usage(previous: &HashMap<u32, u64>, current: &[(String, u32, u64)], elapsed_secs: f64) -> Vec<(String, usize, f32)> {
    let mut groups: HashMap<String, (usize, f64)> = HashMap::new();
    for (name, tid, ticks) in current {
        let delta = previous.get(tid).map_or(0, |before| ticks.saturating_sub(*before));
        let entry = groups.entry(name.clone()).or_default();
        entry.0 += 1;
        entry.1 += delta as f64;
    }
    let mut rows: Vec<(String, usize, f32)> = groups
        .into_iter()
        .map(|(name, (count, ticks))| (name, count, (ticks / CLOCK_TICKS_PER_SEC / elapsed_secs.max(0.001) * 100.0) as f32))
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
    rows
}

impl UsageSampler {
    fn sample(&mut self) {
        let now = std::time::Instant::now();
        if self.last.is_some_and(|last| now.duration_since(last) < SAMPLE_EVERY) {
            return;
        }
        let elapsed = self.last.map_or(0.0, |last| now.duration_since(last).as_secs_f64());
        let threads = read_threads();
        if elapsed > 0.0 {
            self.threads = group_thread_usage(&self.thread_ticks, &threads, elapsed);
            self.process_cpu = self.threads.iter().map(|(_, _, cpu)| cpu).sum();
        }
        self.thread_ticks = threads.iter().map(|(_, tid, ticks)| (*tid, *ticks)).collect();
        self.rss = std::fs::read_to_string("/proc/self/status").ok().and_then(|t| parse_status_rss(&t));
        let clients = read_drm_clients();
        let mut engines: HashMap<String, u64> = HashMap::new();
        let mut engine_ns = HashMap::new();
        let mut memory: HashMap<String, u64> = HashMap::new();
        for client in &clients {
            for (engine, ns) in &client.engines {
                let key = (client.id.clone(), engine.clone());
                if let Some(before) = self.engine_ns.get(&key) {
                    *engines.entry(engine.clone()).or_default() += ns.saturating_sub(*before);
                }
                engine_ns.insert(key, *ns);
            }
            for (region, bytes) in &client.memory {
                *memory.entry(region.clone()).or_default() += bytes;
            }
        }
        if elapsed > 0.0 {
            let mut rows: Vec<(String, f32)> = engines
                .into_iter()
                .map(|(engine, ns)| (engine, (ns as f64 / (elapsed * 1e9) * 100.0) as f32))
                .collect();
            rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            self.engines = rows;
        }
        self.engine_ns = engine_ns;
        let mut memory: Vec<(String, u64)> = memory.into_iter().collect();
        memory.sort();
        self.gpu_memory = memory;
        self.gpu_driver = clients.first().map(|c| c.driver.clone());
        self.last = Some(now);
    }
}

const LEVELS: [(&str, tracing::Level); 5] = [
    ("Error", tracing::Level::ERROR),
    ("Warn", tracing::Level::WARN),
    ("Info", tracing::Level::INFO),
    ("Debug", tracing::Level::DEBUG),
    ("Trace", tracing::Level::TRACE),
];

pub struct DevtoolsView {
    tab: Tab,
    logs: VecDeque<LogEntry>,
    log_seq: u64,
    log_level: usize,
    log_filter: String,
    follow: bool,
    generation: u64,
    http: Vec<HttpSummary>,
    sockets: Vec<SocketSummary>,
    http_filter: String,
    socket_filter: String,
    socket_kinds: [bool; 3],
    show_incoming: bool,
    show_outgoing: bool,
    show_stats: bool,
    selected_http: Option<u64>,
    selected_socket: Option<u64>,
    usage: UsageSampler,
    detail: Option<(u64, String, String)>,
}

impl Default for DevtoolsView {
    fn default() -> Self {
        Self {
            tab: Tab::Logs,
            logs: VecDeque::new(),
            log_seq: 0,
            log_level: 3,
            log_filter: String::new(),
            follow: true,
            generation: u64::MAX,
            http: Vec::new(),
            sockets: Vec::new(),
            http_filter: String::new(),
            socket_filter: String::new(),
            socket_kinds: [true; 3],
            show_incoming: true,
            show_outgoing: true,
            show_stats: false,
            selected_http: None,
            selected_socket: None,
            usage: UsageSampler::default(),
            detail: None,
        }
    }
}

pub fn clock(at: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(at).format("%H:%M:%S%.3f").to_string()
}

pub fn pretty_json(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| text.to_string())
}

pub fn human_size(bytes: usize) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

pub fn op_name(socket: SocketKind, op: u64) -> Option<&'static str> {
    let name = match (socket, op) {
        (SocketKind::Gateway, 0) => "Dispatch",
        (SocketKind::Gateway, 1) => "Heartbeat",
        (SocketKind::Gateway, 2) => "Identify",
        (SocketKind::Gateway, 3) => "Presence Update",
        (SocketKind::Gateway, 4) => "Voice State Update",
        (SocketKind::Gateway, 6) => "Resume",
        (SocketKind::Gateway, 7) => "Reconnect",
        (SocketKind::Gateway, 8) => "Request Guild Members",
        (SocketKind::Gateway, 9) => "Invalid Session",
        (SocketKind::Gateway, 10) => "Hello",
        (SocketKind::Gateway, 11) => "Heartbeat ACK",
        (SocketKind::Gateway, 14) => "Lazy Request",
        (SocketKind::Gateway, 18) => "Stream Create",
        (SocketKind::Gateway, 19) => "Stream Delete",
        (SocketKind::Gateway, 20) => "Stream Watch",
        (SocketKind::Gateway, 37) => "Guild Subscriptions",
        (_, 0) => "Identify",
        (_, 1) => "Select Protocol",
        (_, 2) => "Ready",
        (_, 3) => "Heartbeat",
        (_, 4) => "Session Description",
        (_, 5) => "Speaking",
        (_, 6) => "Heartbeat ACK",
        (_, 7) => "Resume",
        (_, 8) => "Hello",
        (_, 9) => "Resumed",
        (_, 11) => "Clients Connect",
        (_, 12) => "Video",
        (_, 13) => "Client Disconnect",
        (_, 14) => "Session Update",
        (_, 15) => "Media Sink Wants",
        (_, 16) => "Backend Version",
        (_, 21) => "DAVE Prepare Transition",
        (_, 22) => "DAVE Execute Transition",
        (_, 23) => "DAVE Transition Ready",
        (_, 24) => "DAVE Prepare Epoch",
        (_, 25) => "MLS External Sender",
        (_, 26) => "MLS Key Package",
        (_, 27) => "MLS Proposals",
        (_, 28) => "MLS Commit Welcome",
        (_, 29) => "MLS Announce Commit",
        (_, 30) => "MLS Welcome",
        (_, 31) => "MLS Invalid Commit",
        _ => return None,
    };
    Some(name)
}

pub fn frame_label(frame: &SocketSummary) -> String {
    match (&frame.event, frame.op) {
        (Some(event), _) => event.clone(),
        (None, Some(op)) => op_name(frame.socket, op).map_or_else(|| format!("op {op}"), str::to_string),
        (None, None) => if frame.binary { "binary".into() } else { "text".into() },
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatRow {
    pub socket: SocketKind,
    pub direction: Direction,
    pub label: String,
    pub count: usize,
    pub bytes: usize,
}

pub fn socket_stats(frames: &[SocketSummary]) -> Vec<StatRow> {
    let mut rows: HashMap<(SocketKind, Direction, String), (usize, usize)> = HashMap::new();
    for frame in frames {
        let entry = rows.entry((frame.socket, frame.direction, frame_label(frame))).or_default();
        entry.0 += 1;
        entry.1 += frame.size;
    }
    let mut rows: Vec<StatRow> = rows
        .into_iter()
        .map(|((socket, direction, label), (count, bytes))| StatRow { socket, direction, label, count, bytes })
        .collect();
    rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(b.count.cmp(&a.count)).then(a.label.cmp(&b.label)));
    rows
}

fn kind_index(kind: SocketKind) -> usize {
    match kind {
        SocketKind::Gateway => 0,
        SocketKind::Voice => 1,
        SocketKind::Stream => 2,
    }
}

impl DevtoolsView {
    fn refresh(&mut self) {
        let fresh = logs_since(self.log_seq);
        if let Some(last) = fresh.last() {
            self.log_seq = last.seq;
        }
        self.logs.extend(fresh);
        while self.logs.len() > LOG_CAPACITY {
            self.logs.pop_front();
        }
        let generation = capture::generation();
        if generation != self.generation {
            self.generation = generation;
            self.http = capture::http_summaries();
            self.sockets = capture::socket_summaries();
        }
    }

    fn socket_visible(&self, frame: &SocketSummary, filter: &str) -> bool {
        self.socket_kinds[kind_index(frame.socket)]
            && match frame.direction {
                Direction::Incoming => self.show_incoming,
                Direction::Outgoing => self.show_outgoing,
            }
            && (filter.is_empty() || frame_label(frame).to_lowercase().contains(filter) || frame.op.is_some_and(|op| op.to_string() == filter))
    }
}

pub fn draw(ui: &Ui, view: &mut DevtoolsView, mono: imgui::FontId, runtime: &RuntimeSnapshot) {
    view.refresh();
    let display = ui.io().display_size;
    ui.window("##devtools")
        .position([0.0, 0.0], imgui::Condition::Always)
        .size(display, imgui::Condition::Always)
        .no_decoration()
        .no_nav()
        .bring_to_front_on_focus(false)
        .build(|| {
            for (label, tab) in [("Logs", Tab::Logs), ("Network", Tab::Network), ("Sockets", Tab::Sockets), ("Runtime", Tab::Runtime)] {
                let token = (view.tab == tab).then(|| ui.push_style_color(imgui::StyleColor::Button, ACCENT));
                if ui.button(label) {
                    view.tab = tab;
                    view.detail = None;
                }
                drop(token);
                ui.same_line();
            }
            ui.text_disabled("|");
            ui.same_line();
            let mut recording = capture::capturing();
            if ui.checkbox("Record network and sockets", &mut recording) {
                capture::set_capturing(recording);
            }
            ui.same_line();
            if ui.button("Clear") {
                match view.tab {
                    Tab::Logs => view.logs.clear(),
                    Tab::Runtime => {}
                    Tab::Network | Tab::Sockets => {
                        capture::clear();
                        view.selected_http = None;
                        view.selected_socket = None;
                        view.detail = None;
                    }
                }
            }
            ui.separator();
            match view.tab {
                Tab::Logs => draw_logs(ui, view, mono),
                Tab::Network => draw_network(ui, view, mono),
                Tab::Sockets => draw_sockets(ui, view, mono),
                Tab::Runtime => draw_runtime(ui, view, runtime),
            }
        });
}

fn level_color(level: tracing::Level) -> [f32; 4] {
    match level {
        tracing::Level::ERROR => [0.94, 0.33, 0.36, 1.0],
        tracing::Level::WARN => [0.96, 0.74, 0.30, 1.0],
        tracing::Level::INFO => ACCENT,
        _ => MUTED,
    }
}

fn draw_logs(ui: &Ui, view: &mut DevtoolsView, mono: imgui::FontId) {
    ui.set_next_item_width(110.0);
    if let Some(_combo) = ui.begin_combo("##log_level", LEVELS[view.log_level].0) {
        for (index, (label, _)) in LEVELS.iter().enumerate() {
            if ui.selectable_config(*label).selected(index == view.log_level).build() {
                view.log_level = index;
            }
        }
    }
    ui.same_line();
    ui.set_next_item_width(260.0);
    ui.input_text("##log_filter", &mut view.log_filter).hint("Filter by text or target").build();
    ui.same_line();
    ui.checkbox("Follow", &mut view.follow);
    let filter = view.log_filter.to_lowercase();
    let max_level = LEVELS[view.log_level].1;
    let visible: Vec<&LogEntry> = view
        .logs
        .iter()
        .filter(|e| e.level <= max_level)
        .filter(|e| filter.is_empty() || e.message.to_lowercase().contains(&filter) || e.target.to_lowercase().contains(&filter))
        .collect();
    ui.same_line();
    if ui.button("Copy visible") {
        let text: Vec<String> = visible.iter().map(|e| format!("{} {:5} {} {}", clock(e.at), e.level, e.target, e.message)).collect();
        ui.set_clipboard_text(text.join("\n"));
    }
    ui.same_line();
    ui.text_disabled(format!("{} of {} lines", visible.len(), view.logs.len()));
    let font = ui.push_font(mono);
    ui.child_window("log_lines").horizontal_scrollbar(true).build(|| {
        let mut clipper = imgui::ListClipper::new(visible.len() as i32).begin(ui);
        while clipper.step() {
            for index in clipper.display_start()..clipper.display_end() {
                let entry = visible[index as usize];
                ui.text_colored(MUTED, clock(entry.at));
                ui.same_line();
                ui.text_colored(level_color(entry.level), format!("{:5}", entry.level.as_str()));
                ui.same_line();
                ui.text_colored(MUTED, &entry.target);
                ui.same_line();
                ui.text(&entry.message);
            }
        }
        if view.follow && ui.scroll_y() < ui.scroll_max_y() {
            ui.set_scroll_here_y_with_ratio(1.0);
        }
    });
    font.pop();
}

fn status_color(summary: &HttpSummary) -> [f32; 4] {
    match summary.status {
        _ if summary.failed => [0.94, 0.33, 0.36, 1.0],
        None => MUTED,
        Some(status) if status >= 300 => [0.96, 0.74, 0.30, 1.0],
        Some(_) => [0.36, 0.80, 0.48, 1.0],
    }
}

fn detail_pane(ui: &Ui, mono: imgui::FontId, sections: &mut [(&str, String)]) {
    let font = ui.push_font(mono);
    let height = ui.content_region_avail()[1];
    let each = (height / sections.len().max(1) as f32 - ui.text_line_height_with_spacing() * 2.0).max(60.0);
    for (title, body) in sections.iter_mut() {
        ui.text_disabled(*title);
        ui.same_line();
        if ui.small_button(format!("Copy##{title}")) {
            ui.set_clipboard_text(body.as_str());
        }
        ui.input_text_multiline(format!("##{title}"), body, [-1.0, each]).read_only(true).build();
    }
    font.pop();
}

fn draw_network(ui: &Ui, view: &mut DevtoolsView, mono: imgui::FontId) {
    ui.set_next_item_width(260.0);
    ui.input_text("##http_filter", &mut view.http_filter).hint("Filter by path or method").build();
    ui.same_line();
    if !capture::capturing() {
        ui.text_colored([0.96, 0.74, 0.30, 1.0], "Recording is off");
    } else {
        ui.text_disabled(format!("{} requests", view.http.len()));
    }
    let filter = view.http_filter.to_lowercase();
    let visible: Vec<&HttpSummary> = view
        .http
        .iter()
        .rev()
        .filter(|r| filter.is_empty() || r.path.to_lowercase().contains(&filter) || r.method.to_lowercase() == filter)
        .collect();
    let list_height = if view.selected_http.is_some() { ui.content_region_avail()[1] * (1.0 - DETAIL_FRACTION) } else { 0.0 };
    let flags = imgui::TableFlags::ROW_BG | imgui::TableFlags::SCROLL_Y | imgui::TableFlags::RESIZABLE | imgui::TableFlags::BORDERS_INNER_V;
    if let Some(_table) = ui.begin_table_with_sizing("http_table", 6, flags, [0.0, list_height], 0.0) {
        ui.table_setup_scroll_freeze(0, 1);
        for (name, width) in [("Time", 95.0), ("Method", 60.0), ("Path", 0.0), ("Status", 55.0), ("Duration", 70.0), ("Size", 70.0)] {
            let mut column = imgui::TableColumnSetup::new(name);
            if width > 0.0 {
                column.flags = imgui::TableColumnFlags::WIDTH_FIXED;
                column.init_width_or_weight = width;
            } else {
                column.flags = imgui::TableColumnFlags::WIDTH_STRETCH;
            }
            ui.table_setup_column_with(column);
        }
        ui.table_headers_row();
        let mut clipper = imgui::ListClipper::new(visible.len() as i32).begin(ui);
        while clipper.step() {
            for index in clipper.display_start()..clipper.display_end() {
                let record = visible[index as usize];
                ui.table_next_row();
                ui.table_next_column();
                let selected = view.selected_http == Some(record.id);
                if ui
                    .selectable_config(format!("{}##http_{}", clock(record.at), record.id))
                    .selected(selected)
                    .span_all_columns(true)
                    .build()
                {
                    view.selected_http = if selected { None } else { Some(record.id) };
                    view.detail = None;
                }
                ui.table_next_column();
                ui.text(&record.method);
                ui.table_next_column();
                ui.text(&record.path);
                ui.table_next_column();
                ui.text_colored(status_color(record), record.status.map_or_else(|| "…".into(), |s| s.to_string()));
                ui.table_next_column();
                ui.text(record.duration.map_or_else(|| "pending".into(), |d| format!("{} ms", d.as_millis())));
                ui.table_next_column();
                ui.text(record.response_size.map_or_else(String::new, human_size));
            }
        }
    }
    let Some(id) = view.selected_http else { return };
    let Some(record) = capture::http_detail(id) else {
        view.selected_http = None;
        return;
    };
    ui.separator();
    ui.text(format!("{} {}", record.method, record.path));
    if let Some(error) = &record.error {
        ui.same_line();
        ui.text_colored([0.94, 0.33, 0.36, 1.0], error);
    }
    let mut sections = [
        ("Request body", record.request_body.as_deref().map(pretty_json).unwrap_or_else(|| "(none)".into())),
        ("Response body", record.response_body.as_deref().map(pretty_json).unwrap_or_else(|| "(none yet)".into())),
    ];
    detail_pane(ui, mono, &mut sections);
}

fn draw_sockets(ui: &Ui, view: &mut DevtoolsView, mono: imgui::FontId) {
    for (index, kind) in [SocketKind::Gateway, SocketKind::Voice, SocketKind::Stream].into_iter().enumerate() {
        ui.checkbox(kind.label(), &mut view.socket_kinds[index]);
        ui.same_line();
    }
    ui.checkbox("In", &mut view.show_incoming);
    ui.same_line();
    ui.checkbox("Out", &mut view.show_outgoing);
    ui.same_line();
    ui.set_next_item_width(200.0);
    ui.input_text("##socket_filter", &mut view.socket_filter).hint("Event name or op").build();
    ui.same_line();
    ui.checkbox("Summary", &mut view.show_stats);
    if !capture::capturing() {
        ui.same_line();
        ui.text_colored([0.96, 0.74, 0.30, 1.0], "Recording is off");
    }
    let filter = view.socket_filter.trim().to_lowercase();
    let visible: Vec<&SocketSummary> = view.sockets.iter().rev().filter(|f| view.socket_visible(f, &filter)).collect();
    if view.show_stats {
        let owned: Vec<SocketSummary> = visible.iter().map(|f| (*f).clone()).collect();
        let stats = socket_stats(&owned);
        let total_bytes: usize = owned.iter().map(|f| f.size).sum();
        ui.text_disabled(format!("{} frames, {} in view", owned.len(), human_size(total_bytes)));
        let flags = imgui::TableFlags::ROW_BG | imgui::TableFlags::SCROLL_Y | imgui::TableFlags::BORDERS_INNER_V;
        if let Some(_table) = ui.begin_table_with_sizing("socket_stats", 5, flags, [0.0, 0.0], 0.0) {
            ui.table_setup_scroll_freeze(0, 1);
            for name in ["Socket", "Dir", "Event / op", "Frames", "Bytes"] {
                ui.table_setup_column(name);
            }
            ui.table_headers_row();
            for row in &stats {
                ui.table_next_row();
                ui.table_next_column();
                ui.text(row.socket.label());
                ui.table_next_column();
                ui.text(if row.direction == Direction::Incoming { "in" } else { "out" });
                ui.table_next_column();
                ui.text(&row.label);
                ui.table_next_column();
                ui.text(row.count.to_string());
                ui.table_next_column();
                ui.text(human_size(row.bytes));
            }
        }
        return;
    }
    let list_height = if view.selected_socket.is_some() { ui.content_region_avail()[1] * (1.0 - DETAIL_FRACTION) } else { 0.0 };
    let flags = imgui::TableFlags::ROW_BG | imgui::TableFlags::SCROLL_Y | imgui::TableFlags::RESIZABLE | imgui::TableFlags::BORDERS_INNER_V;
    if let Some(_table) = ui.begin_table_with_sizing("socket_table", 6, flags, [0.0, list_height], 0.0) {
        ui.table_setup_scroll_freeze(0, 1);
        for (name, width) in [("Time", 95.0), ("Socket", 65.0), ("Dir", 35.0), ("Op", 35.0), ("Event", 0.0), ("Size", 70.0)] {
            let mut column = imgui::TableColumnSetup::new(name);
            if width > 0.0 {
                column.flags = imgui::TableColumnFlags::WIDTH_FIXED;
                column.init_width_or_weight = width;
            } else {
                column.flags = imgui::TableColumnFlags::WIDTH_STRETCH;
            }
            ui.table_setup_column_with(column);
        }
        ui.table_headers_row();
        let mut clipper = imgui::ListClipper::new(visible.len() as i32).begin(ui);
        while clipper.step() {
            for index in clipper.display_start()..clipper.display_end() {
                let frame = visible[index as usize];
                ui.table_next_row();
                ui.table_next_column();
                let selected = view.selected_socket == Some(frame.id);
                if ui
                    .selectable_config(format!("{}##socket_{}", clock(frame.at), frame.id))
                    .selected(selected)
                    .span_all_columns(true)
                    .build()
                {
                    view.selected_socket = if selected { None } else { Some(frame.id) };
                    view.detail = None;
                }
                ui.table_next_column();
                ui.text(frame.socket.label());
                ui.table_next_column();
                let (arrow, color) = match frame.direction {
                    Direction::Incoming => ("in", [0.36, 0.80, 0.48, 1.0]),
                    Direction::Outgoing => ("out", ACCENT),
                };
                ui.text_colored(color, arrow);
                ui.table_next_column();
                ui.text(frame.op.map_or_else(String::new, |op| op.to_string()));
                ui.table_next_column();
                ui.text(frame_label(frame));
                ui.table_next_column();
                ui.text(human_size(frame.size));
            }
        }
    }
    let Some(id) = view.selected_socket else { return };
    if view.detail.as_ref().is_none_or(|(detail_id, ..)| *detail_id != id) {
        let Some(record) = capture::socket_detail(id) else {
            view.selected_socket = None;
            return;
        };
        let title = format!("{} {} · {}", record.socket.label(), if record.direction == Direction::Incoming { "in" } else { "out" }, human_size(record.size));
        let body = if record.binary { record.payload } else { pretty_json(&record.payload) };
        view.detail = Some((id, title, body));
    }
    let Some((_, title, body)) = view.detail.as_mut() else { return };
    ui.separator();
    ui.text(title.as_str());
    let mut sections = [("Payload", std::mem::take(body))];
    detail_pane(ui, mono, &mut sections);
    *body = std::mem::take(&mut sections[0].1);
}

fn kv(ui: &Ui, label: &str, value: impl AsRef<str>) {
    ui.table_next_row();
    ui.table_next_column();
    ui.text_colored(MUTED, label);
    ui.table_next_column();
    ui.text(value.as_ref());
}

fn kv_table(ui: &Ui, id: &str, rows: impl FnOnce()) {
    if let Some(_table) = ui.begin_table_with_flags(id, 2, imgui::TableFlags::SIZING_FIXED_FIT) {
        rows();
    }
}

fn opt<T: std::fmt::Display>(value: Option<T>, unit: &str) -> String {
    value.map_or_else(|| "–".into(), |v| format!("{v}{unit}"))
}

fn kbps(bps: u32) -> String {
    format!("{:.0} kbps", bps as f32 / 1000.0)
}

fn plot(ui: &Ui, id: &str, label: &str, values: &[f32]) {
    if values.is_empty() {
        return;
    }
    let max = values.iter().copied().fold(0.0f32, f32::max);
    ui.plot_lines(format!("##{id}"), values)
        .overlay_text(format!("{label} (max {max:.1})"))
        .scale_min(0.0)
        .scale_max((max * 1.2).max(1.0))
        .graph_size([ui.content_region_avail()[0], 70.0])
        .build();
}

fn draw_runtime(ui: &Ui, view: &mut DevtoolsView, runtime: &RuntimeSnapshot) {
    view.usage.sample();
    let usage = &view.usage;
    ui.child_window("runtime").build(|| {
        if ui.collapsing_header("Usage by thread", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            ui.text(format!(
                "Process CPU {:.1}% (100% = one core) · RSS {}",
                usage.process_cpu,
                usage.rss.map_or_else(|| "–".into(), |r| human_size(r as usize))
            ));
            if usage.threads.is_empty() {
                ui.text_disabled("Sampling… (per-thread CPU needs /proc, Linux only)");
            } else if let Some(_table) = ui.begin_table_with_flags("threads", 3, imgui::TableFlags::ROW_BG | imgui::TableFlags::SIZING_FIXED_FIT) {
                ui.table_setup_column("Thread");
                ui.table_setup_column("Count");
                ui.table_setup_column("CPU");
                ui.table_headers_row();
                for (name, count, cpu) in &usage.threads {
                    ui.table_next_row();
                    ui.table_next_column();
                    ui.text(name);
                    ui.table_next_column();
                    ui.text(count.to_string());
                    ui.table_next_column();
                    let color = if *cpu >= 50.0 { [0.94, 0.33, 0.36, 1.0] } else if *cpu >= 10.0 { [0.96, 0.74, 0.30, 1.0] } else { MUTED };
                    ui.text_colored(color, format!("{cpu:.1}%"));
                }
            }
        }
        if ui.collapsing_header("GPU", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            kv_table(ui, "gpu_info", || {
                kv(ui, "Adapter", &runtime.adapter);
                kv(ui, "Backend", format!("{} · {}", runtime.backend, runtime.device_type));
                kv(ui, "Driver", &runtime.driver);
                kv(ui, "Surface", format!("{}×{} {} · {}", runtime.surface[0], runtime.surface[1], runtime.format, runtime.present_mode));
            });
            if usage.engines.is_empty() && usage.gpu_memory.is_empty() {
                ui.text_disabled("Per-process GPU usage isn't exposed by this driver (needs DRM fdinfo: amdgpu, i915, xe, nouveau).");
            } else {
                ui.text_colored(MUTED, format!("This process on {} (DRM fdinfo)", usage.gpu_driver.as_deref().unwrap_or("GPU")));
                kv_table(ui, "gpu_usage", || {
                    for (engine, busy) in &usage.engines {
                        kv(ui, &format!("{engine} engine"), format!("{busy:.1}% busy"));
                    }
                    for (region, bytes) in &usage.gpu_memory {
                        kv(ui, &format!("{region} memory"), human_size(*bytes as usize));
                    }
                });
            }
        }
        if ui.collapsing_header("Memory by item (estimates)", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            let t = &runtime.textures;
            let capture_bytes: usize = view.logs.iter().map(|e| e.message.len() + e.target.len() + 48).sum();
            kv_table(ui, "memory_items", || {
                kv(ui, "Images", format!("{} · {}", t.images, human_size(t.image_bytes)));
                kv(ui, "Animated (GIF/APNG frames)", format!("{} · {}", t.animated, human_size(t.animated_bytes)));
                kv(ui, "Inline video players", format!("{} · {}", t.videos, human_size(t.video_bytes)));
                kv(ui, "Live call/stream video", format!("{} · {}", t.live_videos, human_size(t.live_video_bytes)));
                kv(ui, "Icons", format!("{} · {}", t.icons, human_size(t.icon_bytes)));
                kv(ui, "Texture cache total", human_size(t.bytes));
                kv(ui, "Pending / failed loads", format!("{} / {}", t.pending, t.failed));
                kv(ui, "Cached messages", runtime.cached_messages.to_string());
                kv(ui, "Devtools log buffer", format!("{} lines · {}", view.logs.len(), human_size(capture_bytes)));
                kv(ui, "Devtools captures", format!("{} requests · {} frames", view.http.len(), view.sockets.len()));
                if let Some(heap) = crate::memory::stats() {
                    kv(ui, "Heap in use", human_size(heap.in_use + heap.mmapped));
                    kv(ui, "Heap free but held", human_size(heap.free_held));
                }
            });
            if crate::memory::stats().is_some() && ui.button("Trim heap now") {
                crate::memory::trim();
            }
        }
        if ui.collapsing_header("Rendering (main window)", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            let now = std::time::Instant::now();
            let recent: Vec<&FrameSample> = runtime
                .frames
                .iter()
                .filter(|(at, _)| now.duration_since(*at) < std::time::Duration::from_secs(1))
                .map(|(_, f)| f)
                .collect();
            let last: Vec<&FrameSample> = runtime.frames.iter().rev().take(60).map(|(_, f)| f).collect();
            let average = |pick: fn(&FrameSample) -> f32| if last.is_empty() { 0.0 } else { last.iter().map(|f| pick(f)).sum::<f32>() / last.len() as f32 };
            let worst = last.iter().map(|f| f.total_ms()).fold(0.0f32, f32::max);
            kv_table(ui, "render_stats", || {
                kv(ui, "Redraws (last second)", format!("{} (redraws only happen on input, animation or new data)", recent.len()));
                kv(ui, "Acquire surface", format!("{:.2} ms avg", average(|f| f.acquire_ms)));
                kv(ui, "UI build (imgui)", format!("{:.2} ms avg", average(|f| f.build_ms)));
                kv(ui, "Encode + submit (wgpu)", format!("{:.2} ms avg", average(|f| f.submit_ms)));
                kv(ui, "Present", format!("{:.2} ms avg", average(|f| f.present_ms)));
                kv(ui, "Frame total", format!("{:.2} ms avg · {worst:.2} ms worst of last {}", average(FrameSample::total_ms), last.len()));
                if let Some((_, f)) = runtime.frames.last() {
                    kv(ui, "Draw data", format!("{} vertices · {} indices · {} lists · {} draw calls", f.vertices, f.indices, f.draw_lists, f.commands));
                }
            });
            let dev_recent = runtime.devtools_frames.iter().filter(|(at, _)| now.duration_since(*at) < std::time::Duration::from_secs(1)).count();
            let dev_last: Vec<f32> = runtime.devtools_frames.iter().rev().take(60).map(|(_, f)| f.total_ms()).collect();
            let dev_avg = if dev_last.is_empty() { 0.0 } else { dev_last.iter().sum::<f32>() / dev_last.len() as f32 };
            kv_table(ui, "devtools_render", || {
                kv(ui, "Devtools window", format!("{dev_recent} redraws/s · {dev_avg:.2} ms avg (same thread as the main window)"));
                kv(ui, "UI thread busy rendering", format!("~{:.0}% of the last second", (recent.iter().map(|f| f.total_ms()).sum::<f32>() + runtime.devtools_frames.iter().filter(|(at, _)| now.duration_since(*at) < std::time::Duration::from_secs(1)).map(|(_, f)| f.total_ms()).sum::<f32>()) / 10.0));
            });
            let totals: Vec<f32> = runtime.frames.iter().map(|(_, f)| f.total_ms()).collect();
            plot(ui, "frame_total", "frame ms", &totals);
            let builds: Vec<f32> = runtime.frames.iter().map(|(_, f)| f.build_ms).collect();
            plot(ui, "frame_build", "UI build ms", &builds);
        }
        if runtime.calls.is_empty() {
            ui.text_disabled("Not in a call.");
        }
        for call in &runtime.calls {
            if !ui.collapsing_header(call.label, imgui::TreeNodeFlags::DEFAULT_OPEN) {
                continue;
            }
            let s = &call.current;
            kv_table(ui, call.label, || {
                kv(ui, "Endpoint", &s.endpoint);
                kv(ui, "Audio SSRC", s.audio_ssrc.to_string());
                kv(ui, "Bitrate", format!("up {} · down {}", kbps(s.bitrate_up_bps), kbps(s.bitrate_down_bps)));
                kv(ui, "RTT", format!("ws {} · rtc {}", opt(s.ws_rtt_ms, " ms"), opt(s.rtc_rtt_ms.map(|v| format!("{v:.0}")), " ms")));
                kv(ui, "Loss", format!("up {} · down {}", opt(s.loss_up_pct.map(|v| format!("{v:.1}")), "%"), opt(s.loss_down_pct.map(|v| format!("{v:.1}")), "%")));
                kv(ui, "Jitter", opt(s.jitter_ms.map(|v| format!("{v:.1}")), " ms"));
                kv(ui, "DAVE (E2EE)", if s.dave_active { format!("v{} · epoch {}", s.dave_version, opt(s.dave_epoch, "")) } else { "off".into() });
                if s.video_fps > 0.0 || s.video_width > 0 {
                    kv(ui, "Video in", format!("{}×{} · {:.1} fps · {} · {}", s.video_width, s.video_height, s.video_fps, kbps(s.video_bitrate_down_bps), s.video_codec));
                    kv(ui, "Video in errors", format!("{} decrypt failures · {} decoder drops", s.video_decrypt_failures, s.video_decoder_drops));
                }
                if s.video_fps_up > 0.0 {
                    kv(ui, "Video out", format!("{}×{} · {:.1} fps · {}", s.video_up_width, s.video_up_height, s.video_fps_up, kbps(s.video_bitrate_up_bps)));
                }
                if s.audio_stream_active {
                    kv(ui, "Stream audio out", kbps(s.audio_stream_bitrate_up_bps));
                }
            });
            let rtt: Vec<f32> = call.history.iter().map(|h| h.rtc_rtt_ms.unwrap_or(0.0)).collect();
            plot(ui, &format!("{}_rtt", call.label), "RTT ms", &rtt);
            let down: Vec<f32> = call.history.iter().map(|h| h.bitrate_down_bps as f32 / 1000.0).collect();
            plot(ui, &format!("{}_down", call.label), "down kbps", &down);
            let up: Vec<f32> = call.history.iter().map(|h| h.bitrate_up_bps as f32 / 1000.0).collect();
            plot(ui, &format!("{}_up", call.label), "up kbps", &up);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(socket: SocketKind, direction: Direction, op: Option<u64>, event: Option<&str>, size: usize) -> SocketSummary {
        SocketSummary { id: 1, at: SystemTime::UNIX_EPOCH, socket, direction, op, event: event.map(str::to_string), size, binary: false }
    }

    #[test]
    fn task_stat_reads_the_name_and_cpu_ticks_even_with_spaces_in_the_name() {
        let line = "1234 (tokio-runtime w) S 1 2 3 4 5 6 7 8 9 10 250 50 0 0 20 0 1 0 100 0 0";
        assert_eq!(parse_task_stat(line), Some(("tokio-runtime w".into(), 300)));
        assert_eq!(parse_task_stat("garbage"), None);
    }

    #[test]
    fn rss_comes_from_the_status_file_in_bytes() {
        assert_eq!(parse_status_rss("Name:\trustcord\nVmRSS:\t  2048 kB\nThreads:\t9\n"), Some(2048 * 1024));
    }

    #[test]
    fn drm_fdinfo_yields_engine_time_and_memory() {
        let text = "pos:\t0\ndrm-driver:\tamdgpu\ndrm-client-id:\t42\ndrm-engine-gfx:\t1500000 ns\ndrm-engine-dec:\t0 ns\ndrm-memory-vram:\t2048 KiB\ndrm-memory-gtt:\t512 KiB\n";
        let client = parse_drm_fdinfo(text).expect("a drm client");
        assert_eq!((client.id.as_str(), client.driver.as_str()), ("42", "amdgpu"));
        assert_eq!(client.engines, [("gfx".to_string(), 1_500_000), ("dec".to_string(), 0)]);
        assert_eq!(client.memory, [("vram".to_string(), 2048 * 1024), ("gtt".to_string(), 512 * 1024)]);
        assert!(parse_drm_fdinfo("pos:\t0\nflags:\t02\n").is_none());
    }

    #[test]
    fn modern_amdgpu_fdinfo_is_not_double_counted_and_skips_empty_regions() {
        let text = "drm-driver:\tamdgpu\ndrm-client-id:\t274\ndrm-total-vram:\t153844 KiB\ndrm-resident-gtt:\t19636 KiB\ndrm-resident-vram:\t153844 KiB\ndrm-resident-gds:\t0\ndrm-memory-vram:\t153844 KiB\ndrm-memory-gtt: \t19636 KiB\ndrm-memory-cpu: \t0 KiB\ndrm-engine-gfx:\t26321366943 ns\ndrm-engine-compute:\t127380964 ns\n";
        let client = parse_drm_fdinfo(text).expect("a drm client");
        assert_eq!(client.memory, [("gtt".to_string(), 19636 * 1024), ("vram".to_string(), 153844 * 1024)]);
        assert_eq!(client.engines.len(), 2);
    }

    #[test]
    fn thread_usage_groups_by_name_and_ignores_threads_seen_for_the_first_time() {
        let previous = HashMap::from([(1, 100), (2, 200)]);
        let current = [("worker".to_string(), 1, 150), ("worker".to_string(), 2, 210), ("main".to_string(), 3, 999)];
        let rows = group_thread_usage(&previous, &current, 1.0);
        assert_eq!(rows[0], ("worker".to_string(), 2, 60.0));
        assert_eq!(rows[1], ("main".to_string(), 1, 0.0));
    }

    #[test]
    fn frames_are_labelled_by_event_then_op_name() {
        assert_eq!(frame_label(&frame(SocketKind::Gateway, Direction::Incoming, Some(0), Some("READY"), 1)), "READY");
        assert_eq!(frame_label(&frame(SocketKind::Gateway, Direction::Outgoing, Some(2), None, 1)), "Identify");
        assert_eq!(frame_label(&frame(SocketKind::Voice, Direction::Incoming, Some(4), None, 1)), "Session Description");
        assert_eq!(frame_label(&frame(SocketKind::Voice, Direction::Incoming, Some(99), None, 1)), "op 99");
    }

    #[test]
    fn stats_group_by_socket_direction_and_label_largest_first() {
        let frames = [
            frame(SocketKind::Gateway, Direction::Incoming, Some(0), Some("PRESENCE_UPDATE"), 100),
            frame(SocketKind::Gateway, Direction::Incoming, Some(0), Some("PRESENCE_UPDATE"), 150),
            frame(SocketKind::Gateway, Direction::Incoming, Some(0), Some("READY"), 900),
            frame(SocketKind::Gateway, Direction::Outgoing, Some(1), None, 20),
        ];
        let stats = socket_stats(&frames);
        assert_eq!(stats[0].label, "READY");
        assert_eq!((stats[1].label.as_str(), stats[1].count, stats[1].bytes), ("PRESENCE_UPDATE", 2, 250));
        assert_eq!((stats[2].label.as_str(), stats[2].direction), ("Heartbeat", Direction::Outgoing));
    }

    #[test]
    fn json_is_pretty_printed_and_other_text_is_left_alone() {
        assert_eq!(pretty_json(r#"{"a":1}"#), "{\n  \"a\": 1\n}");
        assert_eq!(pretty_json("de ad be ef"), "de ad be ef");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1_048_576), "3.0 MB");
    }

    #[test]
    fn the_log_layer_records_messages_and_fields() {
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber = tracing_subscriber::registry().with(LogCapture);
        let before = LOG_SEQ.load(Ordering::Relaxed);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(user = 7, "devtools test line");
        });
        let fresh = logs_since(before);
        let entry = fresh.iter().find(|e| e.message.starts_with("devtools test line")).expect("captured");
        assert_eq!(entry.level, tracing::Level::WARN);
        assert!(entry.message.ends_with("user=7"), "{}", entry.message);
    }
}
