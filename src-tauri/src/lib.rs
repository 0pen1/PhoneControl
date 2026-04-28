pub mod adb;
mod config;
mod state;
mod ws;

use adb::commands::{keyevent, send_text, set_usb_file_transfer, wake_up_device, CommandResult};
use adb::scrcpy_control;
use adb::screenshot::{start_screenshot_loop, stop_screenshot_loop};
use adb::server::{poll_all_servers, AdbServer};
use adb::stream::{start_stream_loop, stop_stream_loop, StreamOptions};
use config::{load_servers, save_servers, ServerConfig};
use state::{AppState, ADB_PERMITS};

use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};

use ws::{run_ws_server, WsHub};

// ── Server management ────────────────────────────────────────────────────────

#[tauri::command]
async fn add_server(
    host: String,
    port: u16,
    state: State<'_, AppState>,
) -> Result<AdbServer, String> {
    let mut servers = state.servers.lock().await;
    if servers.iter().any(|s| s.host == host && s.port == port) {
        return Err("Server already exists".into());
    }
    let srv = AdbServer::new(host, port);
    servers.push(srv.clone());
    let cfgs: Vec<ServerConfig> = servers
        .iter()
        .map(|s| ServerConfig {
            host: s.host.clone(),
            port: s.port,
            enabled: s.enabled,
        })
        .collect();
    drop(servers);
    save_servers(&cfgs)?;
    Ok(srv)
}

#[tauri::command]
async fn remove_server(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut servers = state.servers.lock().await;
    servers.retain(|s| s.id != id);
    let cfgs: Vec<ServerConfig> = servers
        .iter()
        .map(|s| ServerConfig {
            host: s.host.clone(),
            port: s.port,
            enabled: s.enabled,
        })
        .collect();
    drop(servers);
    save_servers(&cfgs)
}

#[tauri::command]
async fn toggle_server(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut servers = state.servers.lock().await;
    if let Some(s) = servers.iter_mut().find(|s| s.id == id) {
        s.enabled = enabled;
    }
    let cfgs: Vec<ServerConfig> = servers
        .iter()
        .map(|s| ServerConfig {
            host: s.host.clone(),
            port: s.port,
            enabled: s.enabled,
        })
        .collect();
    drop(servers);
    save_servers(&cfgs)
}

#[tauri::command]
async fn get_servers(state: State<'_, AppState>) -> Result<Vec<AdbServer>, String> {
    Ok(state.servers.lock().await.clone())
}

// ── Preview ──────────────────────────────────────────────────────────────────

#[tauri::command]
async fn start_preview(
    serial: String,
    fps: u32,
    server_host: String,
    server_port: u16,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let tokens = Arc::clone(&state.screenshot_tokens);
    tauri::async_runtime::spawn(start_screenshot_loop(
        tokens,
        serial,
        server_host,
        server_port,
        fps,
        app,
    ));
    Ok(())
}

#[tauri::command]
async fn stop_preview(serial: String, state: State<'_, AppState>) -> Result<(), String> {
    stop_screenshot_loop(Arc::clone(&state.screenshot_tokens), &serial).await;
    Ok(())
}

// ── Stream preview (scrcpy) ─────────────────────────────────────────────────

#[tauri::command]
async fn start_stream(
    serial: String,
    server_host: String,
    server_port: u16,
    options: Option<StreamOptions>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    println!(
        "[CMD] start_stream serial={} server={}:{}",
        serial, server_host, server_port
    );
    let tokens = Arc::clone(&state.stream_tokens);
    let control_sockets = Arc::clone(&state.control_sockets);
    let adb_semaphore = Arc::clone(&state.adb_semaphore);
    let opts = options.unwrap_or_default();
    tauri::async_runtime::spawn(start_stream_loop(
        tokens,
        control_sockets,
        adb_semaphore,
        serial,
        server_host,
        server_port,
        opts,
        app,
    ));
    Ok(())
}

#[tauri::command]
async fn stop_stream(serial: String, state: State<'_, AppState>) -> Result<(), String> {
    stop_stream_loop(
        Arc::clone(&state.stream_tokens),
        Arc::clone(&state.control_sockets),
        &serial,
    )
    .await;
    Ok(())
}

