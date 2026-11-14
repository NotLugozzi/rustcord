use crate::error::ClientError;
use crate::identity::ClientIdentity;
use crate::models::{Channel, ChannelType, Emoji, Ban, Gif, Guild, GuildMember, GuildSettings, Invite, Message, Role, Sticker, StickerPack, User};
use reqwest::{header, Client, Method, Response};
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://discord.com/api/v10";

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LoginResponse {
    Success { token: String },
    MfaRequired { ticket: String, mfa: bool },
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptchaChallenge {
    pub captcha_sitekey: String,
    pub captcha_service: String,
    pub captcha_rqdata: Option<String>,
    pub captcha_rqtoken: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CaptchaErrorBody {
    captcha_key: Option<Vec<String>>,
    captcha_sitekey: Option<String>,
    captcha_service: Option<String>,
    captcha_rqdata: Option<String>,
    captcha_rqtoken: Option<String>,
}

pub struct LoginSession {
    http: Client,
}

impl LoginSession {
    pub fn new() -> Result<Self, ClientError> {
        let http = Client::builder()
            .default_headers(login_headers())
            .timeout(std::time::Duration::from_secs(15))
            .cookie_store(true)
            .build()?;
        Ok(Self { http })
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<LoginResponse, ClientError> {
        self.login_with_captcha(email, password, None).await
    }

    pub async fn login_with_captcha(
        &self,
        email: &str,
        password: &str,
        captcha: Option<(&str, &CaptchaChallenge)>,
    ) -> Result<LoginResponse, ClientError> {
        #[derive(Serialize)]
        struct Body<'a> {
            login: &'a str,
            password: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            captcha_key: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            captcha_rqtoken: Option<&'a str>,
        }
        let body = Body {
            login: email,
            password,
            captcha_key: captcha.map(|(token, _)| token),
            captcha_rqtoken: captcha.and_then(|(_, c)| c.captcha_rqtoken.as_deref()),
        };
        let resp = dispatch(self
            .http
            .post(format!("{API_BASE}/auth/login"))
            .json(&body)).await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            if let Ok(challenge) = serde_json::from_str::<CaptchaErrorBody>(&text) {
                if challenge.captcha_key.as_deref() == Some(&["captcha-required".to_string()][..]) {
                    if let (Some(sitekey), Some(service)) = (challenge.captcha_sitekey, challenge.captcha_service) {
                        return Err(ClientError::CaptchaRequired(CaptchaChallenge {
                            captcha_sitekey: sitekey,
                            captcha_service: service,
                            captcha_rqdata: challenge.captcha_rqdata,
                            captcha_rqtoken: challenge.captcha_rqtoken,
                        }));
                    }
                }
            }
            return Err(ClientError::Api { status: status.as_u16(), message: "request rejected by server".into() });
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn submit_mfa(&self, ticket: &str, code: &str) -> Result<String, ClientError> {
        #[derive(Serialize)]
        struct Body<'a> {
            code: &'a str,
            ticket: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {
            token: String,
        }
        let resp = dispatch(self
            .http
            .post(format!("{API_BASE}/auth/mfa/totp"))
            .json(&Body { code, ticket })).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Api { status: status.as_u16(), message: "request rejected by server".into() });
        }
        Ok(resp.json::<Resp>().await?.token)
    }
}

async fn dispatch(builder: reqwest::RequestBuilder) -> Result<Response, reqwest::Error> {
    if !crate::devtools::capturing() {
        return builder.send().await;
    }
    let (client, request) = builder.build_split();
    let request = request?;
    let path = match request.url().query() {
        Some(query) => format!("{}?{query}", request.url().path()),
        None => request.url().path().to_string(),
    };
    let body = request.body().map(|body| body.as_bytes().unwrap_or(b"[streamed multipart body]"));
    let id = crate::devtools::begin_http(request.method().as_str(), &path, body);
    let started = std::time::Instant::now();
    let response = match client.execute(request).await {
        Ok(response) => response,
        Err(error) => {
            crate::devtools::finish_http(id, None, started.elapsed(), Err(error.to_string()));
            return Err(error);
        }
    };
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::devtools::finish_http(id, Some(status.as_u16()), started.elapsed(), Err(error.to_string()));
            return Err(error);
        }
    };
    crate::devtools::finish_http(id, Some(status.as_u16()), started.elapsed(), Ok(&bytes));
    let mut rebuilt = http::Response::new(bytes);
    *rebuilt.status_mut() = status;
    *rebuilt.version_mut() = version;
    *rebuilt.headers_mut() = headers;
    Ok(Response::from(rebuilt))
}

