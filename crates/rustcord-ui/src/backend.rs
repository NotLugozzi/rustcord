use crate::presence;
use crate::session_store;
use rustcord_client::captcha;
use rustcord_client::error::ClientError;
use rustcord_client::gateway::{Gateway, GatewayEvent};
use rustcord_client::rest::{CaptchaChallenge, LoginResponse, LoginSession, RestClient};
use tokio::sync::mpsc;

fn mime_guess_from_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

pub(crate) static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn cache_file(url: &str) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    directories::ProjectDirs::from("dev", "rustcord", "rustcord").map(|d| d.cache_dir().join("images").join(format!("{:016x}", h.finish())))
}

const MAX_DISK_CACHED_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_DOWNLOAD_BYTES: usize = 24 * 1024 * 1024;
const MAX_VOICE_CLIP_BYTES: usize = 24 * 1024 * 1024;
const MAX_ATTACHMENT_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ATTACHMENT_DOWNLOAD_TIME: std::time::Duration = std::time::Duration::from_secs(180);
static MEDIA_DOWNLOADS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(6)));
static IMAGE_DECODERS: std::sync::LazyLock<tokio::sync::Semaphore> = std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(2));

pub(crate) fn http_client() -> &'static reqwest::Client {
    HTTP.get_or_init(|| reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().expect("valid HTTP client configuration"))
}

fn safe_reqwest_error(error: reqwest::Error) -> String { error.without_url().to_string() }

fn response_fits_limit(content_length: Option<u64>, limit: usize) -> bool {
    content_length.is_none_or(|size| size <= limit as u64)
}

fn chunk_fits_limit(current: usize, chunk: usize, limit: usize) -> bool {
    current.checked_add(chunk).is_some_and(|size| size <= limit)
}

async fn fetch_image_bytes(url: &str) -> Option<Vec<u8>> {
    let _permit = MEDIA_DOWNLOADS.acquire().await.ok()?;
    let path = cache_file(url);
    if let Some(cached_path) = &path {
        if std::fs::metadata(cached_path).is_ok_and(|meta| meta.len() <= MAX_IMAGE_DOWNLOAD_BYTES as u64) {
            if let Ok(bytes) = std::fs::read(cached_path) { return Some(bytes); }
        }
    }
    let client = http_client();
    let resp = client.get(url).send().await.and_then(|r| r.error_for_status());
    let bytes = match resp {
        Ok(mut r) => {
            if !response_fits_limit(r.content_length(), MAX_IMAGE_DOWNLOAD_BYTES) {
                tracing::warn!("image response exceeded configured size limit");
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = r.chunk().await.ok()? {
                if !chunk_fits_limit(bytes.len(), chunk.len(), MAX_IMAGE_DOWNLOAD_BYTES) {
                    tracing::warn!("image response exceeded configured size limit");
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        },
        Err(e) => {
            tracing::warn!("failed to download image: {}", e.without_url());
            return None;
        }
    };
    if let Some(p) = path.filter(|_| bytes.len() <= MAX_DISK_CACHED_BYTES) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, &bytes);
    }
    Some(bytes)
}

async fn decode_image_bytes(bytes: Vec<u8>) -> crate::images::Decoded {
    let Ok(_permit) = IMAGE_DECODERS.acquire().await else { return crate::images::Decoded::Failed };
    tokio::task::spawn_blocking(move || crate::images::decode(&bytes)).await.unwrap_or(crate::images::Decoded::Failed)
}

async fn load_voice_clip(url: &str) -> Result<std::sync::Arc<Vec<i16>>, String> {
    let _permit = MEDIA_DOWNLOADS.acquire().await.map_err(|_| "download service unavailable".to_string())?;
    let client = http_client();
    let mut resp = client.get(url).send().await.and_then(|r| r.error_for_status()).map_err(safe_reqwest_error)?;
    if resp.content_length().is_some_and(|size| size > MAX_VOICE_CLIP_BYTES as u64) { return Err("voice clip exceeds size limit".into()); }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(safe_reqwest_error)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_VOICE_CLIP_BYTES { return Err("voice clip exceeds size limit".into()); }
        bytes.extend_from_slice(&chunk);
    }
    let samples = tokio::task::spawn_blocking(move || rustcord_client::voice_message::decode_ogg(&bytes))
        .await
        .map_err(|e| e.to_string())??;
    Ok(std::sync::Arc::new(samples))
}

async fn send_voice_message(client: &RestClient, channel_id: &str, samples: Vec<i16>, reply_to: Option<&str>) -> Result<(), String> {
    use rustcord_client::voice_message as vm;
    let (ogg, duration, waveform) = tokio::task::spawn_blocking(move || {
        vm::encode_ogg(&samples).map(|ogg| (ogg, vm::duration_secs(&samples), vm::waveform_base64(&samples)))
    })
    .await
    .map_err(|e| e.to_string())??;
    client.send_voice_message(channel_id, ogg, duration, waveform, reply_to).await.map(|_| ()).map_err(|e| e.to_string())
}

async fn save_attachment(url: &str, filename: &str) -> Result<std::path::PathBuf, String> {
    use tokio::io::AsyncWriteExt;
    let _permit = MEDIA_DOWNLOADS.acquire().await.map_err(|_| "download service unavailable".to_string())?;
    let dir = directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .ok_or("no Downloads folder")?;
    let name = std::path::Path::new(filename).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("attachment");
    let client = http_client();
    let mut resp = client.get(url).timeout(MAX_ATTACHMENT_DOWNLOAD_TIME).send().await.and_then(|r| r.error_for_status()).map_err(safe_reqwest_error)?;
    if resp.content_length().is_some_and(|size| size > MAX_ATTACHMENT_DOWNLOAD_BYTES) { return Err("attachment exceeds size limit".into()); }
    let (path, mut file) = create_attachment_file(&dir, name).await?;
    let mut written = 0u64;
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(safe_reqwest_error(error));
            }
        };
        written = written.saturating_add(chunk.len() as u64);
        if written > MAX_ATTACHMENT_DOWNLOAD_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err("attachment exceeds size limit".into());
        }
        if let Err(error) = file.write_all(&chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error.to_string());
        }
    }
    if let Err(error) = file.flush().await {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.to_string());
    }
    Ok(path)
}

async fn create_attachment_file(dir: &std::path::Path, name: &str) -> Result<(std::path::PathBuf, tokio::fs::File), String> {
    use tokio::fs::OpenOptions;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.to_string(), String::new()),
    };
    for suffix in 0..1000 {
        let candidate_name = if suffix == 0 { name.to_string() } else { format!("{stem} ({suffix}){ext}") };
        let path = dir.join(candidate_name);
        match OpenOptions::new().write(true).create_new(true).open(&path).await {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not choose a unique attachment filename".into())
}

pub(crate) static GATEWAY_TX: std::sync::Mutex<Option<mpsc::UnboundedSender<String>>> = std::sync::Mutex::new(None);
static ACTIVE_GATEWAY: std::sync::Mutex<Option<(tokio::sync::watch::Sender<bool>, tokio::task::JoinHandle<()>)>> = std::sync::Mutex::new(None);
static CREDENTIAL_OPS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn serialized_credential_op<T, F>(op: F) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let _guard = CREDENTIAL_OPS.lock().await;
    tokio::task::spawn_blocking(op).await
}

