use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::device::server_args;

pub type ScreenshotTokens = Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>;

pub fn new_tokens() -> ScreenshotTokens {
    Arc::new(Mutex::new(std::collections::HashMap::new()))
}

pub async fn start_screenshot_loop(
    tokens: ScreenshotTokens,
    serial: String,
    host: String,
    port: u16,
    fps: u32,
    app: AppHandle,
) {
    let token = CancellationToken::new();
    {
        let mut map = tokens.lock().await;
        if let Some(old) = map.insert(serial.clone(), token.clone()) {
            old.cancel();
        }
    }

    let interval =
        std::time::Duration::from_millis(if fps == 0 { 1000 } else { 1000 / fps as u64 });

    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            _ = tokio::time::sleep(interval) => {
                let data = capture_screenshot(&serial, &host, port).await;
                if let Some(b64) = data {
                    let payload = serde_json::json!({
                        "serial": serial,
                        "data": format!("data:image/jpeg;base64,{}", b64)
                    });
                    let _ = app.emit("screenshot", payload);
                }
            }
        }
    }
}

pub async fn stop_screenshot_loop(tokens: ScreenshotTokens, serial: &str) {
    let mut map = tokens.lock().await;
    if let Some(token) = map.remove(serial) {
        token.cancel();
    }
}

async fn capture_screenshot(serial: &str, host: &str, port: u16) -> Option<String> {
    let mut args = server_args(host, port);
    args.extend([
        "-s".into(),
        serial.into(),
        "exec-out".into(),
        "screencap".into(),
        "-p".into(),
    ]);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("adb").args(&args).output(),
    )
    .await;

    let png_data = match result {
        Ok(Ok(out)) if out.status.success() && !out.stdout.is_empty() => out.stdout,
        _ => return None,
    };

    // Decode PNG, scale down, re-encode as JPEG (~30-60KB vs ~2MB PNG)
    let img = image::load_from_memory(&png_data).ok()?;
    let thumb = img.thumbnail(360, 640);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut buf, image::ImageFormat::Jpeg).ok()?;
    Some(STANDARD.encode(buf.into_inner()))
}
