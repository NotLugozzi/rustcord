use crate::backend::{BackendEvent, GATEWAY_TX};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone)]
pub struct RpcAppInfo {
    pub conn_id: u64,
    pub name: String,
    pub detail: String,
    pub enabled: bool,
    pub icon_url: Option<String>,
    pub image_url: Option<String>,
}

struct App {
    icon_url: Option<String>,
    image_url: Option<String>,
    name: String,
    activity: Value,
    enabled: bool,
}

struct State {
    status: String,
    custom: Option<String>,
    manual: Option<Value>,
    apps: BTreeMap<u64, App>,
    me: Value,
}

static STATE: Mutex<State> =
    Mutex::new(State { status: String::new(), custom: None, manual: None, apps: BTreeMap::new(), me: Value::Null });

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(&mut STATE.lock().unwrap())
}

pub fn set_me(user: Value) {
    with(|s| s.me = user);
}

pub fn me() -> Value {
    with(|s| s.me.clone())
}

pub fn set_status(status: &str, custom: Option<&str>) {
    with(|s| {
        s.status = status.to_string();
        s.custom = custom.filter(|t| !t.is_empty()).map(str::to_string);
    });
}

pub fn set_manual(activity: Option<Value>) {
    with(|s| s.manual = activity);
}

pub fn set_app(conn_id: u64, name: String, icon_url: Option<String>, image_url: Option<String>, activity: Value) {
    with(|s| {
        let enabled = s.apps.get(&conn_id).map_or(true, |a| a.enabled);
        s.apps.insert(conn_id, App { name, icon_url, image_url, activity, enabled });
    });
}

pub fn remove_app(conn_id: u64) {
    with(|s| s.apps.remove(&conn_id).is_some());
}

pub fn set_enabled(conn_id: u64, enabled: bool) {
    with(|s| {
        if let Some(a) = s.apps.get_mut(&conn_id) {
            a.enabled = enabled;
        }
    });
}

pub fn publish(event_tx: &UnboundedSender<BackendEvent>) {
    let (frame, infos) = with(|s| {
        let mut activities: Vec<Value> = Vec::new();
        if let Some(text) = &s.custom {
            activities.push(json!({ "type": 4, "name": "Custom Status", "state": text }));
        }
        activities.extend(s.manual.clone());
        activities.extend(s.apps.values().filter(|a| a.enabled).map(|a| a.activity.clone()));
        let status = if s.status.is_empty() { "online" } else { &s.status };
        let infos = s
            .apps
            .iter()
            .map(|(id, a)| RpcAppInfo {
                conn_id: *id,
                name: a.name.clone(),
                detail: [a.activity["details"].as_str(), a.activity["state"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(" - "),
                enabled: a.enabled,
                icon_url: a.icon_url.clone(),
                image_url: a.image_url.clone(),
            })
            .collect::<Vec<_>>();
        (rustcord_client::gateway::presence_frame(status, &activities), infos)
    });
    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
        let _ = tx.send(frame);
    }
    let _ = event_tx.send(BackendEvent::RpcApps(infos));
}