async fn stop_gateway() {
    *GATEWAY_TX.lock().unwrap_or_else(|p| p.into_inner()) = None;
    let active = ACTIVE_GATEWAY.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some((cancel, mut task)) = active {
        let _ = cancel.send(true);
        if tokio::time::timeout(std::time::Duration::from_secs(3), &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }
}

#[cfg(test)]
mod credential_serialization_tests {
    use super::*;
    static GATEWAY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn guild_icons_are_cropped_to_a_512_square_png() {
        let path = std::env::temp_dir().join(format!("rustcord-icon-test-{}.png", std::process::id()));
        image::RgbImage::new(1000, 600).save(&path).unwrap();
        let png = encode_guild_icon(&path);
        let _ = std::fs::remove_file(&path);
        let icon = image::load_from_memory_with_format(&png.unwrap(), image::ImageFormat::Png).unwrap();
        assert_eq!((icon.width(), icon.height()), (512, 512));
    }

    #[test]
    fn streamed_download_limits_cover_absent_and_inaccurate_length_headers() {
        assert!(response_fits_limit(None, 10));
        assert!(response_fits_limit(Some(8), 10));
        assert!(!response_fits_limit(Some(11), 10));
        assert!(chunk_fits_limit(0, 6, 10));
        assert!(chunk_fits_limit(6, 4, 10));
        assert!(!chunk_fits_limit(6, 5, 10));
    }

    #[tokio::test]
    async fn image_download_and_decode_concurrency_are_bounded() {
        let mut downloads = Vec::new();
        for _ in 0..6 { downloads.push(MEDIA_DOWNLOADS.acquire().await.unwrap()); }
        assert!(MEDIA_DOWNLOADS.try_acquire().is_err());
        drop(downloads);
        let mut decoders = Vec::new();
        for _ in 0..2 { decoders.push(IMAGE_DECODERS.acquire().await.unwrap()); }
        assert!(IMAGE_DECODERS.try_acquire().is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attachment_creation_does_not_follow_existing_symlinks() {
        use tokio::io::AsyncWriteExt;
        let dir = std::env::temp_dir().join(format!("rustcord-attachment-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let protected = dir.join("protected.txt");
        std::fs::write(&protected, b"keep").unwrap();
        std::os::unix::fs::symlink(&protected, dir.join("file.txt")).unwrap();
        let (path, mut output) = create_attachment_file(&dir, "file.txt").await.unwrap();
        assert_eq!(path.file_name().unwrap(), "file (1).txt");
        output.write_all(b"attachment").await.unwrap();
        drop(output);
        assert_eq!(std::fs::read(&protected).unwrap(), b"keep");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_completed_clear_cannot_be_followed_by_a_queued_save() {
        let operations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let save_ops = operations.clone();
        let save_started = started.clone();
        let save = tokio::spawn(async move {
            serialized_credential_op(move || {
                save_ops.lock().unwrap().push("save");
                save_started.store(true, std::sync::atomic::Ordering::Release);
                std::thread::sleep(std::time::Duration::from_millis(30));
            }).await.unwrap();
        });
        while !started.load(std::sync::atomic::Ordering::Acquire) { tokio::task::yield_now().await; }
        let clear_ops = operations.clone();
        let clear = tokio::spawn(async move {
            serialized_credential_op(move || clear_ops.lock().unwrap().push("clear")).await.unwrap();
        });
        save.await.unwrap();
        clear.await.unwrap();
        assert_eq!(*operations.lock().unwrap(), ["save", "clear"]);
    }

    async fn register_fake_gateway() -> (std::sync::Arc<std::sync::atomic::AtomicBool>, mpsc::UnboundedSender<String>) {
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_stopped = stopped.clone();
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let (commands, mut command_rx) = mpsc::unbounded_channel::<String>();
        let task = tokio::spawn(async move {
            tokio::select! { _ = cancelled.changed() => {}, _ = command_rx.recv() => {} }
            task_stopped.store(true, std::sync::atomic::Ordering::Release);
        });
        *GATEWAY_TX.lock().unwrap() = Some(commands.clone());
        *ACTIVE_GATEWAY.lock().unwrap() = Some((cancel, task));
        (stopped, commands)
    }

    #[tokio::test]
    async fn logout_cancels_gateway_clears_sender_and_rejects_old_commands() {
        let _test_guard = GATEWAY_TEST_LOCK.lock().await;
        let (stopped, obsolete) = register_fake_gateway().await;
        stop_gateway().await;
        assert!(stopped.load(std::sync::atomic::Ordering::Acquire));
        assert!(GATEWAY_TX.lock().unwrap().is_none());
        assert!(obsolete.send("stale".into()).is_err());
    }

    #[tokio::test]
    async fn registering_a_new_session_stops_the_previous_gateway_first() {
        let _test_guard = GATEWAY_TEST_LOCK.lock().await;
        let (first_stopped, _) = register_fake_gateway().await;
        stop_gateway().await;
        let (second_stopped, _) = register_fake_gateway().await;
        assert!(first_stopped.load(std::sync::atomic::Ordering::Acquire));
        stop_gateway().await;
        assert!(second_stopped.load(std::sync::atomic::Ordering::Acquire));
    }
}
static UPLOADS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u64, tokio::task::AbortHandle>>> =
    std::sync::LazyLock::new(Default::default);

pub(crate) static VOICE_MEDIA_STATE: std::sync::Mutex<Option<rustcord_client::voice_gateway::VoiceMediaState>> = std::sync::Mutex::new(None);
static ACTIVE_STREAM_KEY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static WATCH_STREAM_KEY: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilters {
    pub author_id: Option<String>,
    pub has: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    Guild { guild_id: String },
    Channel { channel_id: String },
}

pub enum UiCommand {
    LoginWithPassword { email: String, password: String },
    LoginWithToken { token: String },
    SubmitMfa { ticket: String, code: String },
    SelectGuild { guild_id: String },
    LoadAllGuildEmojis { guild_ids: Vec<String> },
    SelectChannel { channel_id: String },
    LoadOlderMessages { channel_id: String, before: String },
    SendMessage { channel_id: String, content: String, reply_to: Option<String> },
    SendSticker { channel_id: String, sticker_id: String, reply_to: Option<String> },
    LoadGuildStickers { guild_ids: Vec<String> },
    LoadStickerPacks,
    TimeoutMember { guild_id: String, user_id: String, duration_secs: Option<i64> },
    SetMemberRole { guild_id: String, user_id: String, role_id: String, add: bool },
    SaveChannel { guild_id: String, channel_id: Option<String>, name: String, topic: Option<String>, voice: bool, parent_id: Option<String> },
    CreateInvite { channel_id: String },
    LoadGuildInvites { guild_id: String },
    RevokeInvite { code: String },
    RenameGuild { guild_id: String, name: String },
    SetGuildIcon { guild_id: String, path: Option<std::path::PathBuf> },
    DeleteGuild { guild_id: String },
    LoadGuildBans { guild_id: String },
    UnbanMember { guild_id: String, user_id: String },
    UpdateGuildSettings { guild_key: String, patch: serde_json::Value },
    SearchGifs { query: String },
    SendAttachments {
        channel_id: String,
        content: String,
        files: Vec<crate::app_state::PendingAttachment>,
        reply_to: Option<String>,
        upload_id: u64,
        progress: std::sync::Arc<rustcord_client::rest::UploadProgress>,
    },
    CancelUpload { upload_id: u64 },
    SendPoll { channel_id: String, poll: serde_json::Value },
    VotePoll { channel_id: String, message_id: String, answer_ids: Vec<u32> },
    EndPoll { channel_id: String, message_id: String },
    LoadVoiceClip { url: String },
    SendVoiceMessage { channel_id: String, samples: Vec<i16>, reply_to: Option<String> },
    ForwardMessage { source: rustcord_client::models::MessageReference, targets: Vec<String>, note: String },
    PreviewAttachment { key: String, source: crate::app_state::AttachmentSource },
    DownloadImage { url: String },
    ShowNotification { title: String, body: String, avatar_url: String },
    #[cfg(feature = "video")]
    OpenVideo { url: String },
    #[cfg(feature = "video")]
    LoadVideoPoster { url: String },
    #[cfg(feature = "video")]
    DecodeVideoBatch { url: String },
    #[cfg(feature = "video")]
    CloseVideo { url: String },
    #[cfg(feature = "video")]
    SeekVideo { url: String, target_secs: f64 },
    SaveAttachment { url: String, filename: String },
    OpenDmWithUser { user_id: String },
    RefreshDmChannels,
    StartTyping { channel_id: String },
    LoadProfile { user_id: String },
    EditMessage { channel_id: String, message_id: String, content: String },
    DeleteMessage { channel_id: String, message_id: String },
    ToggleReaction { channel_id: String, message_id: String, emoji: String, add: bool },
    AckMessage { channel_id: String, message_id: String },
    SearchMessages { scope: SearchScope, query: String, filters: SearchFilters, offset: u32 },
    JumpToMessage { channel_id: String, message_id: String },
    JoinGuildByInvite { invite_code: String },
    ResolveInvite { code: String },
    UpdateRelationship { user_id: String, action: RelationshipAction },
    LoadPins { channel_id: String },
    AddDmRecipients { channel_id: String, existing: Vec<String>, group: bool, user_ids: Vec<String> },
    CreateGuild { name: String },
    RequestGuildMembers { guild_id: String },
    KickMember { guild_id: String, user_id: String },
    BanMember { guild_id: String, user_id: String },
    SetPresence { status: String, custom_status: Option<String> },
    SetManualActivity { name: String, details: String, state: String },
    SetRpcEnabled { conn_id: u64, enabled: bool },
    JoinVoiceChannel { guild_id: Option<String>, channel_id: String, ring: bool },
    DeclineCall { channel_id: String },
    LeaveVoiceChannel { guild_id: Option<String> },
    SetVoiceMuteDeaf { guild_id: Option<String>, channel_id: String, self_mute: bool, self_deaf: bool },
    SetScreenSharing { enabled: bool, include_system_audio: bool, audio_source_pid: Option<u32>, max_height: u32, framerate: u32, guild_id: Option<String>, channel_id: String },
    #[cfg(feature = "screenshare")]
    LoadScreenAudioSources,
    SetCamera { enabled: bool, guild_id: Option<String>, channel_id: String, self_mute: bool, self_deaf: bool },
    SetCameraDevice { device: Option<String> },
    FetchFont { spec: crate::theme::FontSpec },
    LoadCameras,
    WatchStream { stream_key: String },
    StopWatchingStream { stream_key: String },
    SetVoiceInputDevice { device: Option<String> },
    SetMicProcessing { gain: f32, denoise: bool },
    SetUserVolume { user_id: String, volume: f32 },
    SetVoiceOutputDevice { device: Option<String> },
    LoadVoiceDevices,
    RunMicTest { input_device: Option<String>, output_device: Option<String> },
    Logout,
}

pub const HISTORY_PAGE_SIZE: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipAction {
    SendRequest,
    Remove,
    Block,
    Unblock,
}

pub enum InviteFailure {
    Invalid,
    Unavailable,
}

pub enum BackendEvent {
    MfaRequired { ticket: String },
    CaptchaInProgress,
    LoginFailed(String),
    LoggedIn,
    Error(String),
    RpcApps(Vec<crate::presence::RpcAppInfo>),
    ProfileLoaded { user_id: String, profile: rustcord_client::models::UserProfile, user: Option<rustcord_client::models::User> },
    Gateway(GatewayEvent),
    GuildChannelsLoaded { guild_id: String, channels: Vec<rustcord_client::models::Channel> },
    GuildRolesLoaded { guild_id: String, roles: Vec<rustcord_client::models::Role> },
    MyGuildMemberLoaded { guild_id: String, roles: Vec<String> },
    GuildEmojisLoaded { guild_id: String, emojis: Vec<rustcord_client::models::Emoji> },
    GuildStickersLoaded { guild_id: String, stickers: Vec<rustcord_client::models::Sticker> },
    StickerPacksLoaded(Vec<rustcord_client::models::StickerPack>),
    MemberUpdated { guild_id: String, member: rustcord_client::models::GuildMember },
    MemberRoleChanged { guild_id: String, user_id: String, role_id: String, add: bool },
    ChannelSaved(rustcord_client::models::Channel),
    InviteCreated { code: String },
    GuildInvitesLoaded { guild_id: String, result: Result<Vec<rustcord_client::models::Invite>, String> },
    InviteRevoked { code: String },
    GuildEdited(rustcord_client::models::Guild),
    GuildBansLoaded { guild_id: String, result: Result<Vec<rustcord_client::models::Ban>, String> },
    MemberUnbanned { guild_id: String, user_id: String },
    GuildSettingsUpdated(rustcord_client::models::GuildSettings),
    GifsLoaded { query: String, result: Result<Vec<rustcord_client::models::Gif>, String> },
    DmChannelsLoaded(Vec<rustcord_client::models::Channel>),
    MessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    OlderMessagesLoaded { channel_id: String, messages: Vec<rustcord_client::models::Message> },
    SearchResults {
        query: String,
        filters: SearchFilters,
        offset: u32,
        total: u32,
        messages: Vec<rustcord_client::models::Message>,
    },
    SearchIndexing { retry_after: f64 },
    MessagesAroundLoaded { channel_id: String, message_id: String, messages: Vec<rustcord_client::models::Message> },
    SearchFailed(String),
    RelationshipsLoaded(Vec<rustcord_client::models::Relationship>),
    DmOpened(rustcord_client::models::Channel),
    ImageDownloaded { url: String, image: crate::images::Decoded },
    #[cfg(feature = "video")]
    VideoBatchReady(crate::images::VideoBatch),
    #[cfg(feature = "screenshare")]
    RemoteVideoFrame { user_id: String, rgba: image::RgbaImage },
    VoiceConnecting,
    VoiceConnected,
    VoiceDisconnected,
    ScreenShareStopped,
    CameraStopped { error: Option<String> },
    CamerasLoaded(Vec<(String, String)>),
    FontFetched { spec: crate::theme::FontSpec, error: Option<String> },
    #[cfg(feature = "screenshare")]
    ScreenAudioSourcesLoaded { sources: Vec<(u32, String, bool)>, error: Option<String> },
    VoiceSpeaking { user_id: String, speaking: bool },
    VoiceStats(rustcord_client::voice_gateway::VoiceStats),
    #[cfg(feature = "screenshare")]
    ScreenShareStats(rustcord_client::voice_gateway::VoiceStats),
    Notice(String),
    VoiceDevicesLoaded { inputs: Vec<String>, outputs: Vec<String> },
    MemberRemoved { guild_id: String, user_id: String },
    InviteResolved { code: String, result: Result<rustcord_client::models::Invite, InviteFailure> },
    GuildJoined(rustcord_client::models::Guild),
    RelationshipSettled { user_id: String },
    UploadFinished { upload_id: u64 },
    VoiceClipLoaded { url: String, result: Result<std::sync::Arc<Vec<i16>>, String> },
    VoiceMessageSent { channel_id: String, error: Option<String> },
    MessageForwarded { sent: usize, failed: Vec<String> },
    PinsLoaded { channel_id: String, pins: Option<Vec<rustcord_client::models::Message>> },
    MicTestStarted,
    MicTestFinished,
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
) {
    rt.spawn(crate::ipc::serve(event_tx.clone()));
    rt.spawn(async move {
        let mut rest: Option<RestClient> = None;
        let mut login_session: Option<LoginSession> = None;

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                UiCommand::LoginWithPassword { email, password } => {
                    let session = match LoginSession::new() {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                            continue;
                        }
                    };
                    attempt_login(email, password, None, &session, &mut rest, &event_tx).await;
                    login_session = Some(session);
                }
                UiCommand::SubmitMfa { ticket, code } => {
                    let Some(session) = &login_session else {
                        let _ = event_tx.send(BackendEvent::LoginFailed("no active login session".into()));
                        continue;
                    };
                    match session.submit_mfa(&ticket, &code).await {
                        Ok(token) => start_session(token, &mut rest, &event_tx).await,
                        Err(e) => {
                            tracing::error!("mfa submit failed: {e}");
                            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                        }
                    }
                }
                UiCommand::SelectGuild { guild_id } => {
                    if let Some(client) = &rest {
                        match client.guild_channels(&guild_id).await {
                            Ok(channels) => {
                                let _ = event_tx.send(BackendEvent::GuildChannelsLoaded { guild_id: guild_id.clone(), channels });
                            }
                            Err(e) => {
                                tracing::error!("failed to load channels for guild {guild_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load channels: {e}")));
                            }
                        }
                        match client.guild_roles(&guild_id).await {
                            Ok(roles) => {
                                let _ = event_tx.send(BackendEvent::GuildRolesLoaded { guild_id: guild_id.clone(), roles });
                            }
                            Err(e) => {
                                tracing::error!("failed to load roles for guild {guild_id}: {e}");
                            }
                        }
                        match client.my_guild_member(&guild_id).await {
                            Ok(member) => {
                                let _ = event_tx.send(BackendEvent::MyGuildMemberLoaded { guild_id: guild_id.clone(), roles: member.roles });
                            }
                            Err(e) => {
                                tracing::error!("failed to load own member for guild {guild_id}: {e}");
                            }
                        }
                        spawn_guild_emoji_loads(client, vec![guild_id], &event_tx);
                    }
                }
                UiCommand::LoadAllGuildEmojis { guild_ids } => {
                    if let Some(client) = &rest {
                        spawn_guild_emoji_loads(client, guild_ids, &event_tx);
                    }
                }
                UiCommand::SelectChannel { channel_id } => {
                    if let Some(client) = &rest {
                        match client.messages(&channel_id, HISTORY_PAGE_SIZE).await {
                            Ok(messages) => {
                                let _ = event_tx.send(BackendEvent::MessagesLoaded { channel_id, messages });
                            }
                            Err(e) => {
                                tracing::error!("failed to load messages for channel {channel_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load messages: {e}")));
                            }
                        }
                    }
                }
                UiCommand::LoadOlderMessages { channel_id, before } => {
                    if let Some(client) = &rest {
                        match client.messages_before(&channel_id, &before, HISTORY_PAGE_SIZE).await {
                            Ok(messages) => {
                                let _ = event_tx.send(BackendEvent::OlderMessagesLoaded { channel_id, messages });
                            }
                            Err(e) => {
                                tracing::error!("failed to load older messages for {channel_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to load older messages: {e}")));
                            }
                        }
                    }
                }
                UiCommand::JumpToMessage { channel_id, message_id } => {
                    if let Some(client) = &rest {
                        match client.messages_around(&channel_id, &message_id, 50).await {
                            Ok(messages) => {
                                let _ = event_tx.send(BackendEvent::MessagesAroundLoaded {
                                    channel_id,
                                    message_id,
                                    messages,
                                });
                            }
                            Err(e) => {
                                tracing::error!("failed to load messages around {message_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to jump: {e}")));
                            }
                        }
                    }
                }
                UiCommand::SearchMessages { scope, query, filters, offset } => {
                    if let Some(client) = &rest {
                        let client = client.clone();
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            run_search(&client, scope, query, filters, offset, &event_tx).await;
                        });
                    }
                }
                UiCommand::SendMessage { channel_id, content, reply_to } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.send_message_reply(&channel_id, &content, reply_to.as_deref()).await {
                            tracing::warn!("send failed: {e}");
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to send message: {e}")));
                        }
                    }
                }
                UiCommand::SendSticker { channel_id, sticker_id, reply_to } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.send_sticker(&channel_id, &sticker_id, reply_to.as_deref()).await {
                            tracing::warn!("sticker send failed: {e}");
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to send sticker: {e}")));
                        }
                    }
                }
                UiCommand::LoadGuildStickers { guild_ids } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            for guild_id in guild_ids {
                                match client.guild_stickers(&guild_id).await {
                                    Ok(stickers) => {
                                        let _ = event_tx.send(BackendEvent::GuildStickersLoaded { guild_id, stickers });
                                    }
                                    Err(e) => tracing::warn!("failed to load stickers for guild {guild_id}: {e}"),
                                }
                            }
                        });
                    }
                }
                UiCommand::LoadStickerPacks => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            match client.sticker_packs().await {
                                Ok(packs) => {
                                    let _ = event_tx.send(BackendEvent::StickerPacksLoaded(packs));
                                }
                                Err(e) => tracing::warn!("failed to load sticker packs: {e}"),
                            }
                        });
                    }
                }
                UiCommand::SearchGifs { query } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let result = client.gifs(&query).await.map_err(|e| e.to_string());
                            let _ = event_tx.send(BackendEvent::GifsLoaded { query, result });
                        });
                    }
                }
                UiCommand::SendAttachments { channel_id, content, files, reply_to, upload_id, progress } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        let task = tokio::spawn(async move {
                            match read_attachments(files).await {
                                Ok(files) => {
                                    let total = files.iter().map(|f| f.bytes.len() as u64).sum();
                                    progress.total.store(total, std::sync::atomic::Ordering::Relaxed);
                                    if let Err(e) = client
                                        .send_message_with_files(&channel_id, &content, files, reply_to.as_deref(), Some(progress))
                                        .await
                                    {
                                        tracing::warn!("attachment send failed: {e}");
                                        let _ = event_tx.send(BackendEvent::Error(format!("failed to send attachments: {e}")));
                                    }
                                }
                                Err(e) => {
                                    let _ = event_tx.send(BackendEvent::Error(e));
                                }
                            }
                            UPLOADS.lock().unwrap().remove(&upload_id);
                            let _ = event_tx.send(BackendEvent::UploadFinished { upload_id });
                        });
                        UPLOADS.lock().unwrap().insert(upload_id, task.abort_handle());
                    }
                }
                UiCommand::PreviewAttachment { key, source } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let bytes = match source {
                            crate::app_state::AttachmentSource::Path(path) => tokio::fs::read(&path).await.ok().map(std::sync::Arc::new),
                            crate::app_state::AttachmentSource::Bytes { bytes, .. } => Some(bytes),
                        };
                        let image = match bytes {
                            Some(bytes) => tokio::task::spawn_blocking(move || crate::images::decode(&bytes))
                                .await
                                .unwrap_or(crate::images::Decoded::Failed),
                            None => crate::images::Decoded::Failed,
                        };
                        let _ = event_tx.send(BackendEvent::ImageDownloaded { url: key, image });
                    });
                }
                UiCommand::SendPoll { channel_id, poll } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = client.send_poll(&channel_id, poll).await {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to post poll: {e}")));
                            }
                        });
                    }
                }
                UiCommand::LoadVoiceClip { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let result = load_voice_clip(&url).await;
                        if let Err(e) = &result {
                            tracing::warn!("voice message failed to load: {e}");
                        }
                        let _ = event_tx.send(BackendEvent::VoiceClipLoaded { url, result });
                    });
                }
                UiCommand::SendVoiceMessage { channel_id, samples, reply_to } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let error = send_voice_message(&client, &channel_id, samples, reply_to.as_deref()).await.err();
                            let _ = event_tx.send(BackendEvent::VoiceMessageSent { channel_id, error });
                        });
                    }
                }
                UiCommand::ForwardMessage { source, targets, note } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let mut sent = 0;
                            let mut failed = Vec::new();
                            for target in targets {
                                match client.forward_message(&target, &source).await {
                                    Ok(_) => {
                                        sent += 1;
                                        if !note.trim().is_empty() {
                                            if let Err(e) = client.send_message(&target, note.trim()).await {
                                                tracing::warn!("forward note to {target} failed: {e}");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("forward to {target} failed: {e}");
                                        failed.push(e.to_string());
                                    }
                                }
                            }
                            let _ = event_tx.send(BackendEvent::MessageForwarded { sent, failed });
                        });
                    }
                }
                UiCommand::VotePoll { channel_id, message_id, answer_ids } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = client.vote_poll(&channel_id, &message_id, &answer_ids).await {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to vote: {e}")));
                            }
                        });
                    }
                }
                UiCommand::EndPoll { channel_id, message_id } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = client.end_poll(&channel_id, &message_id).await {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to end poll: {e}")));
                            }
                        });
                    }
                }
                UiCommand::CancelUpload { upload_id } => {
                    if let Some(task) = UPLOADS.lock().unwrap().remove(&upload_id) {
                        task.abort();
                    }
                }
                UiCommand::DownloadImage { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let image = match fetch_image_bytes(&url).await {
                            Some(bytes) => decode_image_bytes(bytes).await,
                            None => crate::images::Decoded::Failed,
                        };
                        let _ = event_tx.send(BackendEvent::ImageDownloaded { url, image });
                    });
                }
                UiCommand::ShowNotification { title, body, avatar_url } => {
                    tokio::spawn(async move {
                        let image_path = match fetch_image_bytes(&avatar_url).await {
                            Some(bytes) => {
                                let path = cache_file(&avatar_url).map(|path| path.with_extension("notification.png"));
                                let decoded = decode_image_bytes(bytes).await;
                                tokio::task::spawn_blocking(move || {
                                    let path = path?;
                                    std::fs::create_dir_all(path.parent()?).ok()?;
                                    let image = match decoded {
                                        crate::images::Decoded::Still(image) => image,
                                        _ => return None,
                                    };
                                    image.save_with_format(&path, image::ImageFormat::Png).ok()?;
                                    Some(path)
                                })
                                .await
                                .ok()
                                .flatten()
                            }
                            None => None,
                        };
                        let _ = tokio::task::spawn_blocking(move || {
                            let mut notification = notify_rust::Notification::new();
                            notification.summary(&title).body(&body);
                            if let Some(path) = image_path {
                                notification.image_path(&path.to_string_lossy());
                            }
                            crate::show_notification(&notification);
                            rustcord_client::voice_audio::play_notification_sound();
                        })
                        .await;
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::LoadVideoPoster { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let decoded = match poster_source(&url).await {
                            Some(path) => match VIDEO_WORKER_LIMIT.clone().acquire_owned().await {
                                Ok(permit) => tokio::task::spawn_blocking(move || {
                                    let _permit = permit;
                                    match crate::video::first_frame(&path) {
                                        Some(image) => crate::images::Decoded::Still(image),
                                        None => crate::images::Decoded::Failed,
                                    }
                                }).await.unwrap_or(crate::images::Decoded::Failed),
                                Err(_) => crate::images::Decoded::Failed,
                            },
                            None => crate::images::Decoded::Failed,
                        };
                        let _ = event_tx.send(BackendEvent::ImageDownloaded { url: crate::images::poster_key(&url), image: decoded });
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::OpenVideo { url } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let batch = match start_video_download(&url).await {
                            Ok((path, handle)) => {
                                let open_url = url.clone();
                                tokio::task::spawn_blocking(move || open_video(&open_url, &path, &handle)).await.ok().flatten()
                            }
                            Err(e) => {
                                tracing::warn!("failed to fetch video: {e}");
                                forget_download(&url);
                                None
                            }
                        };
                        if let Some(batch) = batch {
                            let _ = event_tx.send(BackendEvent::VideoBatchReady(batch));
                        }
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::DecodeVideoBatch { url } => {
                    let event_tx = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(batch) = next_video_batch(&url) {
                            let _ = event_tx.send(BackendEvent::VideoBatchReady(batch));
                        }
                    });
                }
                #[cfg(feature = "video")]
                UiCommand::CloseVideo { url } => {
                    close_video(&url);
                }
                #[cfg(feature = "video")]
                UiCommand::SeekVideo { url, target_secs } => {
                    let event_tx = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(batch) = seek_video(&url, target_secs) {
                            let _ = event_tx.send(BackendEvent::VideoBatchReady(batch));
                        }
                    });
                }
                UiCommand::SaveAttachment { url, filename } => {
                    let event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        let event = match save_attachment(&url, &filename).await {
                            Ok(path) => BackendEvent::Notice(format!("Saved to {}", path.display())),
                            Err(e) => BackendEvent::Error(format!("failed to save {filename}: {e}")),
                        };
                        let _ = event_tx.send(event);
                    });
                }
                UiCommand::StartTyping { channel_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.trigger_typing(&channel_id).await {
                            tracing::warn!("failed to send typing indicator: {e}");
                        }
                    }
                }
                UiCommand::LoginWithToken { token } => {
                    start_session(token, &mut rest, &event_tx).await;
                }
                UiCommand::OpenDmWithUser { user_id } => {
                    if let Some(client) = &rest {
                        match client.create_dm(&user_id).await {
                            Ok(channel) => {
                                let _ = event_tx.send(BackendEvent::DmOpened(channel));
                            }
                            Err(e) => {
                                tracing::error!("failed to open dm with {user_id}: {e}");
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to open DM: {e}")));
                            }
                        }
                    }
                }
                UiCommand::RefreshDmChannels => {
                    if let Some(client) = &rest {
                        if let Ok(dms) = client.dm_channels().await {
                            let _ = event_tx.send(BackendEvent::DmChannelsLoaded(dms));
                        }
                    }
                }
                UiCommand::EditMessage { channel_id, message_id, content } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.edit_message(&channel_id, &message_id, &content).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to edit message: {e}")));
                        }
                    }
                }
                UiCommand::DeleteMessage { channel_id, message_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.delete_message(&channel_id, &message_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to delete message: {e}")));
                        }
                    }
                }
                UiCommand::ToggleReaction { channel_id, message_id, emoji, add } => {
                    if let Some(client) = &rest {
                        let result = if add {
                            client.add_reaction(&channel_id, &message_id, &emoji).await
                        } else {
                            client.remove_reaction(&channel_id, &message_id, &emoji).await
                        };
                        if let Err(e) = result {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to update reaction: {e}")));
                        }
                    }
                }
                UiCommand::AckMessage { channel_id, message_id } => {
                    if let Some(client) = &rest {
                        let _ = client.ack_message(&channel_id, &message_id).await;
                    }
                }
                UiCommand::JoinGuildByInvite { invite_code } => {
                    if let Some(client) = &rest {
                        match client.join_guild_by_invite(&invite_code).await {
                            Ok(invite) => {
                                if let Some(guild) = invite.guild {
                                    let _ = event_tx.send(BackendEvent::GuildJoined(guild.to_guild()));
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to join server: {e}")));
                            }
                        }
                    }
                }
                UiCommand::LoadPins { channel_id } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let pins = client
                                .pins(&channel_id)
                                .await
                                .inspect_err(|e| tracing::warn!("failed to load pins for {channel_id}: {e}"))
                                .ok();
                            let _ = event_tx.send(BackendEvent::PinsLoaded { channel_id, pins });
                        });
                    }
                }
                UiCommand::AddDmRecipients { channel_id, existing, group, user_ids } => {
                    if let Some(client) = &rest {
                        if group {
                            for user_id in &user_ids {
                                if let Err(e) = client.add_recipient(&channel_id, user_id).await {
                                    let _ = event_tx.send(BackendEvent::Error(format!("could not add to group: {e}")));
                                }
                            }
                            match client.dm_channels().await {
                                Ok(channels) => {
                                    let _ = event_tx.send(BackendEvent::DmChannelsLoaded(channels));
                                }
                                Err(e) => tracing::warn!("failed to refresh DM channels: {e}"),
                            }
                        } else {
                            let recipients: Vec<String> = existing.into_iter().chain(user_ids).collect();
                            match client.create_group_dm(&recipients).await {
                                Ok(channel) => {
                                    let _ = event_tx.send(BackendEvent::DmOpened(channel));
                                }
                                Err(e) => {
                                    let _ = event_tx.send(BackendEvent::Error(format!("could not create group DM: {e}")));
                                }
                            }
                        }
                    }
                }
                UiCommand::UpdateRelationship { user_id, action } => {
                    if let Some(client) = &rest {
                        let result = match action {
                            RelationshipAction::SendRequest => client.put_relationship(&user_id, None).await,
                            RelationshipAction::Block => client.put_relationship(&user_id, Some(2)).await,
                            RelationshipAction::Remove | RelationshipAction::Unblock => client.delete_relationship(&user_id).await,
                        };
                        if let Err(e) = result {
                            let _ = event_tx.send(BackendEvent::Error(format!("could not update relationship: {e}")));
                        }
                        match client.relationships().await {
                            Ok(relationships) => {
                                let _ = event_tx.send(BackendEvent::RelationshipsLoaded(relationships));
                            }
                            Err(e) => tracing::warn!("failed to refresh relationships: {e}"),
                        }
                        let _ = event_tx.send(BackendEvent::RelationshipSettled { user_id });
                    }
                }
                UiCommand::ResolveInvite { code } => {
                    if let Some(client) = &rest {
                        let client = client.clone();
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let result = client.invite(&code).await.map_err(|e| match e {
                                rustcord_client::error::ClientError::Api { status, .. } if (400..500).contains(&status) && status != 429 => {
                                    InviteFailure::Invalid
                                }
                                e => {
                                    tracing::warn!("could not resolve invite {code}: {e}");
                                    InviteFailure::Unavailable
                                }
                            });
                            let _ = event_tx.send(BackendEvent::InviteResolved { code, result });
                        });
                    }
                }
                UiCommand::CreateGuild { name } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.create_guild(&name).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to create server: {e}")));
                        }
                    }
                }
                UiCommand::RequestGuildMembers { guild_id } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::request_guild_members_frame(&guild_id);
                        let _ = tx.send(frame);
                    }
                }
                UiCommand::KickMember { guild_id, user_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.kick_member(&guild_id, &user_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to kick member: {e}")));
                        } else {
                            let _ = event_tx.send(BackendEvent::MemberRemoved { guild_id, user_id });
                        }
                    }
                }
                UiCommand::BanMember { guild_id, user_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.ban_member(&guild_id, &user_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to ban member: {e}")));
                        } else {
                            let _ = event_tx.send(BackendEvent::MemberRemoved { guild_id, user_id });
                        }
                    }
                }
                UiCommand::TimeoutMember { guild_id, user_id, duration_secs } => {
                    if let Some(client) = &rest {
                        let until = duration_secs.map(|secs| (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339());
                        match client.timeout_member(&guild_id, &user_id, until.as_deref()).await {
                            Ok(member) => {
                                let _ = event_tx.send(BackendEvent::MemberUpdated { guild_id, member });
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to update timeout: {e}")));
                            }
                        }
                    }
                }
                UiCommand::SetMemberRole { guild_id, user_id, role_id, add } => {
                    if let Some(client) = &rest {
                        match client.set_member_role(&guild_id, &user_id, &role_id, add).await {
                            Ok(()) => {
                                let _ = event_tx.send(BackendEvent::MemberRoleChanged { guild_id, user_id, role_id, add });
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to change role: {e}")));
                            }
                        }
                    }
                }
                UiCommand::SaveChannel { guild_id, channel_id, name, topic, voice, parent_id } => {
                    if let Some(client) = &rest {
                        let result = match &channel_id {
                            Some(channel_id) => client.edit_channel(channel_id, &name, topic.as_deref()).await,
                            None => {
                                let kind = if voice { rustcord_client::models::ChannelType::GuildVoice } else { rustcord_client::models::ChannelType::GuildText };
                                client.create_channel(&guild_id, &name, kind, parent_id.as_deref()).await
                            }
                        };
                        match result {
                            Ok(channel) => {
                                let _ = event_tx.send(BackendEvent::ChannelSaved(channel));
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to save channel: {e}")));
                            }
                        }
                    }
                }
                UiCommand::CreateInvite { channel_id } => {
                    if let Some(client) = &rest {
                        match client.create_invite(&channel_id, 7 * 24 * 60 * 60, 0).await {
                            Ok(invite) => {
                                let _ = event_tx.send(BackendEvent::InviteCreated { code: invite.code });
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to create invite: {e}")));
                            }
                        }
                    }
                }
                UiCommand::LoadGuildInvites { guild_id } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let result = client.guild_invites(&guild_id).await.map_err(|e| e.to_string());
                            let _ = event_tx.send(BackendEvent::GuildInvitesLoaded { guild_id, result });
                        });
                    }
                }
                UiCommand::RenameGuild { guild_id, name } => {
                    if let Some(client) = &rest {
                        match client.edit_guild(&guild_id, &serde_json::json!({ "name": name })).await {
                            Ok(guild) => {
                                let _ = event_tx.send(BackendEvent::GuildEdited(guild));
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to rename server: {e}")));
                            }
                        }
                    }
                }
                UiCommand::SetGuildIcon { guild_id, path } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let icon = match path {
                                Some(path) => match tokio::task::spawn_blocking(move || encode_guild_icon(&path)).await {
                                    Ok(Ok(png)) => serde_json::Value::String(rustcord_client::rest::png_data_uri(&png)),
                                    Ok(Err(e)) => {
                                        let _ = event_tx.send(BackendEvent::Error(format!("failed to read server icon: {e}")));
                                        return;
                                    }
                                    Err(_) => return,
                                },
                                None => serde_json::Value::Null,
                            };
                            match client.edit_guild(&guild_id, &serde_json::json!({ "icon": icon })).await {
                                Ok(guild) => {
                                    let _ = event_tx.send(BackendEvent::GuildEdited(guild));
                                }
                                Err(e) => {
                                    let _ = event_tx.send(BackendEvent::Error(format!("failed to update server icon: {e}")));
                                }
                            }
                        });
                    }
                }
                UiCommand::DeleteGuild { guild_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.delete_guild(&guild_id).await {
                            let _ = event_tx.send(BackendEvent::Error(format!("failed to delete server: {e}")));
                        }
                    }
                }
                UiCommand::LoadGuildBans { guild_id } => {
                    if let Some(client) = rest.clone() {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let result = client.guild_bans(&guild_id).await.map_err(|e| e.to_string());
                            let _ = event_tx.send(BackendEvent::GuildBansLoaded { guild_id, result });
                        });
                    }
                }
                UiCommand::UnbanMember { guild_id, user_id } => {
                    if let Some(client) = &rest {
                        match client.unban_member(&guild_id, &user_id).await {
                            Ok(()) => {
                                let _ = event_tx.send(BackendEvent::MemberUnbanned { guild_id, user_id });
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to unban member: {e}")));
                            }
                        }
                    }
                }
                UiCommand::RevokeInvite { code } => {
                    if let Some(client) = &rest {
                        match client.delete_invite(&code).await {
                            Ok(()) => {
                                let _ = event_tx.send(BackendEvent::InviteRevoked { code });
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to revoke invite: {e}")));
                            }
                        }
                    }
                }
                UiCommand::UpdateGuildSettings { guild_key, patch } => {
                    if let Some(client) = &rest {
                        match client.update_guild_settings(&guild_key, &patch).await {
                            Ok(settings) => {
                                let _ = event_tx.send(BackendEvent::GuildSettingsUpdated(settings));
                            }
                            Err(e) => {
                                let _ = event_tx.send(BackendEvent::Error(format!("failed to update notification settings: {e}")));
                            }
                        }
                    }
                }
                UiCommand::LoadProfile { user_id } => {
                    if let Some(client) = &rest {
                        match client.profile(&user_id).await {
                            Ok(v) => {
                                let profile = rustcord_client::models::UserProfile::from_value(&v);
                                let user = serde_json::from_value(v["user"].clone()).ok();
                                let _ = event_tx.send(BackendEvent::ProfileLoaded { user_id, profile, user });
                            }
                            Err(e) => tracing::warn!("failed to load profile {user_id}: {e}"),
                        }
                    }
                }
                UiCommand::SetPresence { status, custom_status } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.set_status(&status, custom_status.as_deref()).await {
                            tracing::warn!("failed to persist status: {e}");
                        }
                    }
                    presence::set_status(&status, custom_status.as_deref());
                    presence::publish(&event_tx);
                }
                UiCommand::SetManualActivity { name, details, state } => {
                    let field = |s: String| (!s.is_empty()).then_some(s);
                    let activity = field(name.trim().to_string()).map(|name| {
                        serde_json::json!({ "name": name, "type": 0, "details": field(details), "state": field(state) })
                    });
                    presence::set_manual(activity);
                    presence::publish(&event_tx);
                }
                UiCommand::SetRpcEnabled { conn_id, enabled } => {
                    presence::set_enabled(conn_id, enabled);
                    presence::publish(&event_tx);
                }
                UiCommand::JoinVoiceChannel { guild_id, channel_id, ring } => {
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let saved = crate::settings::load();
                        let frame = rustcord_client::gateway::voice_state_update_frame(
                            guild_id.as_deref(),
                            Some(&channel_id),
                            saved.self_mute || saved.self_deaf,
                            saved.self_deaf,
                            false,
                        );
                        let _ = tx.send(frame);
                    }
                    if guild_id.is_none() && ring {
                        if let Some(client) = rest.clone() {
                            let event_tx = event_tx.clone();
                            let channel_id = channel_id.clone();
                            tokio::spawn(async move {
                                let mut last_err = None;
                                for _ in 0..4 {
                                    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                                    match client.ring_call(&channel_id).await {
                                        Ok(()) => return,
                                        Err(e) => last_err = Some(e),
                                    }
                                }
                                if let Some(e) = last_err {
                                    tracing::warn!("ring call failed: {e}");
                                    let _ = event_tx.send(BackendEvent::Error(format!("failed to ring: {e}")));
                                }
                            });
                        }
                    }
                    let _ = event_tx.send(BackendEvent::VoiceConnecting);
                }
                UiCommand::DeclineCall { channel_id } => {
                    if let Some(client) = &rest {
                        if let Err(e) = client.stop_ringing(&channel_id).await {
                            tracing::warn!("decline call failed: {e}");
                        }
                    }
                }
                UiCommand::LeaveVoiceChannel { guild_id } => {
                    if let Some(state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        state.screen_sharing.store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::voice_state_update_frame(guild_id.as_deref(), None, false, false, false);
                        let _ = tx.send(frame);
                        if let Some(stream_key) = ACTIVE_STREAM_KEY.lock().unwrap().take() {
                            let _ = tx.send(serde_json::json!({ "op": 19, "d": { "stream_key": stream_key } }).to_string());
                        }
                    }
                    *VOICE_MEDIA_STATE.lock().unwrap() = None;
                    let _ = event_tx.send(BackendEvent::VoiceDisconnected);
                }
                UiCommand::SetVoiceMuteDeaf { guild_id, channel_id, self_mute, self_deaf } => {
                    let self_video = VOICE_MEDIA_STATE
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|state| state.camera_enabled.load(std::sync::atomic::Ordering::Relaxed));
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame =
                            rustcord_client::gateway::voice_state_update_frame(guild_id.as_deref(), Some(&channel_id), self_mute, self_deaf, self_video);
                        let _ = tx.send(frame);
                    }
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.muted.store(self_mute, std::sync::atomic::Ordering::Relaxed);
                        media_state.deafened.store(self_deaf, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                #[cfg(feature = "screenshare")]
                UiCommand::LoadScreenAudioSources => {
                    let events = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let (sources, error) = match rustcord_client::screen_audio::list_sources() {
                            Ok(sources) => (sources, None),
                            Err(error) => (Vec::new(), Some(error)),
                        };
                        let _ = events.send(BackendEvent::ScreenAudioSourcesLoaded { sources, error });
                    });
                }
                UiCommand::SetScreenSharing { enabled, include_system_audio, audio_source_pid, max_height, framerate, guild_id, channel_id } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.system_audio_enabled.store(include_system_audio, std::sync::atomic::Ordering::Relaxed);
                        media_state.system_audio_pid.store(audio_source_pid.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                        media_state.screen_share_max_height.store(max_height.clamp(720, 1080), std::sync::atomic::Ordering::Relaxed);
                        media_state.screen_share_fps.store(framerate.clamp(15, 60), std::sync::atomic::Ordering::Relaxed);
                        media_state.screen_sharing.store(enabled, std::sync::atomic::Ordering::Relaxed);
                    }
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        if enabled {
                            let stream_type = if guild_id.is_some() { "guild" } else { "call" };
                            let frame = serde_json::json!({
                                "op": 18,
                                "d": {
                                    "type": stream_type,
                                    "guild_id": guild_id,
                                    "channel_id": channel_id,
                                    "preferred_region": null
                                }
                            });
                            let _ = tx.send(frame.to_string());
                        } else if let Some(stream_key) = ACTIVE_STREAM_KEY.lock().unwrap().take() {
                            let frame = serde_json::json!({ "op": 19, "d": { "stream_key": stream_key } });
                            let _ = tx.send(frame.to_string());
                        }
                    }
                }
                UiCommand::SetCamera { enabled, guild_id, channel_id, self_mute, self_deaf } => {
                    let connected = match VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        Some(media_state) => {
                            media_state.camera_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
                            true
                        }
                        None => false,
                    };
                    if !connected && enabled {
                        let _ = event_tx.send(BackendEvent::CameraStopped { error: Some("join a call before turning on your camera".into()) });
                    } else if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let frame = rustcord_client::gateway::voice_state_update_frame(guild_id.as_deref(), Some(&channel_id), self_mute, self_deaf, enabled);
                        let _ = tx.send(frame);
                    }
                }
                UiCommand::SetCameraDevice { device } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        *media_state.camera_device.lock().unwrap() = device;
                    }
                }
                UiCommand::FetchFont { spec } => {
                    let events = event_tx.clone();
                    tokio::spawn(async move {
                        let client = http_client();
                        let error = crate::fonts::fetch(client, &spec).await.err();
                        let _ = events.send(BackendEvent::FontFetched { spec, error });
                    });
                }
                UiCommand::LoadCameras => {
                    let events = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        #[cfg(feature = "screenshare")]
                        let cameras = rustcord_client::camera_capture::list_cameras().into_iter().map(|c| (c.id, c.name)).collect();
                        #[cfg(not(feature = "screenshare"))]
                        let cameras = Vec::new();
                        let _ = events.send(BackendEvent::CamerasLoaded(cameras));
                    });
                }
                UiCommand::WatchStream { stream_key } => {
                    *WATCH_STREAM_KEY.lock().unwrap() = Some(stream_key.clone());
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let _ = tx.send(serde_json::json!({ "op": 20, "d": { "stream_key": stream_key } }).to_string());
                    }
                }
                UiCommand::StopWatchingStream { stream_key } => {
                    if WATCH_STREAM_KEY.lock().unwrap().as_deref() == Some(stream_key.as_str()) {
                        *WATCH_STREAM_KEY.lock().unwrap() = None;
                    }
                    if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                        let _ = tx.send(serde_json::json!({ "op": 19, "d": { "stream_key": stream_key } }).to_string());
                    }
                }
                UiCommand::SetMicProcessing { gain, denoise } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.mic.set_gain(gain);
                        media_state.mic.denoise.store(denoise, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                UiCommand::SetUserVolume { user_id, volume } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        media_state.user_volumes.lock().unwrap().insert(user_id, volume);
                    }
                }
                UiCommand::SetVoiceInputDevice { device } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        *media_state.input_device.lock().unwrap() = device;
                    }
                }
                UiCommand::SetVoiceOutputDevice { device } => {
                    if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                        *media_state.output_device.lock().unwrap() = device;
                    }
                }
                UiCommand::LoadVoiceDevices => {
                    let event_tx = event_tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let inputs = rustcord_client::voice_audio::list_input_devices();
                        let outputs = rustcord_client::voice_audio::list_output_devices();
                        let _ = event_tx.send(BackendEvent::VoiceDevicesLoaded { inputs, outputs });
                    });
                }
                UiCommand::RunMicTest { input_device, output_device } => {
                    let event_tx = event_tx.clone();
                    let _ = event_tx.send(BackendEvent::MicTestStarted);
                    tokio::task::spawn_blocking(move || {
                        rustcord_client::voice_audio::run_mic_test(input_device, output_device, std::time::Duration::from_secs(10));
                        let _ = event_tx.send(BackendEvent::MicTestFinished);
                    });
                }
                UiCommand::Logout => {
                    stop_gateway().await;
                    match serialized_credential_op(session_store::clear_token).await {
                        Ok(Ok(())) => {},
                        Ok(Err(_)) => tracing::warn!("failed to delete saved credentials"),
                        Err(_) => tracing::warn!("credential deletion task failed"),
                    }
                    rest = None;
                    login_session = None;
                }
            }
        }
    });
}