#[derive(Debug, Clone)]
pub struct UploadFile {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub content_type: String,
}

fn attachments_payload(content: &str, filenames: &[&str], reply_to: Option<&str>) -> Result<String, ClientError> {
    let attachments: Vec<serde_json::Value> =
        filenames.iter().enumerate().map(|(id, filename)| serde_json::json!({ "id": id, "filename": filename })).collect();
    let mut payload = serde_json::json!({ "content": content, "attachments": attachments });
    if let Some(message_id) = reply_to {
        payload["message_reference"] = serde_json::json!({ "message_id": message_id });
    }
    Ok(serde_json::to_string(&payload)?)
}

fn forward_payload(source: &crate::models::MessageReference) -> serde_json::Value {
    let mut reference = serde_json::json!({
        "type": crate::models::MESSAGE_REFERENCE_FORWARD,
        "message_id": source.message_id,
        "channel_id": source.channel_id,
    });
    if let Some(guild_id) = &source.guild_id {
        reference["guild_id"] = serde_json::json!(guild_id);
    }
    serde_json::json!({ "message_reference": reference })
}

fn voice_message_payload(duration_secs: f64, waveform: &str, reply_to: Option<&str>) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "content": "",
        "flags": crate::voice_message::FLAG_IS_VOICE_MESSAGE,
        "attachments": [{
            "id": "0",
            "filename": crate::voice_message::FILENAME,
            "duration_secs": (duration_secs * 100.0).round() / 100.0,
            "waveform": waveform,
        }],
    });
    if let Some(message_id) = reply_to {
        payload["message_reference"] = serde_json::json!({ "message_id": message_id });
    }
    payload
}

#[derive(Debug, Default)]
pub struct UploadProgress {
    pub sent: std::sync::atomic::AtomicU64,
    pub total: std::sync::atomic::AtomicU64,
}

impl UploadProgress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(std::sync::atomic::Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.sent.load(std::sync::atomic::Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32
    }
}

#[derive(Clone)]
pub struct RestClient {
    http: Client,
    token: String,
}

impl RestClient {
    pub fn new(token: impl Into<String>) -> Result<Self, ClientError> {
        let token = token.into().trim().to_string();
        let http = Client::builder()
            .default_headers(rest_headers(&token)?)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self { http, token })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub async fn current_user(&self) -> Result<User, ClientError> {
        self.get("/users/@me").await
    }

    pub async fn guilds(&self) -> Result<Vec<Guild>, ClientError> {
        self.get("/users/@me/guilds").await
    }

    pub async fn dm_channels(&self) -> Result<Vec<Channel>, ClientError> {
        self.get("/users/@me/channels").await
    }

    pub async fn relationships(&self) -> Result<Vec<crate::models::Relationship>, ClientError> {
        self.get("/users/@me/relationships").await
    }