#[tauri::command]
async fn set_fps(
    serial: String,
    fps: u32,
    server_host: String,
    server_port: u16,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    stop_screenshot_loop(Arc::clone(&state.screenshot_tokens), &serial).await;
    let tokens = Arc::clone(&state.screenshot_tokens);
    tauri::async_runtime::spawn(start_screenshot_loop(
        tokens,
        serial,
        server_host,
        server_port,
        fps,
        app,
    ));
    Ok(())
}

// ── Group control ────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Clone)]
pub struct DeviceResolution {
    pub serial: String,
    pub width: u32,
    pub height: u32,
    pub server_host: String,
    pub server_port: u16,
}

async fn run_adb_taps(
    serials: &[DeviceResolution],
    x: f64,
    y: f64,
    source_width: u32,
    source_height: u32,
) -> Vec<CommandResult> {
    const BATCH: usize = 1;
    let mut results = Vec::with_capacity(serials.len());

    for (batch_idx, chunk) in serials.chunks(BATCH).enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|d| {
                let d = d.clone();
                tokio::task::spawn_blocking(move || {
                    adb::commands::tap(
                        &d.server_host,
                        d.server_port,
                        &d.serial,
                        x,
                        y,
                        source_width,
                        source_height,
                        d.width,
                        d.height,
                    )
                })
            })
            .collect();
        for h in handles {
            match h.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(CommandResult {
                    serial: "__adb_worker__".into(),
                    success: false,
                    message: format!("ADB tap worker failed: {}", e),
                }),
            }
        }
    }

    results
}

async fn run_adb_swipes(
    serials: &[DeviceResolution],
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u32,
    source_width: u32,
    source_height: u32,
) -> Vec<CommandResult> {
    const BATCH: usize = 1;
    let mut results = Vec::with_capacity(serials.len());

    for (batch_idx, chunk) in serials.chunks(BATCH).enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let handles: Vec<_> = chunk
            .iter()
            .map(|d| {
                let d = d.clone();
                tokio::task::spawn_blocking(move || {
                    adb::commands::swipe(
                        &d.server_host,
                        d.server_port,
                        &d.serial,
                        x1,
                        y1,
                        x2,
                        y2,
                        duration_ms,
                        source_width,
                        source_height,
                        d.width,
                        d.height,
                    )
                })
            })
            .collect();
        for h in handles {
            match h.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(CommandResult {
                    serial: "__adb_worker__".into(),
                    success: false,
                    message: format!("ADB swipe worker failed: {}", e),
                }),
            }
        }
    }

    results
}

async fn run_control_taps(
    serials: &[DeviceResolution],
    control_sockets: adb::stream::ControlSockets,
    x: f64,
    y: f64,
    source_width: u32,
    source_height: u32,
) -> (Vec<CommandResult>, Vec<DeviceResolution>) {
    const CONTROL_BATCH_SIZE: usize = 1;
    const CONTROL_BATCH_GAP_MS: u64 = 120;

    let mut results = Vec::new();
    let mut fallback = Vec::new();

    for (batch_idx, chunk) in serials.chunks(CONTROL_BATCH_SIZE).enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(Duration::from_millis(CONTROL_BATCH_GAP_MS)).await;
        }

        let mut handles = Vec::new();
        for d in chunk {
            let socket = {
                let sockets = control_sockets.lock().unwrap();
                match sockets.get(&d.serial) {
                    Some(entry) if entry.video_width > 0 && entry.video_height > 0 => {
                        match entry.stream.try_clone() {
                            Ok(stream) => Some((stream, entry.video_width, entry.video_height)),
                            Err(e) => {
                                println!("[TAP] control clone failed serial={}: {}", d.serial, e);
                                None
                            }
                        }
                    }
                    _ => None,
                }
            };

            let Some((mut stream, video_width, video_height)) = socket else {
                fallback.push(d.clone());
                continue;
            };

            println!(
                "[TAP] scrcpy control send serial={} batch={} screen={}x{}",
                d.serial, batch_idx, video_width, video_height
            );
            let serial = d.serial.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let result = scrcpy_control::inject_tap(
                    &mut stream,
                    x,
                    y,
                    source_width,
                    source_height,
                    video_width,
                    video_height,
                );
                CommandResult {
                    serial,
                    success: result.is_ok(),
                    message: result.err().unwrap_or_default(),
                }
            });

            handles.push((d.clone(), handle));
        }

        for (device, handle) in handles {
            match handle.await {
                Ok(result) => {
                    if !result.success {
                        control_sockets.lock().unwrap().remove(&result.serial);
                        println!(
                            "[TAP] scrcpy control failed serial={}: {}; falling back to ADB",
                            result.serial, result.message
                        );
                        fallback.push(device);
                        continue;
                    }

                    results.push(result);
                }
                Err(e) => {
                    println!(
                        "[TAP] scrcpy control tap worker failed serial={}: {}; falling back to ADB",
                        device.serial, e
                    );
                    fallback.push(device);
                }
            }
        }
    }

    (results, fallback)
}