#[derive(Default)]
struct VoicePending {
    session_id: Option<String>,
    guild_id: Option<String>,
    channel_id: Option<String>,
    token: Option<String>,
    endpoint: Option<String>,
}

#[derive(Default)]
struct StreamPending {
    stream_key: Option<String>,
    rtc_server_id: Option<String>,
    rtc_channel_id: Option<String>,
    token: Option<String>,
    endpoint: Option<String>,
}

fn try_spawn_stream_gateway(
    pending: &mut StreamPending,
    session_id: &Option<String>,
    user_id: &Option<String>,
    stream_task: &mut Option<tokio::task::JoinHandle<()>>,
    forward_tx: &mpsc::UnboundedSender<BackendEvent>,
    publish_stream: bool,
) {
    let (Some(stream_key), Some(rtc_server_id), Some(rtc_channel_id), Some(token), Some(endpoint), Some(session_id), Some(user_id)) = (
        pending.stream_key.clone(),
        pending.rtc_server_id.clone(),
        pending.rtc_channel_id.clone(),
        pending.token.clone(),
        pending.endpoint.clone(),
        session_id.clone(),
        user_id.clone(),
    ) else {
        return;
    };
    let Some(mut media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref().cloned() else { return };
    if publish_stream && !media_state.screen_sharing.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if !publish_stream {
        media_state.screen_sharing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    }
    tracing::info!(publish_stream, "starting screen share RTC gateway");
    if let Some(task) = stream_task.take() {
        task.abort();
    }
    *pending = StreamPending::default();

    let gateway = rustcord_client::voice_gateway::VoiceGateway::new(
        endpoint, rtc_server_id, rtc_channel_id, user_id, session_id, token, media_state, true, publish_stream,
    );
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (_, outgoing_rx) = mpsc::unbounded_channel();
    let events_forward = forward_tx.clone();
    let stream_key_for_events = stream_key.clone();
    tokio::spawn(async move {
        #[cfg(feature = "screenshare")]
        let mut got_video_frame = false;
        let mut got_stats = false;
        while let Some(event) = event_rx.recv().await {
            use rustcord_client::voice_gateway::VoiceGatewayEvent;
            match event {
                VoiceGatewayEvent::Connecting => tracing::info!("screen share RTC websocket connecting"),
                VoiceGatewayEvent::Ready => tracing::info!("screen share RTC signaling ready"),
                VoiceGatewayEvent::MediaConnected => tracing::info!("screen share RTC media connected"),
                VoiceGatewayEvent::ScreenShareStopped => {
                    if publish_stream {
                        if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                            media_state.screen_sharing.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                            let _ = tx.send(serde_json::json!({ "op": 19, "d": { "stream_key": &stream_key_for_events } }).to_string());
                        }
                        let _ = events_forward.send(BackendEvent::ScreenShareStopped);
                    }
                }
                VoiceGatewayEvent::Disconnected { reason } => {
                    tracing::warn!(reason = %reason, "screen share RTC disconnected");
                    if publish_stream {
                        if let Some(media_state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                            media_state.screen_sharing.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        let _ = events_forward.send(BackendEvent::ScreenShareStopped);
                    }
                }
                #[cfg(feature = "screenshare")]
                VoiceGatewayEvent::VideoFrame { user_id, width, height, rgba } => {
                    if !got_video_frame {
                        tracing::info!(width, height, "received first decoded screen share frame");
                        got_video_frame = true;
                    }
                    if let Some(image) = image::RgbaImage::from_raw(width, height, rgba) {
                        let _ = events_forward.send(BackendEvent::RemoteVideoFrame { user_id, rgba: image });
                    }
                }
                VoiceGatewayEvent::Stats(stats) => {
                    if !got_stats {
                        tracing::info!(down_bps = stats.bitrate_down_bps, video_down_bps = stats.video_bitrate_down_bps, dave_active = stats.dave_active, "screen share RTC first stats");
                        got_stats = true;
                    }
                    #[cfg(feature = "screenshare")]
                    let _ = events_forward.send(BackendEvent::ScreenShareStats(stats));
                }
                _ => {}
            }
        }
    });
        let _ = stream_key;
    *stream_task = Some(tokio::spawn(async move {
        if let Err(error) = gateway.run(event_tx, outgoing_rx).await {
            tracing::warn!("screen stream voice gateway exited: {error}");
        }
    }));
}

fn try_spawn_voice_gateway(
    pending_session: &mut Option<VoicePending>,
    my_user_id: &Option<String>,
    forward_tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    let Some(pending) = pending_session.as_ref() else { return };
    let (Some(session_id), Some(guild_id), Some(channel_id), Some(token), Some(endpoint), Some(user_id)) = (
        pending.session_id.clone(),
        pending.guild_id.clone(),
        pending.channel_id.clone(),
        pending.token.clone(),
        pending.endpoint.clone(),
        my_user_id.clone(),
    ) else {
        return;
    };
    *pending_session = None;

    let saved_settings = crate::settings::load();
    let media_state = rustcord_client::voice_gateway::VoiceMediaState {
        input_device: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.input_device)),
        output_device: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.output_device)),
        user_volumes: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.user_volumes)),
        muted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(saved_settings.self_mute || saved_settings.self_deaf)),
        deafened: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(saved_settings.self_deaf)),
        camera_device: std::sync::Arc::new(std::sync::Mutex::new(saved_settings.camera_device)),
        ..Default::default()
    };
    media_state.mic.set_gain(saved_settings.mic_gain.unwrap_or(1.0));
    media_state.mic.denoise.store(saved_settings.denoise, std::sync::atomic::Ordering::Relaxed);
    *VOICE_MEDIA_STATE.lock().unwrap() = Some(media_state.clone());

    let voice_gateway =
        rustcord_client::voice_gateway::VoiceGateway::new(endpoint, guild_id, channel_id, user_id, session_id, token, media_state, false, false);
    let (vg_tx, mut vg_rx) = mpsc::unbounded_channel();
    let (_vg_out_tx, vg_out_rx) = mpsc::unbounded_channel();
    let forward_tx = forward_tx.clone();
    tokio::spawn(async move {
        while let Some(ev) = vg_rx.recv().await {
            use rustcord_client::voice_gateway::VoiceGatewayEvent;
            match ev {
                VoiceGatewayEvent::Connecting => {}
                VoiceGatewayEvent::Ready => {}
                VoiceGatewayEvent::MediaConnected => {
                    let _ = forward_tx.send(BackendEvent::VoiceConnected);
                }
                VoiceGatewayEvent::Disconnected { .. } => {
                    let _ = forward_tx.send(BackendEvent::VoiceDisconnected);
                }
                VoiceGatewayEvent::ScreenShareStopped => {
                    let _ = forward_tx.send(BackendEvent::ScreenShareStopped);
                }
                VoiceGatewayEvent::CameraStopped { error } => {
                    let _ = forward_tx.send(BackendEvent::CameraStopped { error });
                }
                #[cfg(feature = "screenshare")]
                VoiceGatewayEvent::VideoFrame { user_id, width, height, rgba } => {
                    if let Some(image) = image::RgbaImage::from_raw(width, height, rgba) {
                        let _ = forward_tx.send(BackendEvent::RemoteVideoFrame { user_id: crate::app_state::camera_video_key(&user_id), rgba: image });
                    }
                }
                VoiceGatewayEvent::Speaking { user_id: Some(user_id), speaking, .. } => {
                    let _ = forward_tx.send(BackendEvent::VoiceSpeaking { user_id, speaking });
                }
                VoiceGatewayEvent::Stats(stats) => {
                    let _ = forward_tx.send(BackendEvent::VoiceStats(stats));
                }
                _ => {}
            }
        }
    });
    tokio::spawn(async move {
        if let Err(e) = voice_gateway.run(vg_tx, vg_out_rx).await {
            tracing::warn!("voice gateway task exited: {e}");
        }
    });
}

