use std::{io::Read, sync::Arc};
use tauri::Manager;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::ws::WsHub;

// (placeholder) server_args will be used by protocol-level client
// use super::device::server_args;

/// Render a scrcpy codec_id FourCC as a short string for logs.
///
/// scrcpy 3.x encodes `codec_id` as a 4-byte ASCII FourCC in big-endian order.
/// Known video codecs: `h264` (0x68323634), `h265` (0x68323635), `\x00av1`
/// (0x00617631). Unknown values render in hex.
pub(crate) fn parse_codec_fourcc(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        return format!("<{} bytes>", bytes.len());
    }
    let b = &bytes[0..4];
    if b.iter().all(|&c| c.is_ascii_graphic() || c == b' ') {
        // All printable → render as the ASCII it is
        String::from_utf8_lossy(b).to_string()
    } else if b[0] == 0 && b[1..].iter().all(|&c| c.is_ascii_graphic()) {
        // "\x00av1" style — show the printable suffix
        String::from_utf8_lossy(&b[1..]).to_string()
    } else {
        format!("0x{:02x}{:02x}{:02x}{:02x}", b[0], b[1], b[2], b[3])
    }
}

const CONNECT_BASE_DELAY_MS: u64 = 100;
const CONNECT_MAX_DELAY_MS: u64 = 1500;
const MAX_RECONNECT_SLEEP_MS: u64 = 5_000;
const USB_BOUNCE_FAST_RETRY_WINDOW_MS: u64 = 5_000;
const STABLE_DISCONNECT_RECONNECT_DELAY_MS: u64 = 500;

fn reconnect_delay_ms(
    serial: &str,
    attempt: u32,
    device_not_found: bool,
    recent_stream_drop: bool,
) -> u64 {
    let raw_delay = if device_not_found {
        if recent_stream_drop {
            // USB mode switching briefly removes the device from ADB. Poll
            // quickly right after a stable stream drops so we do not add
            // seconds of artificial latency before ADB re-enumerates it.
            let base = std::cmp::min(900, 120 * attempt.min(8) as u64);
            base + reconnect_jitter_ms(serial, 180)
        } else {
            let base = std::cmp::min(15_000, 2_000 * attempt.min(6) as u64);
            base + reconnect_jitter_ms(serial, 2_500)
        }
    } else {
        std::cmp::min(
            CONNECT_MAX_DELAY_MS,
            CONNECT_BASE_DELAY_MS * (1u64 << attempt.min(3)),
        ) + reconnect_jitter_ms(serial, 250)
    };
    raw_delay.min(MAX_RECONNECT_SLEEP_MS)
}

pub type StreamTokens = Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>;
pub struct ControlEntry {
    pub stream: std::net::TcpStream,
    pub video_width: u32,
    pub video_height: u32,
}

pub type ControlSockets = Arc<std::sync::Mutex<std::collections::HashMap<String, ControlEntry>>>;

pub fn new_tokens() -> StreamTokens {
    Arc::new(Mutex::new(std::collections::HashMap::new()))
}

pub fn new_control_sockets() -> ControlSockets {
    Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn remove_control_socket(control_sockets: &ControlSockets, serial: &str, reason: &str) {
    if let Ok(mut sockets) = control_sockets.lock() {
        if sockets.remove(serial).is_some() {
            println!(
                "[SCRCPY-CTRL] removed control socket serial={} reason={}",
                serial, reason
            );
        }
    }
}

fn emit_stream_status(app: &AppHandle, serial: &str, status: &str, error: Option<&str>) {
    let payload = match error {
        Some(error) => serde_json::json!({"serial": serial, "status": status, "error": error}),
        None => serde_json::json!({"serial": serial, "status": status}),
    };
    let _ = app.emit("stream-status", payload);
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct StreamOptions {
    pub max_size: u32,
    pub max_fps: u32,
    pub bit_rate: u32,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            max_size: 720,
            max_fps: 30,
            bit_rate: 4_000_000,
        }
    }
}

fn reconnect_jitter_ms(serial: &str, modulo: u64) -> u64 {
    if modulo == 0 {
        0
    } else {
        fxhash::hash64(serial) % modulo
    }
}

