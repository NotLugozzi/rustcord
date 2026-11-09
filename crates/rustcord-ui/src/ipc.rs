use crate::backend::BackendEvent;
use crate::presence;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);
static APP_INFO: Mutex<Option<HashMap<String, AppInfo>>> = Mutex::new(None);
const MAX_IPC_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_IPC_CLIENTS: usize = 32;

fn valid_message_len(len: usize) -> bool { len <= MAX_IPC_MESSAGE_BYTES }

#[cfg(unix)]
fn set_socket_permissions(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(unix)]
fn private_runtime_dir(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(dir) else { return false };
    let uid = unsafe { libc::geteuid() };
    meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0
}

#[cfg(unix)]
pub async fn serve(event_tx: UnboundedSender<BackendEvent>) {
    use tokio::net::{UnixListener, UnixStream};
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from) else {
        tracing::warn!("RPC disabled: XDG_RUNTIME_DIR is unavailable");
        return;
    };
    let uid = unsafe { libc::geteuid() };
    if !private_runtime_dir(&dir) {
        tracing::warn!("RPC disabled: runtime directory ownership or permissions are unsafe");
        return;
    }
    let mut bound = None;
    for n in 0..10 {
        let path = dir.join(format!("discord-ipc-{n}"));
        if path.exists() {
            if UnixStream::connect(&path).await.is_ok() {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_socket() && meta.uid() == uid && std::fs::remove_file(&path).is_err() { continue; }
        }
        if let Ok(l) = UnixListener::bind(&path) {
            if set_socket_permissions(&path).is_err() {
                drop(l);
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if n > 0 {
                tracing::warn!("discord-ipc-0 is taken; listening on {path:?}, apps will likely talk to the other client instead");
            } else {
                tracing::info!("RPC listening on {path:?}");
            }
            bound = Some(l);
            break;
        }
    }
    let Some(listener) = bound else {
        tracing::warn!("no free discord-ipc socket; RPC disabled");
        return;
    };
    let clients = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_IPC_CLIENTS));
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let Ok(permit) = clients.clone().try_acquire_owned() else { drop(stream); continue };
        let events = event_tx.clone();
        tokio::spawn(async move { let _permit = permit; handle(stream, events).await; });
    }
}

#[cfg(not(unix))]
pub async fn serve(_event_tx: UnboundedSender<BackendEvent>) {}

#[cfg(unix)]
async fn handle(mut stream: tokio::net::UnixStream, event_tx: UnboundedSender<BackendEvent>) {
    let conn_id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let mut app_id = String::new();
    loop {
        let mut head = [0u8; 8];
        if stream.read_exact(&mut head).await.is_err() {
            break;
        }
        let op = u32::from_le_bytes(head[..4].try_into().unwrap());
        let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
        if !valid_message_len(len) {
            break;
        }
        let mut buf = vec![0u8; len];
        if stream.read_exact(&mut buf).await.is_err() {
            break;
        }
        let msg: Value = serde_json::from_slice(&buf).unwrap_or(Value::Null);
        let reply = match op {
            0 => {
                app_id = msg["client_id"].as_str().unwrap_or_default().to_string();
                Some((1, json!({
                    "cmd": "DISPATCH", "evt": "READY", "nonce": null,
                    "data": { "v": 1, "user": presence::me(),
                        "config": { "cdn_host": "cdn.discordapp.com", "api_endpoint": "//discord.com/api", "environment": "production" } }
                })))
            }
            1 => Some((1, handle_command(&msg, conn_id, &app_id, &event_tx).await)),
            2 => break,
            3 => {
                let _ = write_frame(&mut stream, 4, &msg).await;
                None
            }
            _ => None,
        };
        if let Some((op, body)) = reply {
            if write_frame(&mut stream, op, &body).await.is_err() {
                break;
            }
        }
    }
    presence::remove_app(conn_id);
    presence::publish(&event_tx);
}