async fn read_attachments(files: Vec<crate::app_state::PendingAttachment>) -> Result<Vec<rustcord_client::rest::UploadFile>, String> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let (bytes, content_type) = match file.source {
            crate::app_state::AttachmentSource::Path(path) => {
                let bytes = tokio::fs::read(&path).await.map_err(|e| format!("failed to read {}: {e}", file.filename))?;
                (bytes, mime_guess_from_path(&path).to_string())
            }
            crate::app_state::AttachmentSource::Bytes { bytes, content_type } => {
                (std::sync::Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()), content_type)
            }
        };
        out.push(rustcord_client::rest::UploadFile { filename: file.filename, bytes, content_type });
    }
    Ok(out)
}

fn encode_guild_icon(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let icon = image::open(path).map_err(|e| e.to_string())?.resize_to_fill(512, 512, image::imageops::FilterType::Lanczos3);
    let mut png = std::io::Cursor::new(Vec::new());
    icon.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}

async fn load_guild_emojis(client: &RestClient, guild_id: String, event_tx: &mpsc::UnboundedSender<BackendEvent>) {
    match client.guild_emojis(&guild_id).await {
        Ok(emojis) => {
            let _ = event_tx.send(BackendEvent::GuildEmojisLoaded { guild_id, emojis });
        }
        Err(e) => tracing::warn!("failed to load emojis for guild {guild_id}: {e}"),
    }
}

