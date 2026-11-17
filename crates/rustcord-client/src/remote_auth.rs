use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const GATEWAY: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const KEY_BITS: usize = 2048;
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Code(String),
    Scanned { name: String, avatar_url: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Token(String),
    Expired,
    Cancelled,
    Failed(String),
}

#[derive(Debug, PartialEq)]
enum Reply {
    Send(Value),
    Show(Step),
    Ticket(String),
    Token(String),
    Cancelled,
    Heartbeat(Duration),
    Nothing,
}

struct RemoteAuth {
    key: RsaPrivateKey,
}

impl RemoteAuth {
    fn new() -> Result<Self, String> {
        let mut rng = rsa::rand_core::OsRng;
        RsaPrivateKey::new(&mut rng, KEY_BITS).map(|key| Self { key }).map_err(|e| format!("couldn't make a key to link with: {e}"))
    }

    fn init(&self) -> Result<Value, String> {
        let public = self.key.to_public_key().to_public_key_der().map_err(|e| format!("couldn't encode the key: {e}"))?;
        Ok(json!({ "op": "init", "encoded_public_key": STANDARD.encode(public.as_bytes()) }))
    }

    fn decrypt(&self, encrypted: &Value) -> Result<Vec<u8>, String> {
        let bytes = STANDARD.decode(encrypted.as_str().ok_or("Discord sent nothing to read")?).map_err(|_| "Discord sent something unreadable")?;
        self.key.decrypt(Oaep::new::<Sha256>(), &bytes).map_err(|_| "couldn't decrypt what Discord sent".to_string())
    }

    fn handle(&self, message: &Value) -> Result<Reply, String> {
        Ok(match message["op"].as_str().unwrap_or_default() {
            "hello" => match message["heartbeat_interval"].as_u64() {
                Some(ms) => Reply::Heartbeat(Duration::from_millis(ms.max(1_000))),
                None => Reply::Heartbeat(DEFAULT_HEARTBEAT),
            },
            "nonce_proof" => {
                let nonce = self.decrypt(&message["encrypted_nonce"])?;
                Reply::Send(json!({ "op": "nonce_proof", "proof": URL_SAFE_NO_PAD.encode(Sha256::digest(&nonce)) }))
            }
            "pending_remote_init" => {
                let fingerprint = message["fingerprint"].as_str().ok_or("Discord sent no code")?;
                Reply::Show(Step::Code(format!("https://discord.com/ra/{fingerprint}")))
            }
            "pending_ticket" => {
                let payload = self.decrypt(&message["encrypted_user_payload"])?;
                Reply::Show(scanned(&String::from_utf8_lossy(&payload)))
            }
            "pending_login" => Reply::Ticket(message["ticket"].as_str().ok_or("Discord sent no ticket")?.to_string()),
            "pending_finish" => Reply::Token(self.token(&message["encrypted_token"])?),
            "cancel" => Reply::Cancelled,
            _ => Reply::Nothing,
        })
    }

    fn token(&self, encrypted: &Value) -> Result<String, String> {
        let token = String::from_utf8(self.decrypt(encrypted)?).map_err(|_| "Discord sent a login rustcord can't read".to_string())?;
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            return Err("Discord sent a login rustcord can't read".into());
        }
        Ok(token)
    }
}

fn scanned(payload: &str) -> Step {
    let mut parts = payload.splitn(4, ':');
    let id = parts.next().unwrap_or_default();
    let _discriminator = parts.next();
    let avatar = parts.next().filter(|hash| !hash.is_empty() && *hash != "0");
    let name = parts.next().unwrap_or_default().to_string();
    let avatar_url = avatar.map(|hash| {
        let ext = if hash.starts_with("a_") { "gif" } else { "png" };
        format!("https://cdn.discordapp.com/avatars/{id}/{hash}.{ext}?size=128")
    });
    Step::Scanned { name, avatar_url }
}

