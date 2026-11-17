use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const PLAYER: &str = "https://api.spotify.com/v1/me/player";
const DEALER: &str = "wss://dealer.spotify.com/";
const PING_EVERY: Duration = Duration::from_secs(30);
const MIN_REFRESH: Duration = Duration::from_secs(1);
const END_GRACE_MS: i64 = 5_000;
const MAX_TEXT: usize = 128;
const MAX_ARTISTS: usize = 5;
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ConnectedAccount {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub show_activity: bool,
    #[serde(default)]
    pub access_token: Option<String>,
}

impl ConnectedAccount {
    pub fn shares_spotify(&self) -> bool {
        self.kind == "spotify" && self.show_activity
    }
}

fn cut(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_TEXT - 1).collect();
    out.push('\u{2026}');
    out
}

fn cover_id(url: &str) -> Option<&str> {
    url.rsplit('/').next().filter(|id| !id.is_empty())
}

pub fn activity(player: &Value, discord_user_id: &str, now_ms: i64) -> Option<Value> {
    if !player["is_playing"].as_bool().unwrap_or(false) {
        return None;
    }
    if player["device"]["is_private_session"].as_bool().unwrap_or(false) {
        return None;
    }
    if player["currently_playing_type"].as_str().is_some_and(|kind| kind != "track") {
        return None;
    }
    let item = player.get("item").filter(|item| !item.is_null())?;
    let song = item["name"].as_str().filter(|name| !name.is_empty())?;
    let duration = item["duration_ms"].as_i64().unwrap_or(0);
    let progress = player["progress_ms"].as_i64().unwrap_or(0);
    let start = now_ms - progress;
    let end = start + duration;
    if duration > 0 && now_ms > end + END_GRACE_MS {
        return None;
    }
    let artists: Vec<&Value> = item["artists"].as_array().map(|a| a.iter().take(MAX_ARTISTS).collect()).unwrap_or_default();
    let artist_names: Vec<&str> = artists.iter().filter_map(|a| a["name"].as_str()).collect();
    let album = &item["album"];
    let mut activity = json!({
        "type": 2,
        "name": "Spotify",
        "details": cut(song),
        "state": cut(&artist_names.join("; ")),
        "timestamps": { "start": start, "end": end },
    });
    let mut assets = serde_json::Map::new();
    if let Some(cover) = album["images"].as_array().and_then(|images| images.first()).and_then(|image| image["url"].as_str()).and_then(cover_id) {
        assets.insert("large_image".into(), json!(format!("spotify:{cover}")));
    }
    if let Some(name) = album["name"].as_str() {
        assets.insert("large_text".into(), json!(cut(name)));
    }
    if !assets.is_empty() {
        activity["assets"] = Value::Object(assets);
    }
    let local = item["is_local"].as_bool().unwrap_or(false);
    if let (false, Some(track_id)) = (local, item["id"].as_str()) {
        activity["party"] = json!({ "id": format!("spotify:{discord_user_id}") });
        activity["sync_id"] = json!(track_id);
        activity["flags"] = json!(48);
        let album_id = album["id"].as_str();
        activity["metadata"] = json!({
            "album_id": album_id,
            "artist_ids": artists.iter().filter_map(|a| a["id"].as_str()).collect::<Vec<_>>(),
            "context_uri": album_id.map(|id| format!("spotify:album:{id}")),
            "type": "track",
            "button_urls": [],
        });
    }
    Some(activity)
}

pub fn ends_at(activity: &Value) -> Option<i64> {
    activity["timestamps"]["end"].as_i64()
}

fn connection_id(message: &Value) -> Option<String> {
    let uri = message["uri"].as_str()?;
    if !uri.starts_with("hm://pusher/v1/connections/") {
        return None;
    }
    message["headers"]["Spotify-Connection-Id"].as_str().map(str::to_string)
}

fn mentions_player_change(message: &Value) -> bool {
    message["payloads"].as_array().into_iter().flatten().any(|payload| {
        payload["events"].as_array().into_iter().flatten().any(|event| {
            matches!(event["type"].as_str(), Some("PLAYER_STATE_CHANGED" | "DEVICE_STATE_CHANGED"))
        })
    })
}

