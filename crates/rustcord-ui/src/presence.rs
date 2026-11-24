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
    game: Option<Value>,
    spotify: Option<Value>,
}

static STATE: Mutex<State> = Mutex::new(State {
    status: String::new(),
    custom: None,
    manual: None,
    apps: BTreeMap::new(),
    me: Value::Null,
    game: None,
    spotify: None,
});

static GAME_SHARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_game_sharing(enabled: bool) {
    GAME_SHARING.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn game_sharing() -> bool {
    GAME_SHARING.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_game(activity: Option<Value>) -> bool {
    with(|s| {
        let changed = s.game != activity;
        s.game = activity;
        changed
    })
}

pub fn set_spotify(activity: Option<Value>) -> bool {
    with(|s| {
        let changed = s.spotify != activity;
        s.spotify = activity;
        changed
    })
}

fn gather(s: &State) -> Vec<Value> {
    let mut activities: Vec<Value> = Vec::new();
    if let Some(text) = &s.custom {
        activities.push(json!({ "type": 4, "name": "Custom Status", "state": text }));
    }
    activities.extend(s.manual.clone());
    let apps: Vec<Value> = s.apps.values().filter(|a| a.enabled).map(|a| a.activity.clone()).collect();
    if apps.is_empty() {
        activities.extend(s.game.clone());
    }
    activities.extend(apps);
    if s.status != "invisible" {
        activities.extend(s.spotify.clone());
    }
    activities
}

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
    let frame = with(|s| {
        let activities = gather(s);
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
        let frame = (!s.status.is_empty()).then(|| rustcord_client::gateway::presence_frame(status, &activities));
        (frame, activities, infos)
    });
    let (frame, activities, infos) = (frame.0, frame.1, frame.2);
    if let (Some(frame), Some(tx)) = (frame, GATEWAY_TX.lock().unwrap().as_ref()) {
        let _ = tx.send(frame);
    }
    let _ = event_tx.send(BackendEvent::OwnActivities(activities));
    let _ = event_tx.send(BackendEvent::RpcApps(infos));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State { status: "online".into(), custom: None, manual: None, apps: BTreeMap::new(), me: Value::Null, game: None, spotify: None }
    }

    #[test]
    fn a_detected_game_steps_aside_for_rich_presence_and_spotify_hides_when_invisible() {
        let mut s = state();
        s.game = Some(json!({ "type": 0, "name": "Celeste" }));
        s.spotify = Some(json!({ "type": 2, "name": "Spotify" }));
        let kinds = |s: &State| gather(s).iter().map(|a| a["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(kinds(&s), ["Celeste", "Spotify"]);
        s.apps.insert(1, App { icon_url: None, image_url: None, name: "osu!".into(), activity: json!({ "type": 0, "name": "osu!" }), enabled: true });
        assert_eq!(kinds(&s), ["osu!", "Spotify"]);
        s.status = "invisible".into();
        assert_eq!(kinds(&s), ["osu!"]);
    }
}