async fn run_control_swipes(
    serials: &[DeviceResolution],
    control_sockets: adb::stream::ControlSockets,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u32,
    source_width: u32,
    source_height: u32,
) -> (Vec<CommandResult>, Vec<DeviceResolution>) {
    const CONTROL_BATCH_SIZE: usize = 4;
    const CONTROL_BATCH_GAP_MS: u64 = 25;

    let mut results = Vec::new();
    let mut fallback = Vec::new();

    for (batch_idx, chunk) in serials.chunks(CONTROL_BATCH_SIZE).enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(Duration::from_millis(CONTROL_BATCH_GAP_MS)).await;
        }

        let mut handles = Vec::new();
        for d in chunk {
            let socket = {
                let sockets = control_sockets.lock().unwrap();
                match sockets.get(&d.serial) {
                    Some(entry) if entry.video_width > 0 && entry.video_height > 0 => {
                        match entry.stream.try_clone() {
                            Ok(stream) => Some((
                                stream,
                                entry.video_width,
                                entry.video_height,
                                entry.stream.local_addr().ok(),
                                entry.stream.peer_addr().ok(),
                            )),
                            Err(e) => {
                                println!("[SWIPE] control clone failed serial={}: {}", d.serial, e);
                                None
                            }
                        }
                    }
                    _ => None,
                }
            };

            let Some((mut stream, video_width, video_height, local_addr, peer_addr)) = socket
            else {
                fallback.push(d.clone());
                continue;
            };

            println!(
                "[SWIPE] scrcpy control send serial={} batch={} screen={}x{}",
                d.serial, batch_idx, video_width, video_height
            );
            let serial = d.serial.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let result = scrcpy_control::inject_swipe(
                    &mut stream,
                    x1,
                    y1,
                    x2,
                    y2,
                    duration_ms,
                    source_width,
                    source_height,
                    video_width,
                    video_height,
                );
                CommandResult {
                    serial,
                    success: result.is_ok(),
                    message: result.err().unwrap_or_default(),
                }
            });

            handles.push((
                d.clone(),
                local_addr,
                peer_addr,
                video_width,
                video_height,
                handle,
            ));
        }

        for (device, local_addr, peer_addr, video_width, video_height, handle) in handles {
            match handle.await {
                Ok(result) => {
                    if !result.success {
                        control_sockets.lock().unwrap().remove(&result.serial);
                        println!(
                            "[SWIPE] scrcpy control failed serial={}: {}; falling back to ADB",
                            result.serial, result.message
                        );
                        fallback.push(device);
                        continue;
                    }

                    let still_current = {
                        let sockets = control_sockets.lock().unwrap();
                        sockets
                            .get(&result.serial)
                            .map(|entry| {
                                entry.stream.local_addr().ok() == local_addr
                                    && entry.stream.peer_addr().ok() == peer_addr
                                    && entry.video_width == video_width
                                    && entry.video_height == video_height
                            })
                            .unwrap_or(false)
                    };

                    if still_current {
                        results.push(result);
                    } else {
                        println!(
                            "[SWIPE] scrcpy control disconnected during swipe serial={}; falling back to ADB",
                            result.serial
                        );
                        fallback.push(device);
                    }
                }
                Err(e) => {
                    println!(
                        "[SWIPE] scrcpy control swipe worker failed serial={}: {}; falling back to ADB",
                        device.serial, e
                    );
                    fallback.push(device);
                }
            }
        }
    }

    (results, fallback)
}

