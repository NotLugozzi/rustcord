use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

pub const BODY_LIMIT: usize = 64 * 1024;
const HTTP_CAPACITY: usize = 500;
const SOCKET_CAPACITY: usize = 3000;
const REDACTED: &str = "[redacted]";
const SECRET_KEYS: &[&str] = &["token", "password", "new_password", "ticket", "code", "captcha_key", "captcha_rqtoken", "authorization"];

static CAPTURING: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static STORE: Mutex<Store> = Mutex::new(Store { http: VecDeque::new(), sockets: VecDeque::new(), next_id: 1 });

struct Store {
    http: VecDeque<HttpRecord>,
    sockets: VecDeque<SocketRecord>,
    next_id: u64,
}

#[derive(Debug, Clone)]
pub struct HttpRecord {
    pub id: u64,
    pub at: SystemTime,
    pub method: String,
    pub path: String,
    pub request_body: Option<String>,
    pub status: Option<u16>,
    pub duration: Option<Duration>,
    pub response_size: Option<usize>,
    pub response_body: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpSummary {
    pub id: u64,
    pub at: SystemTime,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub duration: Option<Duration>,
    pub response_size: Option<usize>,
    pub failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SocketKind {
    Gateway,
    Voice,
    Stream,
}

impl SocketKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gateway => "Gateway",
            Self::Voice => "Voice",
            Self::Stream => "Stream",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone)]
pub struct SocketRecord {
    pub id: u64,
    pub at: SystemTime,
    pub socket: SocketKind,
    pub direction: Direction,
    pub op: Option<u64>,
    pub event: Option<String>,
    pub size: usize,
    pub binary: bool,
    pub payload: String,
}

#[derive(Debug, Clone)]
pub struct SocketSummary {
    pub id: u64,
    pub at: SystemTime,
    pub socket: SocketKind,
    pub direction: Direction,
    pub op: Option<u64>,
    pub event: Option<String>,
    pub size: usize,
    pub binary: bool,
}

pub fn set_capturing(enabled: bool) {
    CAPTURING.store(enabled, Ordering::Relaxed);
}

pub fn capturing() -> bool {
    CAPTURING.load(Ordering::Relaxed)
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut store = STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = f(&mut store);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    result
}

pub fn clear() {
    with_store(|store| {
        store.http.clear();
        store.sockets.clear();
    });
}

pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if SECRET_KEYS.iter().any(|secret| key.eq_ignore_ascii_case(secret)) && !child.is_null() {
                    *child = Value::String(REDACTED.into());
                } else {
                    redact_value(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}

pub fn redact_text(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(mut value) => {
            redact_value(&mut value);
            value.to_string()
        }
        Err(_) => text.to_string(),
    }
}

pub fn truncate(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} more bytes]", &text[..end], text.len() - end)
}

fn body_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => truncate(redact_text(text), BODY_LIMIT),
        Err(_) => format!("[{} bytes of binary data]", bytes.len()),
    }
}

pub fn begin_http(method: &str, path: &str, request_body: Option<&[u8]>) -> u64 {
    let request_body = request_body.map(body_text);
    with_store(|store| {
        let id = store.next_id;
        store.next_id += 1;
        if store.http.len() >= HTTP_CAPACITY {
            store.http.pop_front();
        }
        store.http.push_back(HttpRecord {
            id,
            at: SystemTime::now(),
            method: method.to_string(),
            path: path.to_string(),
            request_body,
            status: None,
            duration: None,
            response_size: None,
            response_body: None,
            error: None,
        });
        id
    })
}

pub fn finish_http(id: u64, status: Option<u16>, duration: Duration, response: Result<&[u8], String>) {
    let (size, body, error) = match response {
        Ok(bytes) => (Some(bytes.len()), Some(body_text(bytes)), None),
        Err(error) => (None, None, Some(error)),
    };
    with_store(|store| {
        if let Some(record) = store.http.iter_mut().rev().find(|r| r.id == id) {
            record.status = status;
            record.duration = Some(duration);
            record.response_size = size;
            record.response_body = body;
            record.error = error;
        }
    });
}

fn push_socket(record: SocketRecord) {
    with_store(|store| {
        if store.sockets.len() >= SOCKET_CAPACITY {
            store.sockets.pop_front();
        }
        store.sockets.push_back(SocketRecord { id: store.next_id, ..record });
        store.next_id += 1;
    });
}

pub fn describe_frame(text: &str) -> (Option<u64>, Option<String>, String) {
    match serde_json::from_str::<Value>(text) {
        Ok(mut value) => {
            let op = value.get("op").and_then(Value::as_u64);
            let event = value.get("t").and_then(Value::as_str).map(str::to_string);
            redact_value(&mut value);
            (op, event, truncate(value.to_string(), BODY_LIMIT))
        }
        Err(_) => (None, None, truncate(text.to_string(), BODY_LIMIT)),
    }
}