enum PlayerRead {
    Playing(Value),
    Idle,
    Expired,
}

async fn read_player(http: &reqwest::Client, token: &str) -> Result<PlayerRead, String> {
    let response = http.get(PLAYER).bearer_auth(token).send().await.map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        200 => Ok(PlayerRead::Playing(response.json().await.map_err(|e| e.to_string())?)),
        204 => Ok(PlayerRead::Idle),
        401 => Ok(PlayerRead::Expired),
        status => Err(format!("Spotify answered {status}")),
    }
}

pub struct Follower<F> {
    pub account: ConnectedAccount,
    pub discord_user_id: String,
    pub discord: crate::rest::RestClient,
    pub publish: F,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl<F: FnMut(Option<Value>) + Send> Follower<F> {
    async fn token(&mut self, renew: bool) -> Option<String> {
        if !renew {
            if let Some(token) = self.account.access_token.clone() {
                return Some(token);
            }
        }
        match self.discord.spotify_token(&self.account.id).await {
            Ok(token) => {
                self.account.access_token = Some(token.clone());
                Some(token)
            }
            Err(e) => {
                tracing::warn!("couldn't get a Spotify token from Discord: {e}");
                None
            }
        }
    }

    async fn refresh(&mut self, http: &reqwest::Client, token: &str) -> Result<bool, String> {
        match read_player(http, token).await? {
            PlayerRead::Playing(player) => {
                (self.publish)(activity(&player, &self.discord_user_id, now_ms()));
                Ok(true)
            }
            PlayerRead::Idle => {
                (self.publish)(None);
                Ok(true)
            }
            PlayerRead::Expired => Ok(false),
        }
    }

    pub async fn run(mut self) {
        let http = reqwest::Client::new();
        let mut backoff = Duration::from_millis(500);
        let mut renew = false;
        loop {
            let Some(token) = self.token(renew).await else { return };
            renew = false;
            match self.refresh(&http, &token).await {
                Ok(true) => {}
                Ok(false) => {
                    renew = true;
                    continue;
                }
                Err(e) => tracing::debug!("Spotify player read failed: {e}"),
            }
            match self.listen(&http, &token).await {
                Ok(()) => backoff = Duration::from_millis(500),
                Err(e) => tracing::debug!("Spotify connection ended: {e}"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn listen(&mut self, http: &reqwest::Client, token: &str) -> Result<(), String> {
        let url = format!("{DEALER}?access_token={token}");
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.map_err(|_| "couldn't reach Spotify".to_string())?;
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        let mut silent_pings = 0u32;
        let mut last_refresh = tokio::time::Instant::now() - MIN_REFRESH;
        let mut end_check: Option<tokio::time::Instant> = None;
        loop {
            let deadline = end_check.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));
            let frame = tokio::select! {
                _ = ping.tick() => {
                    silent_pings += 1;
                    if silent_pings > 3 {
                        return Err("Spotify went quiet".into());
                    }
                    socket.send(WsMessage::Text(json!({ "type": "ping" }).to_string())).await.map_err(|e| e.to_string())?;
                    continue;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    end_check = None;
                    if !self.refresh(http, token).await? {
                        return Err("Spotify token expired".into());
                    }
                    continue;
                }
                frame = socket.next() => frame,
            };
            silent_pings = 0;
            let text = match frame {
                Some(Ok(WsMessage::Text(text))) => text,
                Some(Ok(WsMessage::Close(_))) | None => return Ok(()),
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(e.to_string()),
            };
            let Ok(message) = serde_json::from_str::<Value>(&text) else { continue };
            if let Some(connection) = connection_id(&message) {
                let url = format!("https://api.spotify.com/v1/me/notifications/player?connection_id={connection}");
                http.put(url).bearer_auth(token).header("Content-Length", "0").send().await.map_err(|e| e.to_string())?;
                continue;
            }
            if mentions_player_change(&message) && last_refresh.elapsed() >= MIN_REFRESH {
                last_refresh = tokio::time::Instant::now();
                match read_player(http, token).await? {
                    PlayerRead::Playing(player) => {
                        let current = activity(&player, &self.discord_user_id, now_ms());
                        end_check = current.as_ref().and_then(ends_at).map(|end| {
                            let wait = (end + END_GRACE_MS - now_ms()).max(0) as u64;
                            tokio::time::Instant::now() + Duration::from_millis(wait)
                        });
                        (self.publish)(current);
                    }
                    PlayerRead::Idle => (self.publish)(None),
                    PlayerRead::Expired => return Err("Spotify token expired".into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(extra: Value) -> Value {
        let mut value = json!({
            "is_playing": true, "progress_ms": 30_000, "currently_playing_type": "track",
            "device": { "is_private_session": false },
            "item": {
                "id": "track1", "name": "Song", "duration_ms": 200_000, "is_local": false,
                "album": { "id": "album1", "name": "Album", "images": [ { "url": "https://i.scdn.co/image/ab67cover" } ] },
                "artists": [ { "name": "A", "id": "a1" }, { "name": "B", "id": "b1" } ]
            }
        });
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        value
    }

    #[test]
    fn a_playing_track_becomes_discords_spotify_activity() {
        let now = 1_000_000;
        let activity = activity(&player(json!({})), "me", now).unwrap();
        assert_eq!(activity["type"], 2);
        assert_eq!(activity["name"], "Spotify");
        assert_eq!(activity["details"], "Song");
        assert_eq!(activity["state"], "A; B");
        assert_eq!(activity["timestamps"], json!({ "start": now - 30_000, "end": now - 30_000 + 200_000 }));
        assert_eq!(activity["assets"], json!({ "large_image": "spotify:ab67cover", "large_text": "Album" }));
        assert_eq!(activity["party"]["id"], "spotify:me");
        assert_eq!(activity["sync_id"], "track1");
        assert_eq!(activity["flags"], 48);
        assert_eq!(activity["metadata"]["context_uri"], "spotify:album:album1");
        assert_eq!(activity["metadata"]["artist_ids"], json!(["a1", "b1"]));
    }

    #[test]
    fn nothing_is_shared_when_paused_private_an_ad_or_long_over() {
        assert!(activity(&player(json!({ "is_playing": false })), "me", 0).is_none());
        assert!(activity(&player(json!({ "device": { "is_private_session": true } })), "me", 0).is_none());
        assert!(activity(&player(json!({ "currently_playing_type": "ad" })), "me", 0).is_none());
        let stale = player(json!({ "progress_ms": 210_000 }));
        assert!(activity(&stale, "me", 0).is_none(), "ten seconds past the end");
    }

    #[test]
    fn a_local_file_has_no_ids_and_long_text_is_cut() {
        let mut local = player(json!({}));
        local["item"]["is_local"] = json!(true);
        local["item"]["name"] = json!("x".repeat(300));
        let activity = activity(&local, "me", 1_000_000).unwrap();
        assert!(activity.get("sync_id").is_none() && activity.get("party").is_none() && activity.get("metadata").is_none());
        assert_eq!(activity["details"].as_str().unwrap().chars().count(), MAX_TEXT);
    }

    #[test]
    fn the_dealer_hands_over_a_connection_id_then_player_events() {
        let hello = json!({ "type": "message", "uri": "hm://pusher/v1/connections/xyz", "headers": { "Spotify-Connection-Id": "conn-1" } });
        assert_eq!(connection_id(&hello).as_deref(), Some("conn-1"));
        let change = json!({ "type": "message", "payloads": [ { "events": [ { "type": "PLAYER_STATE_CHANGED", "event": {} } ] } ] });
        assert!(mentions_player_change(&change));
        assert!(connection_id(&change).is_none());
        assert!(!mentions_player_change(&json!({ "type": "pong" })));
    }

    #[test]
    fn only_spotify_links_that_share_activity_are_followed() {
        let link: ConnectedAccount = serde_json::from_value(json!({ "type": "spotify", "id": "s", "name": "me", "show_activity": true })).unwrap();
        assert!(link.shares_spotify());
        let hidden: ConnectedAccount = serde_json::from_value(json!({ "type": "spotify", "id": "s", "show_activity": false })).unwrap();
        assert!(!hidden.shares_spotify());
        let github: ConnectedAccount = serde_json::from_value(json!({ "type": "github", "id": "g", "show_activity": true })).unwrap();
        assert!(!github.shares_spotify());
    }
}