async fn run_search(
    client: &RestClient,
    scope: SearchScope,
    query: String,
    filters: SearchFilters,
    offset: u32,
    event_tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    let params = rustcord_client::rest::SearchQuery {
        content: &query,
        author_id: filters.author_id.as_deref(),
        has: filters.has.as_deref(),
        offset,
        ..Default::default()
    };
    let result = match &scope {
        SearchScope::Guild { guild_id } => client.search_guild_messages(guild_id, &params).await,
        SearchScope::Channel { channel_id } => client.search_channel_messages(channel_id, &params).await,
    };
    let event = match result {
        Ok(results) => BackendEvent::SearchResults {
            query,
            filters,
            offset,
            total: results.total,
            messages: results.messages,
        },
        Err(rustcord_client::error::ClientError::NotIndexed { retry_after }) => {
            BackendEvent::SearchIndexing { retry_after }
        }
        Err(e) => {
            tracing::warn!("search failed: {e}");
            BackendEvent::SearchFailed(e.to_string())
        }
    };
    let _ = event_tx.send(event);
}

fn spawn_guild_emoji_loads(client: &RestClient, guild_ids: Vec<String>, event_tx: &mpsc::UnboundedSender<BackendEvent>) {
    const MAX_CONCURRENT: usize = 3;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
    for guild_id in guild_ids {
        let client = client.clone();
        let event_tx = event_tx.clone();
        let semaphore = semaphore.clone();
        tokio::spawn(async move {
            let _permit = semaphore.acquire().await;
            load_guild_emojis(&client, guild_id, &event_tx).await;
        });
    }
}