    pub async fn create_dm(&self, recipient_id: &str) -> Result<Channel, ClientError> {
        #[derive(Serialize)]
        struct Body<'a> {
            recipient_id: &'a str,
        }
        self.post("/users/@me/channels", &Body { recipient_id }).await
    }

    pub async fn guild_channels(&self, guild_id: &str) -> Result<Vec<Channel>, ClientError> {
        self.get(&format!("/guilds/{guild_id}/channels")).await
    }

    pub async fn guild_roles(&self, guild_id: &str) -> Result<Vec<crate::models::Role>, ClientError> {
        self.get(&format!("/guilds/{guild_id}/roles")).await
    }

    pub async fn my_guild_member(&self, guild_id: &str) -> Result<crate::models::SelfGuildMember, ClientError> {
        self.get(&format!("/users/@me/guilds/{guild_id}/member")).await
    }

    pub async fn join_guild_by_invite(&self, invite_code: &str) -> Result<crate::models::Invite, ClientError> {
        self.post(&format!("/invites/{invite_code}"), &serde_json::json!({})).await
    }

    pub async fn invite(&self, code: &str) -> Result<crate::models::Invite, ClientError> {
        self.get(&format!("/invites/{code}?with_counts=true")).await
    }

    pub async fn create_guild(&self, name: &str) -> Result<Guild, ClientError> {
        self.post("/guilds", &serde_json::json!({ "name": name })).await
    }

    pub async fn search_guild_messages(
        &self,
        guild_id: &str,
        query: &SearchQuery<'_>,
    ) -> Result<SearchResults, ClientError> {
        self.search(&format!("/guilds/{guild_id}/messages/search"), query).await
    }

    pub async fn search_channel_messages(
        &self,
        channel_id: &str,
        query: &SearchQuery<'_>,
    ) -> Result<SearchResults, ClientError> {
        self.search(&format!("/channels/{channel_id}/messages/search"), query).await
    }

    async fn search(&self, path: &str, query: &SearchQuery<'_>) -> Result<SearchResults, ClientError> {
        let url = format!("{API_BASE}{path}?{}", query.to_query_string());
        let resp = self.send_retrying(|| self.http.get(&url)).await?;
        if resp.status() == reqwest::StatusCode::ACCEPTED {
            let body: IndexingBody = resp.json().await.unwrap_or_default();
            return Err(ClientError::NotIndexed { retry_after: body.retry_after });
        }
        let body: SearchBody = resp.json().await?;
        Ok(SearchResults {
            total: body.total_results,
            messages: body.messages.into_iter().flatten().collect(),
        })
    }

    pub async fn guild_emojis(&self, guild_id: &str) -> Result<Vec<Emoji>, ClientError> {
        self.get(&format!("/guilds/{guild_id}/emojis")).await
    }

    pub async fn messages(&self, channel_id: &str, limit: u32) -> Result<Vec<Message>, ClientError> {
        self.get(&format!("/channels/{channel_id}/messages?limit={limit}")).await
    }

    pub async fn messages_before(
        &self,
        channel_id: &str,
        before: &str,
        limit: u32,
    ) -> Result<Vec<Message>, ClientError> {
        self.get(&format!("/channels/{channel_id}/messages?limit={limit}&before={before}")).await
    }

    pub async fn messages_around(
        &self,
        channel_id: &str,
        around: &str,
        limit: u32,
    ) -> Result<Vec<Message>, ClientError> {
        self.get(&format!("/channels/{channel_id}/messages?limit={limit}&around={around}")).await
    }

    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<Message, ClientError> {
        self.send_message_reply(channel_id, content, None).await
    }

    pub async fn send_message_reply(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<Message, ClientError> {
        #[derive(Serialize)]
        struct MessageReference<'a> {
            message_id: &'a str,
        }
        #[derive(Serialize)]
        struct Body<'a> {
            content: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            message_reference: Option<MessageReference<'a>>,
        }
        let body = Body {
            content,
            message_reference: reply_to.map(|id| MessageReference { message_id: id }),
        };
        self.post(&format!("/channels/{channel_id}/messages"), &body).await
    }

    pub async fn send_sticker(&self, channel_id: &str, sticker_id: &str, reply_to: Option<&str>) -> Result<Message, ClientError> {
        #[derive(Serialize)]
        struct MessageReference<'a> {
            message_id: &'a str,
        }
        #[derive(Serialize)]
        struct Body<'a> {
            content: &'a str,
            sticker_ids: [&'a str; 1],
            #[serde(skip_serializing_if = "Option::is_none")]
            message_reference: Option<MessageReference<'a>>,
        }
        let body = Body {
            content: "",
            sticker_ids: [sticker_id],
            message_reference: reply_to.map(|id| MessageReference { message_id: id }),
        };
        self.post(&format!("/channels/{channel_id}/messages"), &body).await
    }

    pub async fn send_poll(&self, channel_id: &str, poll: serde_json::Value) -> Result<Message, ClientError> {
        let body = serde_json::json!({ "content": "", "poll": poll });
        self.post(&format!("/channels/{channel_id}/messages"), &body).await
    }

    pub async fn forward_message(&self, target_channel_id: &str, source: &crate::models::MessageReference) -> Result<Message, ClientError> {
        let body = forward_payload(source);
        self.post(&format!("/channels/{target_channel_id}/messages"), &body).await
    }

    pub async fn send_voice_message(
        &self,
        channel_id: &str,
        ogg: Vec<u8>,
        duration_secs: f64,
        waveform: String,
        reply_to: Option<&str>,
    ) -> Result<Message, ClientError> {
        let payload_json = serde_json::to_string(&voice_message_payload(duration_secs, &waveform, reply_to))?;
        let part = reqwest::multipart::Part::bytes(ogg)
            .file_name(crate::voice_message::FILENAME)
            .mime_str(crate::voice_message::CONTENT_TYPE)
            .map_err(|e| ClientError::Api { status: 0, message: e.to_string() })?;
        let form = reqwest::multipart::Form::new().text("payload_json", payload_json).part("files[0]", part);
        let resp = dispatch(self.http.post(format!("{API_BASE}/channels/{channel_id}/messages")).multipart(form)).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Api { status: status.as_u16(), message: "request rejected by server".into() });
        }
        Ok(resp.json::<Message>().await?)
    }

    pub async fn vote_poll(&self, channel_id: &str, message_id: &str, answer_ids: &[u32]) -> Result<(), ClientError> {
        let url = format!("{API_BASE}/channels/{channel_id}/polls/{message_id}/answers/@me");
        let body = serde_json::json!({ "answer_ids": answer_ids.iter().map(u32::to_string).collect::<Vec<_>>() });
        self.send_empty(|| self.http.put(&url).json(&body)).await
    }

    pub async fn end_poll(&self, channel_id: &str, message_id: &str) -> Result<Message, ClientError> {
        self.post(&format!("/channels/{channel_id}/polls/{message_id}/expire"), &serde_json::json!({})).await
    }

    pub async fn guild_stickers(&self, guild_id: &str) -> Result<Vec<Sticker>, ClientError> {
        self.get(&format!("/guilds/{guild_id}/stickers")).await
    }

    pub async fn sticker_packs(&self) -> Result<Vec<StickerPack>, ClientError> {
        #[derive(Deserialize)]
        struct Packs {
            sticker_packs: Vec<StickerPack>,
        }
        let packs: Packs = self.get("/sticker-packs").await?;
        Ok(packs.sticker_packs)
    }

    pub async fn gifs(&self, query: &str) -> Result<Vec<Gif>, ClientError> {
        self.get(&gif_path(query)).await
    }

    pub async fn edit_message(&self, channel_id: &str, message_id: &str, content: &str) -> Result<Message, ClientError> {
        let body = serde_json::json!({ "content": content });
        self.request(Method::PATCH, &format!("/channels/{channel_id}/messages/{message_id}"), Some(&body)).await
    }

    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.delete(format!("{API_BASE}/channels/{channel_id}/messages/{message_id}"))).await
    }

    pub async fn pins(&self, channel_id: &str) -> Result<Vec<Message>, ClientError> {
        self.get(&format!("/channels/{channel_id}/pins")).await
    }

    pub async fn add_recipient(&self, channel_id: &str, user_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| {
            self.http.put(format!("{API_BASE}/channels/{channel_id}/recipients/{user_id}")).json(&serde_json::json!({}))
        })
        .await
    }

    pub async fn create_group_dm(&self, recipients: &[String]) -> Result<Channel, ClientError> {
        self.post("/users/@me/channels", &serde_json::json!({ "recipients": recipients })).await
    }

    pub async fn put_relationship(&self, user_id: &str, kind: Option<u8>) -> Result<(), ClientError> {
        let body = match kind {
            Some(kind) => serde_json::json!({ "type": kind }),
            None => serde_json::json!({}),
        };
        self.send_empty(|| self.http.put(format!("{API_BASE}/users/@me/relationships/{user_id}")).json(&body)).await
    }

    pub async fn delete_relationship(&self, user_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.delete(format!("{API_BASE}/users/@me/relationships/{user_id}"))).await
    }

    pub async fn kick_member(&self, guild_id: &str, user_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.delete(format!("{API_BASE}/guilds/{guild_id}/members/{user_id}"))).await
    }

    pub async fn ban_member(&self, guild_id: &str, user_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| {
            self.http.put(format!("{API_BASE}/guilds/{guild_id}/bans/{user_id}")).json(&serde_json::json!({}))
        })
        .await
    }

    pub async fn timeout_member(&self, guild_id: &str, user_id: &str, until: Option<&str>) -> Result<GuildMember, ClientError> {
        let body = serde_json::json!({ "communication_disabled_until": until });
        self.request(Method::PATCH, &format!("/guilds/{guild_id}/members/{user_id}"), Some(&body)).await
    }

    pub async fn set_member_role(&self, guild_id: &str, user_id: &str, role_id: &str, add: bool) -> Result<(), ClientError> {
        let url = format!("{API_BASE}/guilds/{guild_id}/members/{user_id}/roles/{role_id}");
        if add {
            self.send_empty(|| self.http.put(&url).json(&serde_json::json!({}))).await
        } else {
            self.send_empty(|| self.http.delete(&url)).await
        }
    }

    pub async fn create_channel(&self, guild_id: &str, name: &str, kind: ChannelType, parent_id: Option<&str>) -> Result<Channel, ClientError> {
        let body = serde_json::json!({ "name": name, "type": kind, "parent_id": parent_id });
        self.post(&format!("/guilds/{guild_id}/channels"), &body).await
    }

    pub async fn edit_channel(&self, channel_id: &str, name: &str, topic: Option<&str>) -> Result<Channel, ClientError> {
        let mut body = serde_json::json!({ "name": name });
        if let Some(topic) = topic {
            body["topic"] = serde_json::Value::String(topic.to_string());
        }
        self.request(Method::PATCH, &format!("/channels/{channel_id}"), Some(&body)).await
    }

    pub async fn create_invite(&self, channel_id: &str, max_age_secs: u32, max_uses: u32) -> Result<Invite, ClientError> {
        let body = serde_json::json!({ "max_age": max_age_secs, "max_uses": max_uses });
        self.post(&format!("/channels/{channel_id}/invites"), &body).await
    }

    pub async fn guild_invites(&self, guild_id: &str) -> Result<Vec<Invite>, ClientError> {
        self.get(&format!("/guilds/{guild_id}/invites")).await
    }

    pub async fn delete_invite(&self, code: &str) -> Result<(), ClientError> {
        let code = encode_path_segment(code);
        self.send_empty(|| self.http.delete(format!("{API_BASE}/invites/{code}"))).await
    }

    pub async fn edit_guild(&self, guild_id: &str, patch: &serde_json::Value) -> Result<Guild, ClientError> {
        self.request(Method::PATCH, &format!("/guilds/{guild_id}"), Some(patch)).await
    }

    pub async fn delete_guild(&self, guild_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.delete(format!("{API_BASE}/guilds/{guild_id}"))).await
    }

    pub async fn guild_bans(&self, guild_id: &str) -> Result<Vec<Ban>, ClientError> {
        const PAGE: usize = 1000;
        let mut bans: Vec<Ban> = Vec::new();
        loop {
            let after = bans.last().map(|b| format!("&after={}", b.user.id)).unwrap_or_default();
            let page: Vec<Ban> = self.get(&format!("/guilds/{guild_id}/bans?limit={PAGE}{after}")).await?;
            let done = page.len() < PAGE;
            bans.extend(page);
            if done {
                return Ok(bans);
            }
        }
    }

    pub async fn unban_member(&self, guild_id: &str, user_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.delete(format!("{API_BASE}/guilds/{guild_id}/bans/{user_id}"))).await
    }

    pub async fn create_role(&self, guild_id: &str) -> Result<Role, ClientError> {
        self.post(&format!("/guilds/{guild_id}/roles"), &serde_json::json!({ "name": "new role" })).await
    }

    pub async fn edit_role(&self, guild_id: &str, role_id: &str, patch: &serde_json::Value) -> Result<Role, ClientError> {
        self.request(Method::PATCH, &format!("/guilds/{guild_id}/roles/{role_id}"), Some(patch)).await
    }

    pub async fn delete_role(&self, guild_id: &str, role_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.delete(format!("{API_BASE}/guilds/{guild_id}/roles/{role_id}"))).await
    }

    pub async fn update_guild_settings(&self, guild_key: &str, patch: &serde_json::Value) -> Result<GuildSettings, ClientError> {
        self.request(Method::PATCH, &format!("/users/@me/guilds/{guild_key}/settings"), Some(patch)).await
    }

    pub async fn ack_message(&self, channel_id: &str, message_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| {
            self.http
                .post(format!("{API_BASE}/channels/{channel_id}/messages/{message_id}/ack"))
                .json(&serde_json::json!({ "token": null }))
        })
        .await
    }

    pub async fn ring_call(&self, channel_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| {
            self.http
                .post(format!("{API_BASE}/channels/{channel_id}/call/ring"))
                .json(&serde_json::json!({ "recipients": null }))
        })
        .await
    }

    pub async fn stop_ringing(&self, channel_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| {
            self.http
                .post(format!("{API_BASE}/channels/{channel_id}/call/stop-ringing"))
                .json(&serde_json::json!({ "recipients": null }))
        })
        .await
    }

    pub async fn add_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> Result<(), ClientError> {
        let url = format!(
            "{API_BASE}/channels/{channel_id}/messages/{message_id}/reactions/{}/@me",
            encode_path_segment(emoji)
        );
        self.send_empty(|| self.http.put(&url).header(header::CONTENT_LENGTH, "0")).await
    }

    pub async fn remove_reaction(&self, channel_id: &str, message_id: &str, emoji: &str) -> Result<(), ClientError> {
        let url = format!(
            "{API_BASE}/channels/{channel_id}/messages/{message_id}/reactions/{}/@me",
            encode_path_segment(emoji)
        );
        self.send_empty(|| self.http.delete(&url)).await
    }

    pub async fn trigger_typing(&self, channel_id: &str) -> Result<(), ClientError> {
        self.send_empty(|| self.http.post(format!("{API_BASE}/channels/{channel_id}/typing"))).await
    }

    pub async fn send_message_with_attachment(
        &self,
        channel_id: &str,
        content: &str,
        filename: &str,
        file_bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<Message, ClientError> {
        self.send_message_with_attachment_reply(channel_id, content, filename, file_bytes, content_type, None)
            .await
    }

    pub async fn send_message_with_attachment_reply(
        &self,
        channel_id: &str,
        content: &str,
        filename: &str,
        file_bytes: Vec<u8>,
        content_type: &str,
        reply_to: Option<&str>,
    ) -> Result<Message, ClientError> {
        self.send_message_with_attachment_progress(channel_id, content, filename, file_bytes, content_type, reply_to, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_with_attachment_progress(
        &self,
        channel_id: &str,
        content: &str,
        filename: &str,
        file_bytes: Vec<u8>,
        content_type: &str,
        reply_to: Option<&str>,
        progress: Option<std::sync::Arc<UploadProgress>>,
    ) -> Result<Message, ClientError> {
        let file = UploadFile { filename: filename.to_string(), bytes: file_bytes, content_type: content_type.to_string() };
        self.send_message_with_files(channel_id, content, vec![file], reply_to, progress).await
    }

    pub async fn send_message_with_files(
        &self,
        channel_id: &str,
        content: &str,
        files: Vec<UploadFile>,
        reply_to: Option<&str>,
        progress: Option<std::sync::Arc<UploadProgress>>,
    ) -> Result<Message, ClientError> {
        let filenames: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();
        let payload_json = attachments_payload(content, &filenames, reply_to)?;
        let mut form = reqwest::multipart::Form::new().text("payload_json", payload_json);
        for (index, file) in files.into_iter().enumerate() {
            let part = match &progress {
                Some(progress) => {
                    const CHUNK: usize = 64 * 1024;
                    let total = file.bytes.len() as u64;
                    let bytes = bytes::Bytes::from(file.bytes);
                    let chunks: Vec<bytes::Bytes> =
                        (0..bytes.len()).step_by(CHUNK).map(|start| bytes.slice(start..(start + CHUNK).min(bytes.len()))).collect();
                    let progress = progress.clone();
                    let stream = futures_util::stream::iter(chunks.into_iter().map(move |chunk| {
                        progress.sent.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        Ok::<_, std::io::Error>(chunk)
                    }));
                    reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), total)
                }
                None => reqwest::multipart::Part::bytes(file.bytes),
            }
            .file_name(file.filename)
            .mime_str(&file.content_type)
            .map_err(|e| ClientError::Api { status: 0, message: e.to_string() })?;
            form = form.part(format!("files[{index}]"), part);
        }

        let resp = dispatch(self
            .http
            .post(format!("{API_BASE}/channels/{channel_id}/messages"))
            .multipart(form)).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Api { status: status.as_u16(), message: "request rejected by server".into() });
        }
        Ok(resp.json::<Message>().await?)
    }

    pub async fn profile(&self, user_id: &str) -> Result<serde_json::Value, ClientError> {
        self.get(&format!("/users/{user_id}/profile?with_mutual_guilds=true&with_mutual_friends=true")).await
    }

    pub async fn set_status(&self, status: &str, custom_status: Option<&str>) -> Result<serde_json::Value, ClientError> {
        let custom = custom_status.filter(|t| !t.is_empty()).map(|t| serde_json::json!({ "text": t }));
        let body = serde_json::json!({ "status": status, "custom_status": custom });
        self.request(Method::PATCH, "/users/@me/settings", Some(&body)).await
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.request(Method::GET, path, None::<&()>).await
    }

    async fn post<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        self.request(Method::POST, path, Some(body)).await
    }

    async fn request<T: serde::de::DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let url = format!("{API_BASE}{path}");
        let resp = self
            .send_retrying(|| {
                let mut req = self.http.request(method.clone(), &url);
                if let Some(b) = body {
                    req = req.json(b);
                }
                req
            })
            .await?;
        Ok(resp.json::<T>().await?)
    }

    async fn send_retrying<F>(&self, build: F) -> Result<Response, ClientError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        for attempt in 1..=MAX_RATE_LIMIT_ATTEMPTS {
            let resp = dispatch(build()).await?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            if status.as_u16() == 429 && attempt < MAX_RATE_LIMIT_ATTEMPTS {
                if let Some(wait) = retry_after(&resp) {
                    if wait <= MAX_RETRY_WAIT {
                        tracing::debug!("rate limited, retrying in {wait:?} (attempt {attempt})");
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    tracing::warn!("rate limited for {wait:?}, longer than we will wait");
                }
            }
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        Err(ClientError::Api { status: 429, message: "rate limited: retries exhausted".into() })
    }

    async fn send_empty<F>(&self, build: F) -> Result<(), ClientError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        self.send_retrying(build).await.map(|_| ())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchQuery<'a> {
    pub content: &'a str,
    pub channel_id: Option<&'a str>,
    pub author_id: Option<&'a str>,
    pub has: Option<&'a str>,
    pub offset: u32,
}