async fn exchange_ticket(ticket: &str) -> Result<String, String> {
    let client = reqwest::Client::builder().default_headers(crate::identity::ClientIdentity::current().http_headers()).build().map_err(|e| e.to_string())?;
    let response = client
        .post(format!("{}/users/@me/remote-auth/login", crate::rest::API_BASE))
        .json(&json!({ "ticket": ticket }))
        .send()
        .await
        .map_err(|e| format!("couldn't reach Discord: {e}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if body.get("captcha_key").is_some() {
        return Err("Discord wants a captcha for this login. Log in with your email and password instead.".into());
    }
    if !status.is_success() {
        return Err(format!("Discord refused the login ({status})"));
    }
    body["encrypted_token"].as_str().map(str::to_string).ok_or_else(|| "Discord didn't hand over the login".to_string())
}

pub async fn link(mut step: impl FnMut(Step)) -> Outcome {
    let session = match tokio::task::spawn_blocking(RemoteAuth::new).await {
        Ok(Ok(session)) => session,
        Ok(Err(reason)) => return Outcome::Failed(reason),
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    match run(&session, &mut step).await {
        Ok(outcome) => outcome,
        Err(reason) => Outcome::Failed(reason),
    }
}

async fn run(session: &RemoteAuth, step: &mut impl FnMut(Step)) -> Result<Outcome, String> {
    let mut request = GATEWAY.into_client_request().map_err(|e| e.to_string())?;
    let headers = request.headers_mut();
    headers.insert("Origin", "https://discord.com".parse().expect("static header"));
    if let Ok(agent) = crate::identity::ClientIdentity::current().user_agent().parse() {
        headers.insert("User-Agent", agent);
    }
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.map_err(|e| format!("couldn't reach Discord's login service: {e}"))?;
    let mut heartbeat = tokio::time::interval(DEFAULT_HEARTBEAT);
    heartbeat.tick().await;
    loop {
        let frame = tokio::select! {
            _ = heartbeat.tick() => {
                socket.send(WsMessage::Text(json!({ "op": "heartbeat" }).to_string())).await.map_err(|e| e.to_string())?;
                continue;
            }
            frame = socket.next() => frame,
        };
        let text = match frame {
            Some(Ok(WsMessage::Text(text))) => text,
            Some(Ok(WsMessage::Close(_))) | None => return Ok(Outcome::Expired),
            Some(Ok(_)) => continue,
            Some(Err(e)) => {
                tracing::debug!("remote auth connection dropped: {e}");
                return Ok(Outcome::Expired);
            }
        };
        let message: Value = serde_json::from_str(&text).map_err(|_| "couldn't understand Discord's login service".to_string())?;
        match session.handle(&message)? {
            Reply::Heartbeat(every) => {
                heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
                socket.send(WsMessage::Text(session.init()?.to_string())).await.map_err(|e| e.to_string())?;
            }
            Reply::Send(reply) => socket.send(WsMessage::Text(reply.to_string())).await.map_err(|e| e.to_string())?,
            Reply::Show(shown) => step(shown),
            Reply::Ticket(ticket) => {
                let encrypted = exchange_ticket(&ticket).await?;
                let _ = socket.close(None).await;
                return Ok(Outcome::Token(session.token(&Value::String(encrypted))?));
            }
            Reply::Token(token) => {
                let _ = socket.close(None).await;
                return Ok(Outcome::Token(token));
            }
            Reply::Cancelled => return Ok(Outcome::Cancelled),
            Reply::Nothing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPublicKey;

    fn session() -> RemoteAuth {
        let mut rng = rsa::rand_core::OsRng;
        RemoteAuth { key: RsaPrivateKey::new(&mut rng, 1024).unwrap() }
    }

    fn seal(session: &RemoteAuth, plain: &[u8]) -> Value {
        let mut rng = rsa::rand_core::OsRng;
        let public = RsaPublicKey::from(&session.key);
        Value::String(STANDARD.encode(public.encrypt(&mut rng, Oaep::new::<Sha256>(), plain).unwrap()))
    }

    #[test]
    fn the_handshake_proves_the_nonce_and_shows_the_code() {
        let session = session();
        assert_eq!(session.handle(&json!({ "op": "hello", "heartbeat_interval": 41250 })).unwrap(), Reply::Heartbeat(Duration::from_millis(41250)));
        let init = session.init().unwrap();
        assert_eq!(init["op"], "init");
        assert!(STANDARD.decode(init["encoded_public_key"].as_str().unwrap()).is_ok());
        let Reply::Send(proof) = session.handle(&json!({ "op": "nonce_proof", "encrypted_nonce": seal(&session, b"nonce") })).unwrap() else { panic!("no proof") };
        assert_eq!(proof["proof"], URL_SAFE_NO_PAD.encode(Sha256::digest(b"nonce")));
        assert_eq!(
            session.handle(&json!({ "op": "pending_remote_init", "fingerprint": "abc" })).unwrap(),
            Reply::Show(Step::Code("https://discord.com/ra/abc".into()))
        );
    }

    #[test]
    fn a_scan_names_who_scanned_and_a_confirm_hands_over_a_ticket() {
        let session = session();
        let scanned = session.handle(&json!({ "op": "pending_ticket", "encrypted_user_payload": seal(&session, b"42:0:a_hash:luna") })).unwrap();
        assert_eq!(scanned, Reply::Show(Step::Scanned { name: "luna".into(), avatar_url: Some("https://cdn.discordapp.com/avatars/42/a_hash.gif?size=128".into()) }));
        assert_eq!(session.handle(&json!({ "op": "pending_login", "ticket": "t" })).unwrap(), Reply::Ticket("t".into()));
        assert_eq!(session.handle(&json!({ "op": "cancel" })).unwrap(), Reply::Cancelled);
    }

    #[test]
    fn the_token_decrypts_and_garbage_is_refused() {
        let session = session();
        assert_eq!(session.token(&seal(&session, b"mfa.token-value")).unwrap(), "mfa.token-value");
        assert!(session.token(&seal(&session, b"has space")).is_err());
        assert!(session.token(&json!("not base64!!")).is_err());
        assert_eq!(session.handle(&json!({ "op": "pending_finish", "encrypted_token": seal(&session, b"tok") })).unwrap(), Reply::Token("tok".into()));
    }

    #[test]
    fn a_scan_without_an_avatar_has_none() {
        assert_eq!(scanned("1:0:0:ann"), Step::Scanned { name: "ann".into(), avatar_url: None });
    }
}
