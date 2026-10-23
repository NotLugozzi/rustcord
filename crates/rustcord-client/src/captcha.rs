use crate::error::ClientError;
use crate::rest::CaptchaChallenge;
use std::sync::mpsc;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

pub fn solve_blocking(challenge: &CaptchaChallenge) -> Result<String, ClientError> {
    if challenge.captcha_service != "hcaptcha" {
        return Err(ClientError::CaptchaFailed(format!(
            "unsupported captcha service: {}",
            challenge.captcha_service
        )));
    }

    let (tx, rx) = mpsc::channel::<String>();
    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Verify you are human")
        .with_inner_size(tao::dpi::LogicalSize::new(420.0, 520.0))
        .build(&event_loop)
        .map_err(|e| ClientError::CaptchaFailed(e.to_string()))?;

    let rqdata_attr = challenge
        .captcha_rqdata
        .as_deref()
        .map(|d| format!(r#"data-rqdata="{d}""#))
        .unwrap_or_default();

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head><style>body {{ margin: 0; display: flex; align-items: center; justify-content: center; height: 100vh; }}</style></head>
<body>
<div class="h-captcha" data-sitekey="{sitekey}" data-callback="onSolved" {rqdata_attr}></div>
<script src="https://js.hcaptcha.com/1/api.js" async defer></script>
<script>
function onSolved(token) {{
    window.ipc.postMessage(token);
}}
</script>
</body>
</html>"#,
        sitekey = challenge.captcha_sitekey,
    );

    let _webview = WebViewBuilder::new()
        .with_html(html)
        .with_ipc_handler(move |req| {
            let _ = tx.send(req.body().clone());
        })
        .build(&window)
        .map_err(|e| ClientError::CaptchaFailed(e.to_string()))?;

    let mut solved: Option<String> = None;
    event_loop.run_return(|event, _, control_flow| {
        if let Ok(token) = rx.try_recv() {
            solved = Some(token);
            *control_flow = ControlFlow::Exit;
            return;
        }
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {
                *control_flow =
                    ControlFlow::WaitUntil(std::time::Instant::now() + std::time::Duration::from_millis(50));
            }
        }
    });

    solved.ok_or_else(|| ClientError::CaptchaFailed("window closed before captcha solved".into()))
}