impl SearchQuery<'_> {
    pub fn to_query_string(&self) -> String {
        let mut parts = vec![format!("content={}", encode_path_segment(self.content))];
        for (key, value) in [
            ("channel_id", self.channel_id),
            ("author_id", self.author_id),
            ("has", self.has),
        ] {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                parts.push(format!("{key}={}", encode_path_segment(value)));
            }
        }
        parts.push(format!("limit={SEARCH_PAGE_SIZE}"));
        if self.offset > 0 {
            parts.push(format!("offset={}", self.offset));
        }
        parts.join("&")
    }
}

#[derive(Debug, Clone)]
pub struct SearchResults {
    pub total: u32,
    pub messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    #[serde(default)]
    total_results: u32,
    #[serde(default)]
    messages: Vec<Vec<Message>>,
}

#[derive(Debug, Default, Deserialize)]
struct IndexingBody {
    #[serde(default)]
    retry_after: f64,
}

pub const SEARCH_PAGE_SIZE: u32 = 25;

fn gif_path(query: &str) -> String {
    let query = query.trim();
    let params = "media_format=gif&provider=tenor&locale=en-US";
    if query.is_empty() {
        format!("/gifs/trending-gifs?{params}")
    } else {
        format!("/gifs/search?q={}&{params}", encode_path_segment(query))
    }
}

