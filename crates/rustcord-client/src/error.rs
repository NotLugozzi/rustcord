#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("http error: {0}")]
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
}
