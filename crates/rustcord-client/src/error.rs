#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("http request failed")]
    Http(#[from] reqwest::Error),
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("gateway closed: {0}")]
    GatewayClosed(String),
    #[error("not authenticated")]
    Unauthenticated,
    #[error("discord api error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("captcha required")]
    CaptchaRequired(crate::rest::CaptchaChallenge),
    #[error("captcha solve failed: {0}")]
    CaptchaFailed(String),
    #[error("malformed token")]
    MalformedToken,
    #[error("search index not ready, retry in {retry_after}s")]
    NotIndexed { retry_after: f64 },
    #[error("screen capture failed: {0}")]
    ScreenCapture(String),
}

impl ClientError {
    pub fn mfa_ticket(&self) -> Option<String> {
        let ClientError::Api { message, .. } = self else { return None };
        let body: serde_json::Value = serde_json::from_str(message).ok()?;
        if body.get("code")?.as_u64()? != 60003 {
            return None;
        }
        Some(body.get("mfa")?.get("ticket")?.as_str()?.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::ClientError;

    #[test]
    fn mfa_ticket_is_read_only_from_mfa_required_errors() {
        let required = ClientError::Api {
            status: 401,
            message: r#"{"message":"Two factor is required for this operation","code":60003,"mfa":{"ticket":"abc","methods":[{"type":"totp"}]}}"#.into(),
        };
        let other = ClientError::Api { status: 403, message: r#"{"message":"Missing Permissions","code":50013}"#.into() };
        assert_eq!(required.mfa_ticket().as_deref(), Some("abc"));
        assert_eq!(other.mfa_ticket(), None);
    }
}
