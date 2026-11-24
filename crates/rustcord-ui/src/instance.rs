use std::time::Duration;

const MAX_MESSAGE_BYTES: u64 = 4096;
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Show,
    Open(String),
}

impl Message {
    fn encode(&self) -> String {
        match self {
            Self::Show => "show\n".into(),
            Self::Open(link) => format!("open {}\n", link.replace(['\n', '\r'], "")),
        }
    }

    fn decode(line: &str) -> Option<Self> {
        let line = line.trim_end_matches(['\n', '\r']);
        if line == "show" {
            return Some(Self::Show);
        }
        line.strip_prefix("open ").filter(|link| !link.is_empty()).map(|link| Self::Open(link.to_string()))
    }
}

pub enum Claim {
    Primary(Listener),
    Forwarded,
    Unavailable,
}

#[cfg(unix)]
pub struct Listener {
    socket: std::os::unix::net::UnixListener,
}

#[cfg(unix)]
fn socket_path() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::geteuid() };
    let private = |dir: &std::path::Path| std::fs::metadata(dir).is_ok_and(|meta| meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0);
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).filter(|dir| private(dir));
    let dir = match runtime {
        Some(dir) => dir,
        None => {
            let dir = crate::paths::cache_dir()?;
            std::fs::create_dir_all(&dir).ok()?;
            dir
        }
    };
    Some(dir.join("rustcord-instance.sock"))
}

#[cfg(unix)]
fn forward(path: &std::path::Path, message: &Message) -> bool {
    use std::io::{BufRead, BufReader, Write};
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(path) else { return false };
    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));
    let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
    if stream.write_all(message.encode().as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}

#[cfg(unix)]
pub fn claim(message: &Message, _runtime: &tokio::runtime::Handle) -> Claim {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let Some(path) = socket_path() else { return Claim::Unavailable };
    for _ in 0..2 {
        if forward(&path, message) {
            return Claim::Forwarded;
        }
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_socket()) {
            let _ = std::fs::remove_file(&path);
        }
        match std::os::unix::net::UnixListener::bind(&path) {
            Ok(socket) => {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                return Claim::Primary(Listener { socket });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => {
                tracing::warn!("single instance is unavailable: {error}");
                return Claim::Unavailable;
            }
        }
    }
    Claim::Unavailable
}

#[cfg(unix)]
impl Listener {
    pub fn serve(self, _runtime: &tokio::runtime::Handle, on_message: impl Fn(Message) + Send + 'static) {
        use std::io::{BufRead, BufReader, Read, Write};
        let spawned = std::thread::Builder::new().name("rustcord-instance".into()).spawn(move || {
            for stream in self.socket.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
                let mut line = String::new();
                let Ok(reader) = stream.try_clone() else { continue };
                if BufReader::new(reader.take(MAX_MESSAGE_BYTES)).read_line(&mut line).is_err() {
                    continue;
                }
                if let Some(message) = Message::decode(&line) {
                    let _ = stream.write_all(b"ok\n");
                    on_message(message);
                }
            }
        });
        if let Err(error) = spawned {
            tracing::warn!("could not listen for other rustcord launches: {error}");
        }
    }
}

#[cfg(windows)]
pub struct Listener {
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    name: String,
}

#[cfg(windows)]
fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!(r"\\.\pipe\rustcord-instance-{user}")
}

#[cfg(windows)]
pub fn claim(message: &Message, runtime: &tokio::runtime::Handle) -> Claim {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
    let name = pipe_name();
    runtime.block_on(async {
        if let Ok(server) = ServerOptions::new().first_pipe_instance(true).create(&name) {
            return Claim::Primary(Listener { server, name });
        }
        for _ in 0..10 {
            match ClientOptions::new().open(&name) {
                Ok(mut client) => {
                    if client.write_all(message.encode().as_bytes()).await.is_err() {
                        return Claim::Unavailable;
                    }
                    let mut reply = String::new();
                    let read = tokio::time::timeout(REPLY_TIMEOUT, BufReader::new(client).read_line(&mut reply)).await;
                    return if matches!(read, Ok(Ok(_))) && reply.trim() == "ok" { Claim::Forwarded } else { Claim::Unavailable };
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        Claim::Unavailable
    })
}

#[cfg(windows)]
impl Listener {
    pub fn serve(self, runtime: &tokio::runtime::Handle, on_message: impl Fn(Message) + Send + 'static) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::windows::named_pipe::ServerOptions;
        let Listener { mut server, name } = self;
        runtime.spawn(async move {
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                let next = match ServerOptions::new().create(&name) {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::warn!("stopped listening for other rustcord launches: {error}");
                        return;
                    }
                };
                let mut connected = std::mem::replace(&mut server, next);
                let mut line = String::new();
                let read = tokio::time::timeout(REPLY_TIMEOUT, BufReader::new((&mut connected).take(MAX_MESSAGE_BYTES)).read_line(&mut line)).await;
                if matches!(read, Ok(Ok(_))) {
                    if let Some(message) = Message::decode(&line) {
                        let _ = connected.write_all(b"ok\n").await;
                        on_message(message);
                    }
                }
            }
        });
    }
}

#[cfg(not(any(unix, windows)))]
pub struct Listener;

#[cfg(not(any(unix, windows)))]
pub fn claim(_message: &Message, _runtime: &tokio::runtime::Handle) -> Claim {
    Claim::Unavailable
}

#[cfg(not(any(unix, windows)))]
impl Listener {
    pub fn serve(self, _runtime: &tokio::runtime::Handle, _on_message: impl Fn(Message) + Send + 'static) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_reject_garbage() {
        for message in [Message::Show, Message::Open("discord://-/channels/1/2".into())] {
            assert_eq!(Message::decode(&message.encode()), Some(message));
        }
        assert_eq!(Message::decode(&Message::Open("a\nshow".into()).encode()), Some(Message::Open("ashow".into())));
        assert_eq!(Message::decode("open \n"), None);
        assert_eq!(Message::decode("quit\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_launch_hands_its_link_to_the_first() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dir = std::env::temp_dir().join(format!("rustcord-instance-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("instance.sock");
        let socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        Listener { socket }.serve(runtime.handle(), move |message| {
            let _ = tx.send(message);
        });
        let link = Message::Open("discord://-/invite/rust".into());
        assert!(forward(&path, &link));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), link);
        assert!(forward(&path, &Message::Show));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Message::Show);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!forward(&path, &Message::Show));
    }
}