fn attempt_login<'a>(
    email: String,
    password: String,
    captcha: Option<(String, CaptchaChallenge)>,
    session: &'a LoginSession,
    rest: &'a mut Option<RestClient>,
    event_tx: &'a mpsc::UnboundedSender<BackendEvent>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let captcha_ref = captcha.as_ref().map(|(token, challenge)| (token.as_str(), challenge));
        match session.login_with_captcha(&email, &password, captcha_ref).await {
            Ok(LoginResponse::Success { token }) => {
                start_session(token, rest, event_tx).await;
            }
            Ok(LoginResponse::MfaRequired { ticket, .. }) => {
                let _ = event_tx.send(BackendEvent::MfaRequired { ticket });
            }
            Err(ClientError::CaptchaRequired(challenge)) => {
                let _ = event_tx.send(BackendEvent::CaptchaInProgress);
                let solve_result = tokio::task::spawn_blocking({
                    let challenge = challenge.clone();
                    move || captcha::request_solve(challenge)
                })
                .await;

                match solve_result {
                    Ok(Ok(token)) => {
                        attempt_login(email, password, Some((token, challenge)), session, rest, event_tx).await;
                    }
                    Ok(Err(e)) => {
                        let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                    }
                    Err(e) => {
                        let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
                    }
                }
            }
            Err(e) => {
                tracing::error!("login failed: {e}");
                let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
            }
        }
    })
}