/// Start a scrcpy-based video stream.
///
/// This currently records to stdout as an MKV stream, then decodes frames and emits JPEG data.
///
/// Notes:
/// - Control is intentionally disabled (Phase 1).
/// - For simplicity, we emit JPEG base64 via a Tauri event; this can later be optimized to binary.
pub async fn start_stream_loop(
    tokens: StreamTokens,
    control_sockets: ControlSockets,
    adb_semaphore: Arc<Semaphore>,
    serial: String,
    host: String,
    port: u16,
    opts: StreamOptions,
    app: AppHandle,
) {
    println!(
        "[STREAM] start_stream_loop serial={} server={}:{} opts={{max_size={}, max_fps={}, bit_rate={}}}",
        serial, host, port, opts.max_size, opts.max_fps, opts.bit_rate
    );
    let token = CancellationToken::new();
    {
        let mut map = tokens.lock().await;
        if let Some(old) = map.insert(serial.clone(), token.clone()) {
            old.cancel();
        }
    }

    // Backoff for connection-attempt failures. Large batches can make many
    // devices re-enumerate at once, so device-not-found retries use a longer
    // staggered delay to avoid a reconnect storm against the ADB server.
    let mut attempt: u32 = 0;
    let mut first_run = true;
    let mut disconnect_count: u32 = 0;
    let mut fast_retry_until: Option<std::time::Instant> = None;

    loop {
        if token.is_cancelled() {
            break;
        }

        // Cleanup from previous iteration
        remove_control_socket(&control_sockets, &serial, "stream iteration restart");

        let status_label = if first_run {
            "starting"
        } else {
            "reconnecting"
        };
        let _ = app.emit(
            "stream-status",
            serde_json::json!({"serial": serial, "status": status_label}),
        );
        if !first_run {
            println!(
                "[STREAM] reconnecting serial={} attempt={}",
                serial, attempt
            );
        }
        first_run = false;

        // Connect to scrcpy server. This is ADB-heavy, so share a global
        // limiter with group input commands to avoid starving tap/swipe.
        let adb_permit = tokio::select! {
            _ = token.cancelled() => { break; }
            permit = Arc::clone(&adb_semaphore).acquire_owned() => {
                match permit {
                    Ok(permit) => permit,
                    Err(_) => break,
                }
            }
        };

        let serial_clone = serial.clone();
        let host_clone = host.clone();
        let opts_clone = opts.clone();
        let conn_result = tokio::task::spawn_blocking(move || {
            super::scrcpy_client::start_scrcpy_and_connect(
                &serial_clone,
                &host_clone,
                port,
                &opts_clone,
            )
        })
        .await;
        drop(adb_permit);

        let scrcpy_conn = match conn_result {
            Ok(Ok(c)) => {
                attempt = 0;
                fast_retry_until = None;
                c
            }
            Ok(Err(e)) => {
                println!("[STREAM] connect failed serial={}: {}", serial, e);
                let device_not_found = e.contains("device") && e.contains("not found");
                let _ = app.emit(
                    "stream-status",
                    serde_json::json!({"serial": serial, "status": "reconnecting", "error": e}),
                );
                attempt += 1;
                let recent_stream_drop = fast_retry_until
                    .map(|deadline| std::time::Instant::now() <= deadline)
                    .unwrap_or(false);
                let delay =
                    reconnect_delay_ms(&serial, attempt, device_not_found, recent_stream_drop);
                println!(
                    "[STREAM] reconnect sleep serial={} attempt={} delay={}ms mode={}",
                    serial,
                    attempt,
                    delay,
                    if device_not_found && recent_stream_drop {
                        "usb-bounce-fast"
                    } else if device_not_found {
                        "device-not-found"
                    } else {
                        "normal"
                    }
                );
                tokio::select! {
                    _ = token.cancelled() => { break; }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
                }
                continue;
            }
            Err(e) => {
                println!("[STREAM] connect task panicked serial={}: {}", serial, e);
                attempt += 1;
                let delay = reconnect_delay_ms(&serial, attempt, false, false);
                tokio::select! {
                    _ = token.cancelled() => { break; }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
                }
                continue;
            }
        };

        let _ = app.emit(
            "stream-status",
            serde_json::json!({"serial": serial, "status": "connected"}),
        );
        let connected_at = std::time::Instant::now();
        let stdout = scrcpy_conn.stream;
        let local_port = scrcpy_conn.local_port;
        let mut server_child = scrcpy_conn.server_child;

        if let Some(ctrl) = scrcpy_conn.control {
            control_sockets.lock().unwrap().insert(
                serial.clone(),
                ControlEntry {
                    stream: ctrl,
                    video_width: 0,
                    video_height: 0,
                },
            );
        }

        let serial_for_task = serial.clone();
        let token_for_task = token.clone();
        let app_for_task = app.clone();
        let hub = app.state::<WsHub>().inner().clone();
        let cs_for_task = Arc::clone(&control_sockets);
        let forward_task = tokio::task::spawn_blocking(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                forward_h264_to_ws(
                    stdout,
                    &serial_for_task,
                    &token_for_task,
                    &app_for_task,
                    &hub,
                    &cs_for_task,
                )
            }));

            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let _ = app_for_task.emit(
                        "stream-error",
                        serde_json::json!({ "serial": &serial_for_task, "error": e }),
                    );
                }
                Err(_) => {
                    let _ = app_for_task.emit(
                        "stream-error",
                        serde_json::json!({ "serial": &serial_for_task, "error": "panic in stream forward" }),
                    );
                }
            }

            remove_control_socket(&cs_for_task, &serial_for_task, "stream forward ended");
        });

        let cancelled = tokio::select! {
            _ = token.cancelled() => true,
            _ = forward_task => {
                println!("[STREAM] stream disconnected serial={}", serial);
                remove_control_socket(&control_sockets, &serial, "stream disconnected");
                false
            }
        };

        let _ = server_child.kill();
        super::scrcpy_client::remove_forward(&host, port, &serial, local_port);
        println!(
            "[SCRCPY] forward listener removed serial={} port={} reason=stream ended",
            serial, local_port
        );
        if cancelled {
            break;
        }

        let alive_ms = connected_at.elapsed().as_millis() as u64;
        if alive_ms >= 2_000 {
            disconnect_count = 0;
            fast_retry_until = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_millis(USB_BOUNCE_FAST_RETRY_WINDOW_MS),
            );
            println!(
                "[STREAM] stable-disconnect reconnect sleep serial={} alive={}ms delay={}ms",
                serial, alive_ms, STABLE_DISCONNECT_RECONNECT_DELAY_MS
            );
            tokio::select! {
                _ = token.cancelled() => { break; }
                _ = tokio::time::sleep(std::time::Duration::from_millis(STABLE_DISCONNECT_RECONNECT_DELAY_MS)) => {}
            }
            continue;
        } else {
            disconnect_count += 1;
            let raw_delay = std::cmp::min(5_000, 500 * disconnect_count.min(8) as u64)
                + reconnect_jitter_ms(&serial, 500);
            let delay = raw_delay.min(MAX_RECONNECT_SLEEP_MS);
            println!(
                "[STREAM] rapid-disconnect backoff serial={} count={} alive={}ms delay={}ms",
                serial, disconnect_count, alive_ms, delay
            );
            tokio::select! {
                _ = token.cancelled() => { break; }
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
            }
        }
    }

    // Final cleanup
    remove_control_socket(&control_sockets, &serial, "stream loop stopped");
    tokens.lock().await.remove(&serial);
    let _ = app.emit(
        "stream-status",
        serde_json::json!({"serial": serial, "status": "stopped"}),
    );
}