pub fn png_data_uri(png: &[u8]) -> String {
    use base64::Engine;
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

pub fn encode_path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

const MAX_RATE_LIMIT_ATTEMPTS: u32 = 4;
const MAX_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

fn retry_after(resp: &Response) -> Option<std::time::Duration> {
    let secs: f64 = resp.headers().get("retry-after")?.to_str().ok()?.trim().parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some(std::time::Duration::from_secs_f64(secs))
}

fn login_headers() -> header::HeaderMap {
    ClientIdentity::current().http_headers()
}

fn rest_headers(token: &str) -> Result<header::HeaderMap, ClientError> {
    let mut headers = ClientIdentity::current().http_headers();
    let auth = header::HeaderValue::from_str(token).map_err(|_| ClientError::MalformedToken)?;
    headers.insert(header::AUTHORIZATION, auth);
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_and_rest_send_the_same_client_identity() {
        let login = login_headers();
        let rest = rest_headers("token").unwrap();
        for name in [header::USER_AGENT.as_str(), "x-super-properties"] {
            assert!(login.get(name).is_some(), "{name} missing");
            assert_eq!(login.get(name), rest.get(name), "{name}");
        }
        assert_eq!(rest.get(header::AUTHORIZATION).unwrap(), "token");
        assert!(login.get(header::AUTHORIZATION).is_none());
    }

    #[test]
    fn a_token_with_control_characters_is_rejected_without_panicking() {
        assert!(matches!(rest_headers("bad\ntoken"), Err(ClientError::MalformedToken)));
    }

    #[test]
    fn a_forward_references_the_source_with_type_one() {
        let source = crate::models::MessageReference { kind: 0, message_id: Some("4".into()), channel_id: Some("5".into()), guild_id: None };
        let body = forward_payload(&source);
        assert_eq!(body, serde_json::json!({ "message_reference": { "type": 1, "message_id": "4", "channel_id": "5" } }));
        let source = crate::models::MessageReference { guild_id: Some("6".into()), ..source };
        assert_eq!(forward_payload(&source)["message_reference"]["guild_id"], "6");
    }

    #[test]
    fn a_voice_message_is_flagged_and_carries_its_waveform() {
        let body = voice_message_payload(3.14159, "AAD/", Some("7"));
        assert_eq!(body["flags"], 8192);
        assert_eq!(body["content"], "");
        assert_eq!(body["attachments"][0]["filename"], "voice-message.ogg");
        assert_eq!(body["attachments"][0]["duration_secs"], 3.14);
        assert_eq!(body["attachments"][0]["waveform"], "AAD/");
        assert_eq!(body["message_reference"]["message_id"], "7");
    }

    #[test]
    fn an_empty_gif_query_asks_for_trending() {
        assert_eq!(gif_path("  "), "/gifs/trending-gifs?media_format=gif&provider=tenor&locale=en-US");
        assert_eq!(gif_path("cat & dog"), "/gifs/search?q=cat%20%26%20dog&media_format=gif&provider=tenor&locale=en-US");
    }

    #[test]
    fn unreserved_characters_pass_through() {
        assert_eq!(encode_path_segment("smile:123"), "smile%3A123");
        assert_eq!(encode_path_segment("aZ09-_.~"), "aZ09-_.~");
    }

    #[test]
    fn a_unicode_emoji_is_percent_encoded_per_utf8_byte() {
        assert_eq!(encode_path_segment("\u{1F44D}"), "%F0%9F%91%8D");
        assert_eq!(encode_path_segment("\u{2764}"), "%E2%9D%A4");
    }

    #[test]
    fn every_file_gets_an_attachment_entry_matching_its_part_index() {
        let payload: serde_json::Value = serde_json::from_str(&attachments_payload("hi", &["a.png", "b.png"], Some("9")).unwrap()).unwrap();
        assert_eq!(payload["content"], "hi");
        assert_eq!(
            payload["attachments"],
            serde_json::json!([{ "id": 0, "filename": "a.png" }, { "id": 1, "filename": "b.png" }])
        );
        assert_eq!(payload["message_reference"]["message_id"], "9");
        let bare: serde_json::Value = serde_json::from_str(&attachments_payload("", &["a.png"], None).unwrap()).unwrap();
        assert!(bare.get("message_reference").is_none());
    }

    #[test]
    fn characters_that_would_break_the_path_are_escaped() {
        for (raw, expected) in [("/", "%2F"), ("?", "%3F"), ("#", "%23"), ("%", "%25"), (" ", "%20")] {
            assert_eq!(encode_path_segment(raw), expected, "{raw}");
        }
    }

    #[test]
    fn a_search_query_encodes_spaces_and_skips_empty_filters() {
        let query = SearchQuery { content: "hello world", ..Default::default() };
        assert_eq!(query.to_query_string(), "content=hello%20world&limit=25");
    }

    #[test]
    fn a_search_query_carries_its_filters() {
        let query = SearchQuery {
            content: "deploy",
            channel_id: Some("123"),
            author_id: Some("456"),
            has: Some("image"),
            offset: 50,
        };
        assert_eq!(
            query.to_query_string(),
            "content=deploy&channel_id=123&author_id=456&has=image&limit=25&offset=50",
        );
    }

    #[test]
    fn an_empty_filter_string_is_omitted_rather_than_sent_blank() {
        let query = SearchQuery { content: "x", author_id: Some(""), ..Default::default() };
        assert!(!query.to_query_string().contains("author_id"), "{}", query.to_query_string());
    }

    #[test]
    fn search_results_flatten_the_nested_message_arrays() {
        let raw = serde_json::json!({
            "total_results": 2,
            "messages": [[sample_message("1")], [sample_message("2")]],
        });
        let body: SearchBody = serde_json::from_value(raw).unwrap();
        let flattened: Vec<Message> = body.messages.into_iter().flatten().collect();
        assert_eq!(body.total_results, 2);
        assert_eq!(flattened.len(), 2);
        assert_eq!(flattened[0].id, "1");
        assert_eq!(flattened[1].id, "2");
    }

    #[test]
    fn an_empty_result_set_is_not_an_error() {
        let body: SearchBody =
            serde_json::from_value(serde_json::json!({ "total_results": 0, "messages": [] })).unwrap();
        assert_eq!(body.total_results, 0);
        assert!(body.messages.is_empty());
    }

    fn sample_message(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "channel_id": "c",
            "author": { "id": "u", "username": "u", "discriminator": "0" },
            "content": "hi",
            "timestamp": "2026-11-03T00:00:00+00:00",
            "edited_timestamp": null,
            "hit": true,
        })
    }

    #[test]
    fn a_multi_codepoint_emoji_keeps_every_byte() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let encoded = encode_path_segment(family);
        assert_eq!(encoded.matches('%').count(), family.len(), "{encoded}");
        assert!(encoded.contains("%E2%80%8D"), "zero-width joiner must survive: {encoded}");
    }
}