async fn start_session(
    token: String,
    rest: &mut Option<RestClient>,
    event_tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    stop_gateway().await;
    let client = match RestClient::new(&token) {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
            return;
        }
    };

    match client.current_user().await {
        Ok(me) => presence::set_me(serde_json::to_value(me).unwrap_or_default()),
        Err(e) => {
            let _ = event_tx.send(BackendEvent::LoginFailed(e.to_string()));
            return;
        }
    }

    let token_to_save = token.clone();
    match serialized_credential_op(move || session_store::save_token(&token_to_save)).await {
        Ok(Ok(())) => {},
        Ok(Err(_)) => tracing::warn!("failed to save credentials"),
        Err(_) => tracing::warn!("credential save task failed"),
    }
    *rest = Some(client.clone());
    let _ = event_tx.send(BackendEvent::LoggedIn);

    let (gw_tx, mut gw_rx) = mpsc::unbounded_channel();
    let gateway = Gateway::new(token.clone());
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    *GATEWAY_TX.lock().unwrap() = Some(out_tx);
    let forward_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut my_user_id: Option<String> = None;
        let mut my_session_id: Option<String> = None;
        let mut pending_session: Option<VoicePending> = None;
        let mut stream_pending = StreamPending::default();
        let mut stream_task: Option<tokio::task::JoinHandle<()>> = None;
        while let Some(ev) = gw_rx.recv().await {
            match &ev {
                GatewayEvent::Ready { user, .. } => {
                    my_user_id = Some(user.id.clone());
                }
                GatewayEvent::VoiceStateUpdate(vs) if my_user_id.as_deref() == Some(vs.user_id.as_str()) => {
                    if vs.channel_id.is_some() {
                        my_session_id = Some(vs.session_id.clone());
                        let pending = pending_session.get_or_insert_with(VoicePending::default);
                        pending.session_id = Some(vs.session_id.clone());
                        pending.channel_id = vs.channel_id.clone();
                        try_spawn_voice_gateway(&mut pending_session, &my_user_id, &forward_tx);
                    } else {
                        my_session_id = None;
                        pending_session = None;
                        stream_pending = StreamPending::default();
                        if let Some(task) = stream_task.take() {
                            task.abort();
                        }
                        *ACTIVE_STREAM_KEY.lock().unwrap() = None;
                    }
                }
                GatewayEvent::VoiceServerUpdate { token: voice_server_token, guild_id, endpoint } => {
                    let pending = pending_session.get_or_insert_with(VoicePending::default);
                    pending.token = Some(voice_server_token.clone());
                    pending.guild_id = Some(guild_id.clone());
                    pending.endpoint = endpoint.clone();
                    try_spawn_voice_gateway(&mut pending_session, &my_user_id, &forward_tx);
                }
                GatewayEvent::StreamCreate { stream_key, rtc_server_id, rtc_channel_id } => {
                    let is_ours = my_user_id.as_ref().is_some_and(|id| stream_key.rsplit(':').next() == Some(id.as_str()));
                    tracing::info!(is_ours, stream_key_present = !stream_key.is_empty(), rtc_ids_present = !rtc_server_id.is_empty() && !rtc_channel_id.is_empty(), "handling screen share announcement");
                    if is_ours {
                        stream_pending.stream_key = Some(stream_key.clone());
                        stream_pending.rtc_server_id = Some(rtc_server_id.clone());
                        stream_pending.rtc_channel_id = Some(rtc_channel_id.clone());
                        *ACTIVE_STREAM_KEY.lock().unwrap() = Some(stream_key.clone());
                        let sharing = VOICE_MEDIA_STATE.lock().unwrap().as_ref()
                            .is_some_and(|state| state.screen_sharing.load(std::sync::atomic::Ordering::Relaxed));
                        if sharing {
                            try_spawn_stream_gateway(&mut stream_pending, &my_session_id, &my_user_id, &mut stream_task, &forward_tx, true);
                        } else if let Some(tx) = GATEWAY_TX.lock().unwrap().as_ref() {
                            let _ = tx.send(serde_json::json!({ "op": 19, "d": { "stream_key": stream_key } }).to_string());
                        }
                    } else if WATCH_STREAM_KEY.lock().unwrap().as_deref() == Some(stream_key.as_str()) {
                        stream_pending.stream_key = Some(stream_key.clone());
                        stream_pending.rtc_server_id = Some(rtc_server_id.clone());
                        stream_pending.rtc_channel_id = Some(rtc_channel_id.clone());
                        try_spawn_stream_gateway(&mut stream_pending, &my_session_id, &my_user_id, &mut stream_task, &forward_tx, false);
                    }
                }
                GatewayEvent::StreamServerUpdate { stream_key, token, endpoint } => {
                    let matches_stream = stream_pending.stream_key.as_deref().is_none_or(|key| key == stream_key);
                    let is_ours = my_user_id.as_ref().is_some_and(|id| stream_key.rsplit(':').next() == Some(id.as_str()));
                    let is_watched = WATCH_STREAM_KEY.lock().unwrap().as_deref() == Some(stream_key.as_str());
                    tracing::info!(matches_stream, is_ours, is_watched, has_endpoint = endpoint.is_some(), "handling screen share server update");
                    if matches_stream && (is_ours || is_watched) {
                        stream_pending.stream_key = Some(stream_key.clone());
                        stream_pending.token = Some(token.clone());
                        stream_pending.endpoint = endpoint.clone();
                        if endpoint.is_some() {
                            try_spawn_stream_gateway(&mut stream_pending, &my_session_id, &my_user_id, &mut stream_task, &forward_tx, is_ours);
                        } else if let Some(task) = stream_task.take() {
                            task.abort();
                        }
                    }
                }
                GatewayEvent::StreamDelete { stream_key } => {
                    let own_stream = my_user_id.as_ref().is_some_and(|id| stream_key.rsplit(':').next() == Some(id.as_str()));
                    let watched_stream = WATCH_STREAM_KEY.lock().unwrap().as_deref() == Some(stream_key.as_str());
                    if own_stream || stream_pending.stream_key.as_deref() == Some(stream_key.as_str())
                        || ACTIVE_STREAM_KEY.lock().unwrap().as_deref() == Some(stream_key.as_str()) || watched_stream
                    {
                        stream_pending = StreamPending::default();
                        *ACTIVE_STREAM_KEY.lock().unwrap() = None;
                        if let Some(task) = stream_task.take() {
                            task.abort();
                        }
                        if watched_stream {
                            *WATCH_STREAM_KEY.lock().unwrap() = None;
                        }
                        if let Some(state) = VOICE_MEDIA_STATE.lock().unwrap().as_ref() {
                            state.screen_sharing.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                _ => {}
            }
            let _ = forward_tx.send(BackendEvent::Gateway(ev));
        }
    });
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        if let Err(e) = gateway.run_with_cancel(gw_tx, out_rx, cancel_rx).await {
            tracing::warn!("gateway task exited: {e}");
        }
    });
    *ACTIVE_GATEWAY.lock().unwrap_or_else(|p| p.into_inner()) = Some((cancel_tx, task));

    if let Ok(dms) = client.dm_channels().await {
        let _ = event_tx.send(BackendEvent::DmChannelsLoaded(dms));
    }

    if let Ok(relationships) = client.relationships().await {
        let _ = event_tx.send(BackendEvent::RelationshipsLoaded(relationships));
    }
}

#[cfg(feature = "video")]
enum VideoWorkerCommand {
    Next { source: crate::video::SourceState, failed: bool, reply: std::sync::mpsc::SyncSender<Option<crate::images::VideoBatch>> },
    Seek { target: f64, reply: std::sync::mpsc::SyncSender<Option<crate::images::VideoBatch>> },
    Close,
}

#[cfg(feature = "video")]
static VIDEO_WORKERS: std::sync::Mutex<Option<std::collections::HashMap<String, std::sync::mpsc::SyncSender<VideoWorkerCommand>>>> = std::sync::Mutex::new(None);

#[cfg(feature = "video")]
static VIDEO_WORKER_LIMIT: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));

#[cfg(feature = "video")]
const MAX_VIDEO_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(feature = "video")]
const MAX_VIDEO_DOWNLOAD_TIME: std::time::Duration = std::time::Duration::from_secs(180);

#[cfg(feature = "video")]
const PLAYABLE_HEAD_BYTES: u64 = 512 * 1024;