pub async fn stop_stream_loop(tokens: StreamTokens, control_sockets: ControlSockets, serial: &str) {
    remove_control_socket(&control_sockets, serial, "stop stream requested");
    let mut map = tokens.lock().await;
    if let Some(token) = map.remove(serial) {
        token.cancel();
    }
}

/// Forward raw H.264 packets from the scrcpy stream to the WebSocket hub.
///
/// Instead of decoding H.264 and re-encoding as JPEG, we send the raw NAL
/// units to the frontend where WebCodecs `VideoDecoder` handles GPU decoding.
fn forward_h264_to_ws<R: Read + Send + 'static>(
    mut input: R,
    serial: &str,
    token: &CancellationToken,
    app: &AppHandle,
    hub: &WsHub,
    control_sockets: &ControlSockets,
) -> Result<(), String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];

    let mut dummy_consumed = false;
    let mut codec_meta_consumed = false;
    let mut video_width: u32 = 0;
    let mut video_height: u32 = 0;
    let mut invalid_header_hits: u32 = 0;
    let mut first_data_at: Option<std::time::Instant> = None;
    let loop_start = std::time::Instant::now();
    let mut last_idle_log: Option<std::time::Instant> = None;
    let mut last_config: Option<Vec<u8>> = None;

    println!("[SCRCPY-FWD] entering read loop serial={}", serial);

    while !token.is_cancelled() {
        let n = match input.read(&mut chunk) {
            Ok(0) => {
                remove_control_socket(control_sockets, serial, "stream EOF");
                emit_stream_status(
                    app,
                    serial,
                    "disconnected",
                    Some("video stream disconnected"),
                );
                println!(
                    "[SCRCPY-FWD] stream EOF serial={} after {:?}",
                    serial,
                    loop_start.elapsed()
                );
                break;
            }
            Ok(n) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                let now = std::time::Instant::now();
                let should_log = last_idle_log
                    .map(|t| now.duration_since(t) >= std::time::Duration::from_secs(5))
                    .unwrap_or(true);
                if should_log && first_data_at.is_none() {
                    last_idle_log = Some(now);
                    println!(
                        "[SCRCPY-FWD] no bytes from server serial={} waited={:?}",
                        serial,
                        loop_start.elapsed()
                    );
                }
                continue;
            }
            Err(e) => {
                remove_control_socket(control_sockets, serial, "stream read error");
                emit_stream_status(app, serial, "disconnected", Some("video stream read error"));
                return Err(format!("read scrcpy stream failed: {e}"));
            }
        };

        if first_data_at.is_none() {
            first_data_at = Some(std::time::Instant::now());
            println!("[SCRCPY-FWD] first bytes serial={} n={}", serial, n);
            let _ = app.emit(
                "stream-status",
                serde_json::json!({"serial": serial, "status": "receiving"}),
            );
        }
        buf.extend_from_slice(&chunk[..n]);

        if !dummy_consumed && !buf.is_empty() {
            if buf[0] == 0 {
                buf.drain(..1);
            }
            dummy_consumed = true;
        }
        if dummy_consumed && !codec_meta_consumed && buf.len() >= 12 {
            let fourcc = parse_codec_fourcc(&buf[0..4]);
            video_width = u32::from_be_bytes(buf[4..8].try_into().unwrap());
            video_height = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            println!(
                "[SCRCPY-FWD] codec meta serial={} codec={} w={} h={}",
                serial, fourcc, video_width, video_height
            );
            if let Ok(mut sockets) = control_sockets.lock() {
                if let Some(entry) = sockets.get_mut(serial) {
                    entry.video_width = video_width;
                    entry.video_height = video_height;
                    println!(
                        "[SCRCPY-FWD] updated control socket video size serial={} {}x{}",
                        serial, video_width, video_height
                    );
                }
            }
            buf.drain(..12);
            codec_meta_consumed = true;
        }

        if !codec_meta_consumed {
            continue;
        }

        while buf.len() >= 12 {
            let pts_raw = u64::from_be_bytes(buf[0..8].try_into().unwrap());
            let packet_size = u32::from_be_bytes(buf[8..12].try_into().unwrap()) as usize;
            if packet_size == 0 {
                buf.drain(..12);
                continue;
            }
            if packet_size > 10 * 1024 * 1024 {
                invalid_header_hits = invalid_header_hits.saturating_add(1);
                if invalid_header_hits % 2000 == 1 {
                    let head_len = std::cmp::min(16, buf.len());
                    let mut hex = String::new();
                    for b in &buf[..head_len] {
                        use std::fmt::Write as _;
                        let _ = write!(&mut hex, "{:02x}", b);
                    }
                    println!(
                        "[SCRCPY-FWD] invalid header serial={} buf_len={} head={}...",
                        serial,
                        buf.len(),
                        hex
                    );
                }
                buf.drain(..1);
                continue;
            }

            invalid_header_hits = 0;
            if buf.len() < 12 + packet_size {
                break;
            }

            let is_config = (pts_raw >> 63) & 1 == 1;
            let is_key = (pts_raw >> 62) & 1 == 1;
            let pts = pts_raw & 0x3FFF_FFFF_FFFF_FFFF;

            let nal_data = &buf[12..12 + packet_size];

            if is_config {
                // Config packet — cache and forward as type 0
                last_config = Some(nal_data.to_vec());
                let packed =
                    WsHub::pack_h264_frame(serial, 0, pts, video_width, video_height, nal_data);
                hub.broadcast(serial, packed);
            } else {
                let packet_type = if is_key { 1u8 } else { 2 };
                // For keyframes, prepend the last config (SPS/PPS) so the
                // decoder can be (re-)configured even if it missed the
                // initial config packet due to late WS subscription.
                if is_key {
                    if let Some(ref cfg) = last_config {
                        let packed =
                            WsHub::pack_h264_frame(serial, 0, pts, video_width, video_height, cfg);
                        hub.broadcast(serial, packed);
                    }
                }
                let packed = WsHub::pack_h264_frame(
                    serial,
                    packet_type,
                    pts,
                    video_width,
                    video_height,
                    nal_data,
                );
                hub.broadcast(serial, packed);
            }

            buf.drain(..12 + packet_size);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_codec_fourcc_h264() {
        // scrcpy 3.x codec_id for H.264 is the ASCII FourCC "h264".
        let bytes = [0x68u8, 0x32, 0x36, 0x34];
        assert_eq!(parse_codec_fourcc(&bytes), "h264");
    }

    #[test]
    fn parse_codec_fourcc_h265() {
        let bytes = [0x68u8, 0x32, 0x36, 0x35];
        assert_eq!(parse_codec_fourcc(&bytes), "h265");
    }

    #[test]
    fn parse_codec_fourcc_av1() {
        // AV1 uses a leading NUL: "\x00av1" (0x00617631).
        let bytes = [0x00u8, 0x61, 0x76, 0x31];
        assert_eq!(parse_codec_fourcc(&bytes), "av1");
    }

    #[test]
    fn parse_codec_fourcc_h264_is_not_in_1_to_3() {
        // Regression guard: the old code used `(1..=3).contains(&codec_id)`
        // which never matched because the FourCC is a large integer, so
        // codec meta was never drained.
        let codec_id = u32::from_be_bytes([0x68, 0x32, 0x36, 0x34]);
        assert!(
            !(1..=3).contains(&codec_id),
            "h264 FourCC = {codec_id} must not match the old 1..=3 check"
        );
    }

    #[test]
    fn parse_codec_fourcc_unknown_renders_hex() {
        let bytes = [0x01u8, 0x02, 0x03, 0x04];
        assert_eq!(parse_codec_fourcc(&bytes), "0x01020304");
    }

    #[test]
    fn parse_codec_fourcc_short_bytes() {
        assert_eq!(parse_codec_fourcc(&[0x68, 0x32]), "<2 bytes>");
    }

    #[test]
    fn reconnect_delay_uses_fast_retry_after_stable_stream_drop() {
        let delay = reconnect_delay_ms("device-a", 1, true, true);

        assert!(
            delay < 300,
            "first USB-bounce retry should not wait multiple seconds, got {delay}ms"
        );
    }

    #[test]
    fn reconnect_delay_keeps_longer_stagger_for_cold_device_not_found() {
        let delay = reconnect_delay_ms("device-a", 1, true, false);

        assert!(
            delay >= 2_000,
            "cold device-not-found retry should stay staggered, got {delay}ms"
        );
    }

    #[test]
    fn reconnect_delay_is_capped() {
        let delay = reconnect_delay_ms("device-a", 6, true, false);

        assert_eq!(delay, MAX_RECONNECT_SLEEP_MS);
    }
}