#[tauri::command]
async fn tap_devices(
    serials: Vec<DeviceResolution>,
    x: f64,
    y: f64,
    source_width: u32,
    source_height: u32,
    state: State<'_, AppState>,
) -> Result<Vec<CommandResult>, String> {
    println!(
        "[TAP] tap_devices called: {} devices, x={:.1} y={:.1} src={}x{}",
        serials.len(),
        x,
        y,
        source_width,
        source_height
    );

    let (mut results, fallback) = run_control_taps(
        &serials,
        Arc::clone(&state.control_sockets),
        x,
        y,
        source_width,
        source_height,
    )
    .await;

    if !fallback.is_empty() {
        println!(
            "[TAP] {} devices need ADB fallback because scrcpy control is unavailable or changed",
            fallback.len()
        );
        println!("[TAP] waiting for exclusive ADB gate");
        let _adb_exclusive = Arc::clone(&state.adb_semaphore)
            .acquire_many_owned(ADB_PERMITS)
            .await
            .map_err(|e| e.to_string())?;
        println!("[TAP] acquired exclusive ADB gate");
        results.extend(run_adb_taps(&fallback, x, y, source_width, source_height).await);
    }

    let ok = results.iter().filter(|r| r.success).count();
    let fail = results.len() - ok;
    println!(
        "[TAP] done: {} ok, {} failed (scrcpy-control={}, adb-fallback={}, devices={})",
        ok,
        fail,
        serials.len() - fallback.len(),
        fallback.len(),
        serials.len()
    );
    Ok(results)
}

#[tauri::command]
async fn swipe_devices(
    serials: Vec<DeviceResolution>,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u32,
    source_width: u32,
    source_height: u32,
    state: State<'_, AppState>,
) -> Result<Vec<CommandResult>, String> {
    println!(
        "[SWIPE] swipe_devices called: {} devices, dur={}ms",
        serials.len(),
        duration_ms
    );

    let (mut results, fallback) = run_control_swipes(
        &serials,
        Arc::clone(&state.control_sockets),
        x1,
        y1,
        x2,
        y2,
        duration_ms,
        source_width,
        source_height,
    )
    .await;

    if !fallback.is_empty() {
        println!(
            "[SWIPE] {} devices need ADB fallback because scrcpy control is unavailable or changed",
            fallback.len()
        );
        println!("[SWIPE] waiting for exclusive ADB gate");
        let _adb_exclusive = Arc::clone(&state.adb_semaphore)
            .acquire_many_owned(ADB_PERMITS)
            .await
            .map_err(|e| e.to_string())?;
        println!("[SWIPE] acquired exclusive ADB gate");
        results.extend(
            run_adb_swipes(
                &fallback,
                x1,
                y1,
                x2,
                y2,
                duration_ms,
                source_width,
                source_height,
            )
            .await,
        );
    }

    let ok = results.iter().filter(|r| r.success).count();
    let fail = results.len() - ok;
    println!(
        "[SWIPE] done: {} ok, {} failed (scrcpy-control={}, adb-fallback={}, devices={})",
        ok,
        fail,
        serials.len() - fallback.len(),
        fallback.len(),
        serials.len()
    );
    Ok(results)
}