#[cfg(feature = "video")]
#[derive(Clone)]
struct DownloadHandle {
    state: std::sync::Arc<DownloadState>,
}

#[cfg(feature = "video")]
#[derive(Default)]
struct DownloadState {
    written: std::sync::atomic::AtomicU64,
    complete: std::sync::atomic::AtomicBool,
    failed: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "video")]
impl DownloadHandle {
    fn source_state(&self) -> crate::video::SourceState {
        if self.state.complete.load(std::sync::atomic::Ordering::Relaxed) {
            crate::video::SourceState::Complete
        } else {
            crate::video::SourceState::Growing
        }
    }

    fn failed(&self) -> bool {
        self.state.failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn written(&self) -> u64 {
        self.state.written.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(feature = "video")]
static DOWNLOADS: std::sync::Mutex<Option<std::collections::HashMap<String, DownloadHandle>>> = std::sync::Mutex::new(None);

#[cfg(feature = "video")]
async fn start_video_download(url: &str) -> Result<(std::path::PathBuf, DownloadHandle), String> {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncWriteExt;

    let final_path = cache_file(url).ok_or("no cache directory")?.with_extension("video");
    if final_path.exists() {
        let state = std::sync::Arc::new(DownloadState::default());
        state.complete.store(true, Ordering::Relaxed);
        let handle = DownloadHandle { state };
        DOWNLOADS.lock().unwrap().get_or_insert_with(Default::default).insert(url.to_string(), handle.clone());
        return Ok((final_path, handle));
    }
    if let Some(dir) = final_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let permit = MEDIA_DOWNLOADS.clone().acquire_owned().await.map_err(|_| "download service unavailable".to_string())?;
    let client = http_client();
    let mut resp = client.get(url).timeout(MAX_VIDEO_DOWNLOAD_TIME).send().await.and_then(|r| r.error_for_status()).map_err(safe_reqwest_error)?;
    if resp.content_length().is_some_and(|len| len > MAX_VIDEO_BYTES) {
        return Err(format!("video larger than {MAX_VIDEO_BYTES} bytes"));
    }

    let partial = final_path.with_extension("video.part");
    let mut file = tokio::fs::File::create(&partial).await.map_err(|e| e.to_string())?;
    let state = std::sync::Arc::new(DownloadState::default());
    let handle = DownloadHandle { state: state.clone() };
    DOWNLOADS.lock().unwrap().get_or_insert_with(Default::default).insert(url.to_string(), handle.clone());

    let mut written = 0u64;
    while written < PLAYABLE_HEAD_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                written += chunk.len() as u64;
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                state.written.store(written, Ordering::Relaxed);
            }
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;

    let rest_partial = partial.clone();
    let rest_final = final_path.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let result = async {
            while let Some(chunk) = resp.chunk().await.map_err(safe_reqwest_error)? {
                written += chunk.len() as u64;
                if written > MAX_VIDEO_BYTES {
                    return Err(format!("video larger than {MAX_VIDEO_BYTES} bytes"));
                }
                file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                file.flush().await.map_err(|e| e.to_string())?;
                state.written.store(written, Ordering::Relaxed);
            }
            file.flush().await.map_err(|e| e.to_string())?;
            drop(file);
            tokio::fs::rename(&rest_partial, &rest_final).await.map_err(|e| e.to_string())
        }
        .await;
        match result {
            Ok(()) => state.complete.store(true, Ordering::Relaxed),
            Err(e) => {
                tracing::warn!("video download failed: {e}");
                state.failed.store(true, Ordering::Relaxed);
                state.complete.store(true, Ordering::Relaxed);
                let _ = tokio::fs::remove_file(&rest_partial).await;
            }
        }
    });

    Ok((partial, handle))
}

#[cfg(feature = "video")]
async fn poster_source(url: &str) -> Option<std::path::PathBuf> {
    use tokio::io::AsyncWriteExt;
    let _permit = MEDIA_DOWNLOADS.acquire().await.ok()?;
    let final_path = cache_file(url)?.with_extension("video");
    if final_path.exists() {
        return Some(final_path);
    }
    let poster_path = final_path.with_extension("poster");
    if poster_path.exists() {
        return Some(poster_path);
    }
    if let Some(dir) = poster_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let client = http_client();
    let mut resp = client.get(url).send().await.and_then(|r| r.error_for_status()).ok()?;
    let partial = poster_path.with_extension("poster.part");
    let mut file = tokio::fs::File::create(&partial).await.ok()?;
    let mut written = 0u64;
    while written < PLAYABLE_HEAD_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let take = (PLAYABLE_HEAD_BYTES - written).min(chunk.len() as u64) as usize;
                file.write_all(&chunk[..take]).await.ok()?;
                written += take as u64;
                if take < chunk.len() { break; }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    file.flush().await.ok()?;
    drop(file);
    tokio::fs::rename(&partial, &poster_path).await.ok()?;
    Some(poster_path)
}

#[cfg(feature = "video")]
fn download_handle(url: &str) -> Option<DownloadHandle> {
    DOWNLOADS.lock().unwrap().as_ref()?.get(url).cloned()
}

#[cfg(feature = "video")]
fn forget_download(url: &str) {
    if let Some(map) = DOWNLOADS.lock().unwrap().as_mut() {
        map.remove(url);
    }
}

#[cfg(feature = "video")]
fn open_video(url: &str, path: &std::path::Path, handle: &DownloadHandle) -> Option<crate::images::VideoBatch> {
    if let Some(previous) = VIDEO_WORKERS.lock().unwrap().as_mut().and_then(|workers| workers.remove(url)) {
        let _ = previous.send(VideoWorkerCommand::Close);
    }
    let permit = VIDEO_WORKER_LIMIT.clone().blocking_acquire_owned().ok()?;
    let url = url.to_string();
    let path = path.to_path_buf();
    let handle = handle.clone();
    let (command_tx, command_rx) = std::sync::mpsc::sync_channel(8);
    let (initial_tx, initial_rx) = std::sync::mpsc::sync_channel(1);
    let worker_url = url.clone();
    std::thread::Builder::new().name("rustcord-video-decoder".into()).spawn(move || {
        let url = worker_url;
        let _permit = permit;
        let mut path = path;
        let mut decoder = match crate::video::VideoDecoder::open(&path) {
            Ok(d) => d,
            Err(initial_error) if handle.source_state() == crate::video::SourceState::Growing => {
                tracing::debug!("video not playable yet ({initial_error}); waiting for download");
                while handle.source_state() == crate::video::SourceState::Growing {
                    if matches!(command_rx.recv_timeout(std::time::Duration::from_millis(100)), Ok(VideoWorkerCommand::Close)) {
                        let _ = initial_tx.send(None);
                        return;
                    }
                }
                if handle.failed() {
                    forget_download(&url);
                    let _ = initial_tx.send(None);
                    return;
                }
                let final_path = path.with_extension("");
                if final_path.exists() { path = final_path; }
                match crate::video::VideoDecoder::open(&path) {
                    Ok(d) => d,
                    Err(_) => {
                        forget_download(&url);
                        let _ = initial_tx.send(None);
                        return;
                    }
                }
            }
            Err(_) => {
                forget_download(&url);
                let _ = initial_tx.send(None);
                return;
            }
        };
        let batch = decoder.next_batch(crate::video::BATCH_FRAMES, handle.source_state());
        let meta = crate::images::VideoMeta {
            duration_secs: decoder.duration_secs,
            has_audio: decoder.has_audio(),
            width: decoder.width,
            height: decoder.height,
        };
        let keep = !batch.finished;
        if initial_tx.send(Some((crate::images::VideoBatch { url: url.clone(), batch, meta: Some(meta), seek_generation: 0 }, keep))).is_err() || !keep {
            return;
        }
        let mut seek_generation = 0;
        while let Ok(command) = command_rx.recv() {
            match command {
                VideoWorkerCommand::Next { source, failed, reply } => {
                    let mut batch = decoder.next_batch(crate::video::BATCH_FRAMES, source);
                    if batch.awaiting_data && failed {
                        batch.awaiting_data = false;
                        batch.finished = true;
                    }
                    let reply_batch = crate::images::VideoBatch { url: url.clone(), batch, meta: None, seek_generation };
                    let finished = reply_batch.batch.finished;
                    let _ = reply.send(Some(reply_batch));
                    if finished { break; }
                }
                VideoWorkerCommand::Seek { target, reply } => {
                    if let Err(e) = decoder.seek(target) { tracing::warn!("video seek failed: {e}"); }
                    seek_generation += 1;
                    let batch = decoder.next_batch(crate::video::BATCH_FRAMES, crate::video::SourceState::Complete);
                    let _ = reply.send(Some(crate::images::VideoBatch { url: url.clone(), batch, meta: None, seek_generation }));
                }
                VideoWorkerCommand::Close => break,
            }
        }
    }).ok()?;
    VIDEO_WORKERS.lock().unwrap().get_or_insert_with(Default::default).insert(url.clone(), command_tx);
    let result = match initial_rx.recv() {
        Ok(result) => result,
        Err(_) => {
            VIDEO_WORKERS.lock().unwrap().as_mut()?.remove(&url);
            return None;
        }
    };
    let Some((batch, keep)) = result else {
        VIDEO_WORKERS.lock().unwrap().as_mut()?.remove(&url);
        return None;
    };
    if !keep {
        VIDEO_WORKERS.lock().unwrap().as_mut()?.remove(&url);
    }
    Some(batch)
}

#[cfg(feature = "video")]
fn next_video_batch(url: &str) -> Option<crate::images::VideoBatch> {
    let handle = download_handle(url);
    let source = handle.as_ref().map(|h| h.source_state()).unwrap_or(crate::video::SourceState::Complete);
    let workers = VIDEO_WORKERS.lock().unwrap();
    let worker = workers.as_ref()?.get(url)?.clone();
    drop(workers);
    let (reply, response) = std::sync::mpsc::sync_channel(1);
    worker.send(VideoWorkerCommand::Next { source, failed: handle.is_some_and(|h| h.failed()), reply }).ok()?;
    let batch = response.recv().ok()??;
    if batch.batch.finished { VIDEO_WORKERS.lock().unwrap().as_mut()?.remove(url); }
    Some(batch)
}

#[cfg(feature = "video")]
fn seek_video(url: &str, target_secs: f64) -> Option<crate::images::VideoBatch> {
    let worker = VIDEO_WORKERS.lock().unwrap().as_ref()?.get(url)?.clone();
    let (reply, response) = std::sync::mpsc::sync_channel(1);
    worker.send(VideoWorkerCommand::Seek { target: target_secs, reply }).ok()?;
    response.recv().ok()?
}

#[cfg(feature = "video")]
fn close_video(url: &str) {
    if let Some(worker) = VIDEO_WORKERS.lock().unwrap().as_mut().and_then(|workers| workers.remove(url)) {
        let _ = worker.send(VideoWorkerCommand::Close);
    }
    forget_download(url);
}
