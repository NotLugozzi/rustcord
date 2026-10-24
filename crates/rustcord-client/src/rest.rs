use crate::error::ClientError;
use crate::models::{Channel, Emoji, Guild, Message, User};
use reqwest::{header, Client, Method};
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://discord.com/api/v10";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

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
        let mut headers = header::HeaderMap::new();
        headers.insert(header::USER_AGENT, header::HeaderValue::from_static(USER_AGENT));
        headers.insert("X-Super-Properties", header::HeaderValue::from_static(SUPER_PROPERTIES_B64));
        let http = Client::builder()
            .default_headers(headers)
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
        let resp = self
            .http
            .post(format!("{API_BASE}/auth/login"))
            .json(&body)
            .send()
            .await?;
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
            return Err(ClientError::Api { status: status.as_u16(), message: text });
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
        let resp = self
            .http
            .post(format!("{API_BASE}/auth/mfa/totp"))
            .json(&Body { code, ticket })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        Ok(resp.json::<Resp>().await?.token)
    }
}

#[derive(Clone)]
pub struct RestClient {
    http: Client,
    token: String,
}

impl RestClient {
    pub fn new(token: impl Into<String>) -> Result<Self, ClientError> {
        let token = token.into();
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, header::HeaderValue::from_str(&token).unwrap());
        headers.insert(header::USER_AGENT, header::HeaderValue::from_static(USER_AGENT));
        headers.insert("X-Super-Properties", header::HeaderValue::from_static(SUPER_PROPERTIES_B64));

        let http = Client::builder()
            .default_headers(headers)
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

    pub async fn edit_message(&self, channel_id: &str, message_id: &str, content: &str) -> Result<Message, ClientError> {
        let body = serde_json::json!({ "content": content });
        self.request(Method::PATCH, &format!("/channels/{channel_id}/messages/{message_id}"), Some(&body)).await
    }

    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<(), ClientError> {
        let resp = self.http.delete(format!("{API_BASE}/channels/{channel_id}/messages/{message_id}")).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        Ok(())
    }

    pub async fn trigger_typing(&self, channel_id: &str) -> Result<(), ClientError> {
        let resp = self
            .http
            .post(format!("{API_BASE}/channels/{channel_id}/typing"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        Ok(())
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
        #[derive(Serialize)]
        struct PayloadAttachment<'a> {
            id: u32,
            filename: &'a str,
        }
        #[derive(Serialize)]
        struct MessageReference<'a> {
            message_id: &'a str,
        }
        #[derive(Serialize)]
        struct Payload<'a> {
            content: &'a str,
            attachments: Vec<PayloadAttachment<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            message_reference: Option<MessageReference<'a>>,
        }
        let payload = Payload {
            content,
            attachments: vec![PayloadAttachment { id: 0, filename }],
            message_reference: reply_to.map(|id| MessageReference { message_id: id }),
        };
        let payload_json = serde_json::to_string(&payload)?;

        let file_part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename.to_string())
            .mime_str(content_type)
            .map_err(|e| ClientError::Api { status: 0, message: e.to_string() })?;
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload_json)
            .part("files[0]", file_part);

        let resp = self
            .http
            .post(format!("{API_BASE}/channels/{channel_id}/messages"))
            .multipart(form)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
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
        let mut req = self.http.request(method, &url);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status: status.as_u16(), message });
        }
        Ok(resp.json::<T>().await?)
    }
}

const SUPER_PROPERTIES_B64: &str = "eyJvcyI6IldpbmRvd3MiLCJicm93c2VyIjoiQ2hyb21lIiwiZGV2aWNlIjoiIiwic3lzdGVtX2xvY2FsZSI6ImVuLVVTIiwiYnJvd3Nlcl91c2VyX2FnZW50IjoiTW96aWxsYS81LjAgKFdpbmRvd3MgTlQgMTAuMDsgV2luNjQ7IHg2NCkgQXBwbGVXZWJLaXQvNTM3LjM2IChLSFRNTCwgbGlrZSBHZWNrbykgQ2hyb21lLzEyOS4wLjAuMCBTYWZhcmkvNTM3LjM2IiwiYnJvd3Nlcl92ZXJzaW9uIjoiMTI5LjAuMC4wIiwib3NfdmVyc2lvbiI6IjEwIiwicmVmZXJyZXIiOiIiLCJyZWZlcnJpbmdfZG9tYWluIjoiIiwicmVmZXJyZXJfY3VycmVudCI6IiIsInJlZmVycmluZ19kb21haW5fY3VycmVudCI6IiIsInJlbGVhc2VfY2hhbm5lbCI6InN0YWJsZSIsImNsaWVudF9idWlsZF9udW1iZXIiOjMwMDAwMCwiY2xpZW50X2V2ZW50X3NvdXJjZSI6bnVsbH0=";
