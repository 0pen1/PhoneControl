use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;
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
    let png_data = capture_screenshot_png(serial, host, port).await?;
    tokio::task::spawn_blocking(move || encode_screenshot_jpeg(png_data))
        .await
        .ok()
        .flatten()
}

async fn capture_screenshot_png(serial: &str, host: &str, port: u16) -> Option<Vec<u8>> {
    let mut args = server_args(host, port);
    args.extend([
        "-s".into(),
        serial.into(),
        "exec-out".into(),
        "screencap".into(),
        "-p".into(),
    ]);

    let mut child = tokio::process::Command::new("adb")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.ok()?;
        Some(buf)
    });

    let status = match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = stdout_task.await;
            return None;
        }
    };

    let stdout = stdout_task.await.ok().flatten()?;
    if status.success() && !stdout.is_empty() {
        Some(stdout)
    } else {
        None
    }
}

fn encode_screenshot_jpeg(png_data: Vec<u8>) -> Option<String> {
    // Decode PNG, scale down, re-encode as JPEG (~30-60KB vs ~2MB PNG)
    let img = image::load_from_memory(&png_data).ok()?;
    let thumb = img.thumbnail(360, 640);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut buf, image::ImageFormat::Jpeg).ok()?;
    Some(STANDARD.encode(buf.into_inner()))
}
