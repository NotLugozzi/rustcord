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