pub fn record_socket_text(socket: SocketKind, direction: Direction, text: &str) {
    if !capturing() {
        return;
    }
    let (op, event, payload) = describe_frame(text);
    push_socket(SocketRecord { id: 0, at: SystemTime::now(), socket, direction, op, event, size: text.len(), binary: false, payload });
}

pub fn record_socket_binary(socket: SocketKind, direction: Direction, bytes: &[u8], op: Option<u64>) {
    if !capturing() {
        return;
    }
    let preview: Vec<String> = bytes.iter().take(64).map(|b| format!("{b:02x}")).collect();
    let more = if bytes.len() > 64 { format!(" … ({} bytes)", bytes.len()) } else { String::new() };
    push_socket(SocketRecord {
        id: 0,
        at: SystemTime::now(),
        socket,
        direction,
        op,
        event: None,
        size: bytes.len(),
        binary: true,
        payload: format!("{}{more}", preview.join(" ")),
    });
}

pub fn record_ws_message(socket: SocketKind, direction: Direction, message: &tokio_tungstenite::tungstenite::Message) {
    use tokio_tungstenite::tungstenite::Message;
    if !capturing() {
        return;
    }
    match message {
        Message::Text(text) => record_socket_text(socket, direction, text),
        Message::Binary(bytes) => {
            let op = match direction {
                Direction::Incoming => bytes.get(2),
                Direction::Outgoing => bytes.first(),
            };
            record_socket_binary(socket, direction, bytes, op.map(|&b| u64::from(b)));
        }
        Message::Close(frame) => {
            let reason = frame.as_ref().map(|f| format!("close {}: {}", f.code, f.reason)).unwrap_or_else(|| "close".into());
            push_socket(SocketRecord {
                id: 0,
                at: SystemTime::now(),
                socket,
                direction,
                op: None,
                event: Some("CLOSE".into()),
                size: 0,
                binary: false,
                payload: reason,
            });
        }
        _ => {}
    }
}

pub fn http_summaries() -> Vec<HttpSummary> {
    let store = STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    store
        .http
        .iter()
        .map(|r| HttpSummary {
            id: r.id,
            at: r.at,
            method: r.method.clone(),
            path: r.path.clone(),
            status: r.status,
            duration: r.duration,
            response_size: r.response_size,
            failed: r.error.is_some() || r.status.is_some_and(|s| s >= 400),
        })
        .collect()
}

pub fn http_detail(id: u64) -> Option<HttpRecord> {
    let store = STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    store.http.iter().find(|r| r.id == id).cloned()
}

pub fn socket_summaries() -> Vec<SocketSummary> {
    let store = STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    store
        .sockets
        .iter()
        .map(|r| SocketSummary {
            id: r.id,
            at: r.at,
            socket: r.socket,
            direction: r.direction,
            op: r.op,
            event: r.event.clone(),
            size: r.size,
            binary: r.binary,
        })
        .collect()
}

pub fn socket_detail(id: u64) -> Option<SocketRecord> {
    let store = STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    store.sockets.iter().find(|r| r.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_at_any_depth_but_nulls_stay_null() {
        let text = r#"{"op":2,"d":{"token":"abc","properties":{"os":"x"},"list":[{"password":"p"}],"code":null}}"#;
        let redacted: Value = serde_json::from_str(&redact_text(text)).unwrap();
        assert_eq!(redacted["d"]["token"], REDACTED);
        assert_eq!(redacted["d"]["list"][0]["password"], REDACTED);
        assert_eq!(redacted["d"]["properties"]["os"], "x");
        assert!(redacted["d"]["code"].is_null());
    }

    #[test]
    fn non_json_text_passes_through_and_long_text_is_cut_on_a_char_boundary() {
        assert_eq!(redact_text("plain"), "plain");
        let cut = truncate("ééé".to_string(), 3);
        assert!(cut.starts_with('é') && cut.contains("more bytes"), "{cut}");
    }

    #[test]
    fn a_frame_is_described_by_its_op_and_event_name() {
        let (op, event, payload) = describe_frame(r#"{"op":0,"t":"MESSAGE_CREATE","s":4,"d":{"content":"hi"}}"#);
        assert_eq!((op, event.as_deref()), (Some(0), Some("MESSAGE_CREATE")));
        assert!(payload.contains("\"hi\""));
    }

    #[test]
    fn http_exchanges_are_recorded_and_completed_without_their_secrets() {
        let id = begin_http("POST", "/auth/login", Some(br#"{"login":"me","password":"hunter2"}"#));
        finish_http(id, Some(200), Duration::from_millis(12), Ok(br#"{"token":"secret","user_id":"1"}"#));
        let record = http_detail(id).expect("recorded");
        assert!(!record.request_body.as_deref().unwrap().contains("hunter2"));
        assert!(!record.response_body.as_deref().unwrap().contains("secret"));
        assert_eq!((record.status, record.response_size), (Some(200), Some(32)));
        assert!(http_summaries().iter().any(|s| s.id == id && !s.failed));
    }
}