#[cfg(all(test, unix))]
mod security_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_runtime_directory_requires_owned_private_directory() {
        let path = std::env::temp_dir().join(format!("rustcord-ipc-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(private_runtime_dir(&path));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!private_runtime_dir(&path));
        std::fs::remove_dir(&path).unwrap();
    }

    #[test]
    fn ipc_message_limit_rejects_oversized_frames() {
        assert!(valid_message_len(MAX_IPC_MESSAGE_BYTES));
        assert!(!valid_message_len(MAX_IPC_MESSAGE_BYTES + 1));
    }

    #[tokio::test]
    async fn socket_mode_helper_and_client_limit_are_restrictive() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = std::env::temp_dir().join(format!("rustcord-ipc-socket-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("socket-permissions-probe");
        std::fs::write(&socket, []).unwrap();
        set_socket_permissions(&socket).unwrap();
        assert_eq!(std::fs::symlink_metadata(&socket).unwrap().mode() & 0o777, 0o600);
        let clients = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_IPC_CLIENTS));
        let mut permits = Vec::new();
        for _ in 0..MAX_IPC_CLIENTS { permits.push(clients.clone().acquire_owned().await.unwrap()); }
        assert!(clients.clone().try_acquire_owned().is_err());
        drop(permits);
        std::fs::remove_file(socket).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(unix)]
async fn write_frame(stream: &mut tokio::net::UnixStream, op: u32, body: &Value) -> std::io::Result<()> {
    let payload = body.to_string();
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload.as_bytes());
    stream.write_all(&out).await
}

async fn handle_command(msg: &Value, conn_id: u64, app_id: &str, event_tx: &UnboundedSender<BackendEvent>) -> Value {
    let cmd = msg["cmd"].as_str().unwrap_or_default();
    let mut data = json!({});
    if cmd == "SET_ACTIVITY" {
        let activity = &msg["args"]["activity"];
        if activity.is_null() {
            presence::remove_app(conn_id);
        } else {
            let info = app_info(app_id).await;
            let image_url = activity["assets"]["large_image"].as_str().and_then(|key| {
                let key = info.assets.get(key).map_or(key, String::as_str);
                rustcord_client::models::activity_image_url(Some(app_id), key)
            });
            let gateway_activity = to_gateway_activity(app_id, &info.name, activity);
            presence::set_app(conn_id, info.name, info.icon_url, image_url, gateway_activity);
            data = activity.clone();
            data["application_id"] = json!(app_id);
        }
        presence::publish(event_tx);
    }
    json!({ "cmd": cmd, "data": data, "evt": null, "nonce": msg["nonce"] })
}

fn to_gateway_activity(app_id: &str, name: &str, rpc: &Value) -> Value {
    let mut a = json!({ "name": name, "type": rpc["type"].as_u64().unwrap_or(0), "application_id": app_id });
    for key in ["details", "state", "assets", "party", "instance", "flags"] {
        if !rpc[key].is_null() {
            a[key] = rpc[key].clone();
        }
    }
    if let Some(ts) = rpc["timestamps"].as_object() {
        let ms = |v: &Value| v.as_u64().map(|t| if t < 100_000_000_000 { t * 1000 } else { t });
        let mut out = json!({});
        for k in ["start", "end"] {
            if let Some(t) = ts.get(k).and_then(ms) {
                out[k] = json!(t);
            }
        }
        a["timestamps"] = out;
    }
    a
}

#[derive(Clone, Default)]
struct AppInfo {
    name: String,
    icon_url: Option<String>,
    assets: HashMap<String, String>,
}

async fn app_info(app_id: &str) -> AppInfo {
    if let Some(info) = APP_INFO.lock().unwrap().get_or_insert_with(HashMap::new).get(app_id) {
        return info.clone();
    }
    let client = crate::backend::http_client();
    let get = |url: String| async move { client.get(url).send().await.ok()?.json::<Value>().await.ok() };
    let (rpc, assets) = tokio::join!(
        get(format!("https://discord.com/api/v10/applications/{app_id}/rpc")),
        get(format!("https://discord.com/api/v10/oauth2/applications/{app_id}/assets")),
    );
    let rpc = rpc.unwrap_or_default();
    let info = AppInfo {
        name: rpc["name"].as_str().unwrap_or("Unknown app").to_string(),
        icon_url: rpc["icon"].as_str().map(|h| format!("https://cdn.discordapp.com/app-icons/{app_id}/{h}.png?size=64")),
        assets: assets
            .unwrap_or_default()
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["id"].as_str()?.to_string())))
            .collect(),
    };
    APP_INFO.lock().unwrap().get_or_insert_with(HashMap::new).insert(app_id.to_string(), info.clone());
    info
}