#[tauri::command]
async fn send_text_devices(
    serials: Vec<DeviceResolution>,
    text: String,
) -> Result<Vec<CommandResult>, String> {
    let handles: Vec<_> = serials
        .into_iter()
        .map(|d| {
            let text = text.clone();
            tokio::task::spawn_blocking(move || {
                send_text(&d.server_host, d.server_port, &d.serial, &text)
            })
        })
        .collect();
    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        results.push(h.await.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

#[tauri::command]
async fn keyevent_devices(
    serials: Vec<DeviceResolution>,
    keycode: u32,
) -> Result<Vec<CommandResult>, String> {
    let handles: Vec<_> = serials
        .into_iter()
        .map(|d| {
            tokio::task::spawn_blocking(move || {
                keyevent(&d.server_host, d.server_port, &d.serial, keycode)
            })
        })
        .collect();
    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        results.push(h.await.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

#[tauri::command]
async fn set_usb_file_transfer_devices(
    serials: Vec<DeviceResolution>,
    state: State<'_, AppState>,
) -> Result<Vec<CommandResult>, String> {
    println!(
        "[USB-MTP] set_usb_file_transfer_devices called: {} devices",
        serials.len()
    );

    println!("[USB-MTP] waiting for exclusive ADB gate");
    let _adb_exclusive = Arc::clone(&state.adb_semaphore)
        .acquire_many_owned(ADB_PERMITS)
        .await
        .map_err(|e| e.to_string())?;
    println!("[USB-MTP] acquired exclusive ADB gate");

    let mut results = Vec::with_capacity(serials.len());
    for (idx, d) in serials.into_iter().enumerate() {
        if idx > 0 {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let result = tokio::task::spawn_blocking(move || {
            set_usb_file_transfer(&d.server_host, d.server_port, &d.serial)
        })
        .await
        .map_err(|e| e.to_string())?;
        results.push(result);
    }

    let ok = results.iter().filter(|r| r.success).count();
    let fail = results.len() - ok;
    println!(
        "[USB-MTP] done: {} ok, {} failed (batch=1, devices={})",
        ok,
        fail,
        results.len()
    );
    Ok(results)
}

#[tauri::command]
async fn wake_up_devices(serials: Vec<DeviceResolution>) -> Result<Vec<CommandResult>, String> {
    let handles: Vec<_> = serials
        .into_iter()
        .map(|d| {
            tokio::task::spawn_blocking(move || {
                wake_up_device(&d.server_host, d.server_port, &d.serial)
            })
        })
        .collect();
    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        results.push(h.await.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

// ── scrcpy control ───────────────────────────────────────────────────────────

#[tauri::command]
async fn scrcpy_tap(
    serial: String,
    x: f64,
    y: f64,
    source_width: u32,
    source_height: u32,
    _target_width: u32,
    _target_height: u32,
    _server_host: String,
    _server_port: u16,
    state: State<'_, AppState>,
) -> Result<CommandResult, String> {
    println!(
        "[SCRCPY-CTRL] tap serial={} x={:.1} y={:.1} src={}x{}",
        serial, x, y, source_width, source_height
    );
    let mut sockets = state.control_sockets.lock().unwrap();
    if let Some(entry) = sockets.get_mut(&serial) {
        let vw = entry.video_width;
        let vh = entry.video_height;
        println!("[SCRCPY-CTRL] using video dimensions {}x{}", vw, vh);
        match scrcpy_control::inject_tap(
            &mut entry.stream,
            x,
            y,
            source_width,
            source_height,
            vw,
            vh,
        ) {
            Ok(()) => {
                println!("[SCRCPY-CTRL] tap OK serial={}", serial);
                return Ok(CommandResult {
                    serial,
                    success: true,
                    message: String::new(),
                });
            }
            Err(e) => {
                println!("[SCRCPY-CTRL] tap failed serial={}: {}", serial, e);
                sockets.remove(&serial);
            }
        }
    } else {
        println!("[SCRCPY-CTRL] no control socket for serial={}", serial);
    }
    Ok(CommandResult {
        serial,
        success: false,
        message: "no control socket".into(),
    })
}

#[tauri::command]
async fn scrcpy_swipe(
    serial: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u32,
    source_width: u32,
    source_height: u32,
    _target_width: u32,
    _target_height: u32,
    _server_host: String,
    _server_port: u16,
    state: State<'_, AppState>,
) -> Result<CommandResult, String> {
    println!(
        "[SCRCPY-CTRL] swipe serial={} ({:.0},{:.0})->({:.0},{:.0}) dur={}ms",
        serial, x1, y1, x2, y2, duration_ms
    );
    let mut sockets = state.control_sockets.lock().unwrap();
    if let Some(entry) = sockets.get_mut(&serial) {
        let vw = entry.video_width;
        let vh = entry.video_height;
        match scrcpy_control::inject_swipe(
            &mut entry.stream,
            x1,
            y1,
            x2,
            y2,
            duration_ms,
            source_width,
            source_height,
            vw,
            vh,
        ) {
            Ok(()) => {
                return Ok(CommandResult {
                    serial,
                    success: true,
                    message: String::new(),
                })
            }
            Err(e) => {
                println!("[SCRCPY-CTRL] swipe failed serial={}: {}", serial, e);
                sockets.remove(&serial);
            }
        }
    }
    Ok(CommandResult {
        serial,
        success: false,
        message: "no control socket".into(),
    })
}

// ── scrcpy ───────────────────────────────────────────────────────────────────

#[tauri::command]
async fn launch_scrcpy(
    serial: String,
    server_host: String,
    server_port: u16,
) -> Result<(), String> {
    let is_remote = !(server_host == "127.0.0.1" || server_host == "localhost");
    tauri::async_runtime::spawn(async move {
        let mut cmd = tokio::process::Command::new("scrcpy");
        cmd.args(["-s", &serial]);
        if is_remote {
            cmd.env(
                "ADB_SERVER_SOCKET",
                format!("tcp:{}:{}", server_host, server_port),
            );
            cmd.args(["--tunnel-host", &server_host]);
        }
        let _ = cmd.status().await;
    });
    Ok(())
}

// ── Refresh devices ──────────────────────────────────────────────────────────

#[tauri::command]
async fn refresh_devices(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let servers = Arc::clone(&state.servers);
    tauri::async_runtime::spawn(poll_all_servers(servers, app));
    Ok(())
}

// ── Shell command ─────────────────────────────────────────────────────────────

#[tauri::command]
async fn run_shell_devices(
    serials: Vec<DeviceResolution>,
    cmd: String,
) -> Result<Vec<CommandResult>, String> {
    use adb::device::server_args;
    let handles: Vec<_> = serials
        .into_iter()
        .map(|d| {
            let cmd = cmd.clone();
            tokio::task::spawn_blocking(move || {
                let mut args = server_args(&d.server_host, d.server_port);
                args.extend(["-s".into(), d.serial.clone(), "shell".into()]);
                args.extend(cmd.split_whitespace().map(String::from));
                let out = std::process::Command::new("adb").args(&args).output();
                match out {
                    Ok(o) => CommandResult {
                        serial: d.serial.clone(),
                        success: o.status.success(),
                        message: String::from_utf8_lossy(&o.stdout).to_string()
                            + &String::from_utf8_lossy(&o.stderr),
                    },
                    Err(e) => CommandResult {
                        serial: d.serial.clone(),
                        success: false,
                        message: e.to_string(),
                    },
                }
            })
        })
        .collect();
    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        results.push(h.await.map_err(|e| e.to_string())?);
    }
    Ok(results)
}

// ── Config ───────────────────────────────────────────────────────────────────

#[tauri::command]
async fn load_config(state: State<'_, AppState>) -> Result<Vec<AdbServer>, String> {
    Ok(state.servers.lock().await.clone())
}

// ── App entry ────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Ensure adb/scrcpy are findable in bundled macOS app
    if let Ok(path) = std::env::var("PATH") {
        let extra = [
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "/Library/android/SDK/platform-tools",
            &format!(
                "{}/Library/Android/sdk/platform-tools",
                std::env::var("HOME").unwrap_or_default()
            ),
        ];
        let new_path = format!("{}:{}", extra.join(":"), path);
        std::env::set_var("PATH", new_path);
    }

    let servers_cfg = load_servers();
    let servers: Vec<AdbServer> = servers_cfg.iter().map(AdbServer::from_config).collect();
    let app_state = AppState::new(servers);

    let ws_hub = WsHub::default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
        .manage(ws_hub)
        .invoke_handler(tauri::generate_handler![
            add_server,
            remove_server,
            toggle_server,
            get_servers,
            start_preview,
            stop_preview,
            set_fps,
            start_stream,
            stop_stream,
            tap_devices,
            swipe_devices,
            scrcpy_tap,
            scrcpy_swipe,
            send_text_devices,
            keyevent_devices,
            set_usb_file_transfer_devices,
            wake_up_devices,
            launch_scrcpy,
            run_shell_devices,
            load_config,
            refresh_devices,
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = app.state::<AppState>();
            let servers = Arc::clone(&state.servers);

            // Start local WS server for high-frequency frames
            let hub = app.state::<WsHub>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = run_ws_server(hub, "127.0.0.1:32199".parse().unwrap()).await;
            });

            tauri::async_runtime::spawn(async move {
                loop {
                    poll_all_servers(Arc::clone(&servers), app_handle.clone()).await;
                    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
