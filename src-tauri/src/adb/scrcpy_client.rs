use std::{
    collections::HashSet,
    io::{Read, Write},
    net::TcpStream,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use super::device::server_args;
use super::stream::StreamOptions;

/// Build the argv that starts the scrcpy server on the device.
/// Returns the tokens that should follow `adb -s SERIAL shell` — i.e. we
/// launch exactly like the official scrcpy CLI does:
///   `adb shell CLASSPATH=... app_process / com.genymobile.scrcpy.Server VER k=v k=v ...`
/// without an intermediate `sh -c` wrapper. Wrapping in `sh -c` has been
/// observed to suppress server stderr on some devices.
///
/// `tunnel_forward=true` tells the server to *listen* on the localabstract
/// socket (forward tunnel) instead of actively connecting back to the client
/// (reverse tunnel). Without this, the default `tunnel_forward=false` causes
/// the server to exit early when it cannot reach the client — which always
/// happens with remote ADB servers — leaving us with a forward-mapped port
/// that refuses connections.
pub(crate) fn build_start_argv(
    remote_path: &str,
    ver: &str,
    scid: u32,
    opts: &StreamOptions,
) -> Vec<String> {
    vec![
        format!("CLASSPATH={remote_path}"),
        "app_process".into(),
        "/".into(),
        "com.genymobile.scrcpy.Server".into(),
        ver.into(),
        format!("scid={scid:08x}"),
        "log_level=info".into(),
        "audio=false".into(),
        "control=true".into(),
        "tunnel_forward=true".into(),
        "stay_awake=true".into(),
        format!("max_size={}", opts.max_size),
        format!("max_fps={}", opts.max_fps),
        format!("video_bit_rate={}", opts.bit_rate),
        "video_codec_options=i-frame-interval=2".into(),
        "send_device_meta=false".into(),
        "send_codec_meta=true".into(),
        "send_dummy_byte=true".into(),
        "send_frame_meta=true".into(),
    ]
}

/// Single-string form, kept only for tests. Production code uses
/// `build_start_argv` directly.
#[cfg(test)]
pub(crate) fn build_start_cmd(
    remote_path: &str,
    ver: &str,
    scid: u32,
    opts: &StreamOptions,
) -> String {
    build_start_argv(remote_path, ver, scid, opts).join(" ")
}

/// Local TCP port for the `adb forward` used to reach the scrcpy server.
///
/// Range `32200..=39999` gives 7800 ports. Birthday-problem collision
/// probability at 100 devices ≈ 0.6%, vs ~50% with the old 800-port range.
pub(crate) fn scrcpy_local_port(serial: &str) -> u16 {
    32200 + (fxhash::hash64(serial) % 7800) as u16
}

/// Address to connect to after `adb forward tcp:PORT ...` succeeds.
///
/// `adb forward` binds the listening socket on the ADB **server's** host, not
/// on the client machine. So for a local ADB server (`127.0.0.1` / `localhost`)
/// we connect to `127.0.0.1:PORT`; for a remote ADB server we must connect to
/// `{remote_host}:PORT` over the network. Hardcoding `127.0.0.1` here was the
/// bug that surfaced as "tcp connect failed ... Connection refused" for
/// every device on a remote ADB setup.
pub(crate) fn forward_connect_addr(server_host: &str, local_port: u16) -> String {
    format!("{server_host}:{local_port}")
}

fn scrcpy_version() -> Result<String, String> {
    let out = std::process::Command::new("scrcpy")
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to run scrcpy --version: {e}"))?;
    let s = String::from_utf8_lossy(&out.stdout);
    // Output like: "scrcpy 3.2 <...>"
    let ver = s
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "failed to parse scrcpy version".to_string())?;
    Ok(ver.to_string())
}

fn scrcpy_server_installed_path() -> Result<String, String> {
    // scrcpy provides the server path via SCRCPY_SERVER_PATH env when built from source.
    // For standard installs, we can ask scrcpy to print logs at debug level and rely on
    // the known default locations.
    // Here we support Homebrew default plus SCRCPY_SERVER_PATH override.
    if let Ok(p) = std::env::var("SCRCPY_SERVER_PATH") {
        return Ok(p);
    }

    let candidates = [
        "/usr/local/opt/scrcpy/share/scrcpy/scrcpy-server",
        "/opt/homebrew/opt/scrcpy/share/scrcpy/scrcpy-server",
    ];
    for p in candidates {
        if std::path::Path::new(p).exists() {
            return Ok(p.to_string());
        }
    }

    Err("scrcpy-server path not found (set SCRCPY_SERVER_PATH)".into())
}

#[derive(Clone)]
struct ScrcpyRuntimeInfo {
    version: String,
    server_path: String,
    server_size: u64,
}

static SCRCPY_RUNTIME_INFO: OnceLock<Result<ScrcpyRuntimeInfo, String>> = OnceLock::new();

fn scrcpy_runtime_info() -> Result<ScrcpyRuntimeInfo, String> {
    SCRCPY_RUNTIME_INFO
        .get_or_init(|| {
            let version = scrcpy_version()?;
            let server_path = scrcpy_server_installed_path()?;
            let server_size = std::fs::metadata(&server_path)
                .map_err(|e| format!("scrcpy-server metadata failed: {e}"))?
                .len();
            if server_size == 0 {
                return Err("scrcpy-server file is empty".into());
            }
            println!(
                "[SCRCPY] runtime cached ver={} server={} size={}",
                version, server_path, server_size
            );
            Ok(ScrcpyRuntimeInfo {
                version,
                server_path,
                server_size,
            })
        })
        .clone()
}

fn parse_first_u64(s: &str) -> Option<u64> {
    s.split(|c: char| !c.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
}

static VERIFIED_REMOTE_SERVERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn verified_remote_servers() -> &'static Mutex<HashSet<String>> {
    VERIFIED_REMOTE_SERVERS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn remote_server_cache_key(
    host: &str,
    port: u16,
    serial: &str,
    remote_path: &str,
    size: u64,
) -> String {
    format!("{host}:{port}:{serial}:{remote_path}:{size}")
}

fn mark_remote_server_verified(key: &str) {
    if let Ok(mut cache) = verified_remote_servers().lock() {
        cache.insert(key.to_string());
    }
}

fn is_remote_server_verified(key: &str) -> bool {
    verified_remote_servers()
        .lock()
        .map(|cache| cache.contains(key))
        .unwrap_or(false)
}

fn clear_remote_server_verified(key: &str) {
    if let Ok(mut cache) = verified_remote_servers().lock() {
        cache.remove(key);
    }
}

/// Minimal scrcpy bootstrapper.
///
/// Phase 1 goal: establish a TCP connection to scrcpy server and read some bytes.
///
/// We intentionally do not implement the full protocol yet (device name, codec meta, control
/// channel, etc.). We use the official `scrcpy-server` artifact already shipped with the
/// installed scrcpy client, pushed to the device and started via `app_process`.
pub struct ScrcpyConnection {
    pub serial: String,
    pub local_port: u16,
    pub stream: TcpStream,
    pub control: Option<TcpStream>,
    pub server_child: std::process::Child,
    pub scid: u32,
}

fn run_adb(host: &str, port: u16, args: &[String]) -> Result<std::process::Output, String> {
    let mut full = server_args(host, port);
    full.extend_from_slice(args);
    std::process::Command::new("adb")
        .args(&full)
        .output()
        .map_err(|e| format!("adb spawn failed: {e}"))
}

pub fn remove_forward(host: &str, port: u16, serial: &str, local_port: u16) {
    let _ = run_adb(
        host,
        port,
        &[
            "-s".into(),
            serial.into(),
            "forward".into(),
            "--remove".into(),
            format!("tcp:{local_port}").into(),
        ],
    );
}

fn run_adb_spawn(host: &str, port: u16, args: &[String]) -> Result<std::process::Child, String> {
    let mut full = server_args(host, port);
    full.extend_from_slice(args);
    std::process::Command::new("adb")
        .args(&full)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("adb spawn failed: {e}"))
}

pub fn terminate_child(child: &mut std::process::Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_log_pump(serial: &str, mut reader: impl Read + Send + 'static, stream_name: &'static str) {
    let serial = serial.to_string();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut pending = Vec::<u8>::new();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            pending.extend_from_slice(&buf[..n]);
            while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                let line = pending.drain(..=pos).collect::<Vec<u8>>();
                let s = String::from_utf8_lossy(&line);
                print!("[SCRCPY-SERVER][{}][{}] {}", serial, stream_name, s);
            }
            if pending.len() > 1024 * 1024 {
                pending.clear();
            }
        }
        // Flush any trailing bytes that were not newline-terminated.
        // scrcpy crash output occasionally lacks a trailing \n and was
        // getting dropped silently, making failures look like clean exits.
        if !pending.is_empty() {
            let s = String::from_utf8_lossy(&pending);
            println!(
                "[SCRCPY-SERVER][{}][{}] (unterminated) {}",
                serial, stream_name, s
            );
        }
    });
}

fn adb_shell_check(host: &str, port: u16, serial: &str, cmd: &str) -> Result<String, String> {
    let out = run_adb(
        host,
        port,
        &[
            "-s".into(),
            serial.into(),
            "shell".into(),
            "sh".into(),
            "-c".into(),
            cmd.into(),
        ],
    )?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn start_scrcpy_and_connect(
    serial: &str,
    server_host: &str,
    server_port: u16,
    opts: &StreamOptions,
) -> Result<ScrcpyConnection, String> {
    let started_at = std::time::Instant::now();
    // 0) Determine local scrcpy metadata once per app process. Running
    // `scrcpy --version` and probing the server path for every phone adds a
    // serialized cost to large pages.
    let runtime = scrcpy_runtime_info()?;
    let ver = runtime.version;

    // 1) Push server to device only if not already present (saves ~1s).
    let remote_path = "/data/local/tmp/scrcpy-server.jar";
    let remote_cache_key = remote_server_cache_key(
        server_host,
        server_port,
        serial,
        remote_path,
        runtime.server_size,
    );

    if is_remote_server_verified(&remote_cache_key) {
        println!(
            "[SCRCPY] server cache hit serial={} ver={} elapsed={}ms",
            serial,
            ver,
            started_at.elapsed().as_millis()
        );
    } else {
        let remote_size_str = adb_shell_check(
            server_host,
            server_port,
            serial,
            &format!(
                "stat -c %s {0} 2>/dev/null || wc -c < {0} 2>/dev/null || echo 0",
                remote_path
            ),
        )
        .unwrap_or_else(|_| "0".to_string());
        let remote_size = parse_first_u64(&remote_size_str).unwrap_or(0);
        println!(
            "[SCRCPY] server size check serial={} local={} remote={} elapsed={}ms",
            serial,
            runtime.server_size,
            remote_size,
            started_at.elapsed().as_millis()
        );

        if runtime.server_size != remote_size {
            let out = run_adb(
                server_host,
                server_port,
                &[
                    "-s".into(),
                    serial.into(),
                    "push".into(),
                    runtime.server_path,
                    remote_path.into(),
                ],
            )?;
            if !out.status.success() {
                return Err(format!(
                    "adb push failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            println!(
                "[SCRCPY] server pushed serial={} remote={} ver={} elapsed={}ms",
                serial,
                remote_path,
                ver,
                started_at.elapsed().as_millis()
            );
        } else {
            println!(
                "[SCRCPY] server already on device serial={} ver={} elapsed={}ms",
                serial,
                ver,
                started_at.elapsed().as_millis()
            );
        }
        mark_remote_server_verified(&remote_cache_key);
    }

    // 2) Start scrcpy server on device.
    // scid identifies concurrent clients. Use random to avoid collision
    // with a previous server process that hasn't exited yet on the device.
    let scid = {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let mut h = RandomState::new().build_hasher();
        h.write(serial.as_bytes());
        (h.finish() as u32) & 0x7FFF_FFFF
    };
    // Note: args are key=value pairs, order irrelevant.
    // We disable audio and control (Phase 1).
    let start_argv = build_start_argv(remote_path, &ver, scid, opts);

    // IMPORTANT: Keep the server process alive.
    // Do NOT start it in the background with '&' then let adb exit immediately,
    // otherwise the server is killed when the shell session ends on many devices.
    //
    // Launch style matches the official scrcpy CLI: `adb shell CLASSPATH=... app_process / ...`
    // with individual argv tokens. A `sh -c "..."` wrapper was observed to
    // silence server stderr on some devices (PKG110 / OPPO).
    let mut argv: Vec<String> = vec!["-s".into(), serial.into(), "shell".into()];
    argv.extend(start_argv);
    let mut server_child = run_adb_spawn(server_host, server_port, &argv).map_err(|e| {
        clear_remote_server_verified(&remote_cache_key);
        e
    })?;

    println!(
        "[SCRCPY] server started serial={} scid={:08x} elapsed={}ms (adb shell kept alive)",
        serial,
        scid,
        started_at.elapsed().as_millis()
    );

    if let Some(stdout) = server_child.stdout.take() {
        spawn_log_pump(serial, stdout, "stdout");
    }
    if let Some(stderr) = server_child.stderr.take() {
        spawn_log_pump(serial, stderr, "stderr");
    }

    // 3) Set up the forward tunnel.
    //
    // We always use forward tunnel (server listens on the localabstract socket,
    // client connects via `adb forward`). This is the only mode that works for
    // remote ADB servers, and it also works fine for local ADB. Using a single
    // mode avoids the reverse/forward state machine and the associated races.
    // The server was started with `tunnel_forward=true` to match.
    let socket_name = format!("localabstract:scrcpy_{:08x}", scid);

    // Use tcp:0 to let ADB pick a free port, avoiding "Address already in use"
    let out = run_adb(
        server_host,
        server_port,
        &[
            "-s".into(),
            serial.into(),
            "forward".into(),
            "tcp:0".into(),
            socket_name.clone().into(),
        ],
    )?;
    if !out.status.success() {
        terminate_child(&mut server_child);
        clear_remote_server_verified(&remote_cache_key);
        return Err(format!(
            "adb forward failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    // adb forward tcp:0 prints the allocated port on stdout
    let actual_port: u16 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|e| format!("failed to parse allocated port: {e}"))?;
    let addr = forward_connect_addr(server_host, actual_port);

    println!(
        "[SCRCPY] tunnel forward serial={} port={} scid={:08x} adb={}:{} elapsed={}ms",
        serial,
        actual_port,
        scid,
        server_host,
        server_port,
        started_at.elapsed().as_millis()
    );

    // 4) Establish TCP connection with retry.
    //
    // The server may still be warming up (remote ADB can take a few hundred ms),
    // so we retry for up to 8 seconds. With `adb forward`, the TCP connect can
    // succeed immediately but the server hasn't bound the abstract socket yet,
    // causing an instant EOF. We detect this by peeking 1 byte — if it returns
    // 0 (EOF) we reconnect.
    let stream = {
        let mut last_err: Option<String>;
        let start = std::time::Instant::now();
        loop {
            match TcpStream::connect(&addr) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_millis(300))).ok();
                    let mut peek = [0u8; 1];
                    match s.peek(&mut peek) {
                        Ok(0) => {
                            // Immediate EOF — server not ready yet
                            last_err = Some("immediate EOF (server not ready)".into());
                        }
                        Ok(_) => break s,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            // Timeout on peek means the connection is alive but no data yet — good
                            break s;
                        }
                        Err(e) => {
                            last_err = Some(format!("peek failed: {e}"));
                        }
                    }
                }
                Err(e) => {
                    last_err = Some(e.to_string());
                }
            }
            if start.elapsed() > Duration::from_secs(3) {
                terminate_child(&mut server_child);
                remove_forward(server_host, server_port, serial, actual_port);
                clear_remote_server_verified(&remote_cache_key);
                return Err(format!(
                    "tcp connect failed after retries: {}",
                    last_err.unwrap_or_else(|| "unknown".into())
                ));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    };

    println!(
        "[SCRCPY] tcp connected serial={} port={} elapsed={}ms",
        serial,
        actual_port,
        started_at.elapsed().as_millis()
    );

    stream.set_nodelay(true).ok();
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();

    // 5) Connect control socket (second accept on the same abstract socket).
    //
    // The server blocks on controlSocket = localServerSocket.accept() right
    // after the video accept. The retry loop handles the short race where
    // adb-forward accepts before the device-side control socket is ready.
    println!(
        "[SCRCPY] attempting control socket connect serial={} addr={}",
        serial, addr
    );
    let control = {
        let mut last_err: Option<String>;
        let start = std::time::Instant::now();
        let mut ctrl: Option<TcpStream> = None;
        loop {
            match TcpStream::connect(&addr) {
                Ok(s) => {
                    // The control socket does NOT receive a dummy byte —
                    // only the first accepted socket (video) does. Just
                    // verify the connection is alive; EOF = server hasn't
                    // accepted yet.
                    s.set_read_timeout(Some(Duration::from_millis(500))).ok();
                    let mut peek = [0u8; 1];
                    match s.peek(&mut peek) {
                        Ok(0) => {
                            last_err = Some("control: immediate EOF".into());
                        }
                        _ => {
                            // Any non-EOF (data, timeout, WouldBlock) means
                            // the connection is alive. Do NOT read anything.
                            ctrl = Some(s);
                            break;
                        }
                    }
                }
                Err(e) => {
                    last_err = Some(e.to_string());
                }
            }
            if start.elapsed() > Duration::from_secs(3) {
                println!(
                    "[SCRCPY] control socket failed serial={}: {} (video-only mode)",
                    serial,
                    last_err.unwrap_or_else(|| "unknown".into())
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        ctrl
    };

    if let Some(ref c) = control {
        c.set_nodelay(true).ok();
        c.set_write_timeout(Some(Duration::from_secs(2))).ok();
        let local = c.local_addr().ok();
        let peer = c.peer_addr().ok();
        println!(
            "[SCRCPY] control socket connected serial={} local={:?} peer={:?} elapsed={}ms",
            serial,
            local,
            peer,
            started_at.elapsed().as_millis()
        );
    }

    // Keep the forward mapping for the whole scrcpy session, like the native
    // scrcpy client does. Removing it immediately leaves active connections
    // relying only on existing asocket pairs; with a remote ADB server and many
    // devices, batch control writes have been observed to make those sessions
    // EOF before the injected event is visible.
    println!(
        "[SCRCPY] forward listener kept serial={} port={} for active session",
        serial, actual_port
    );

    Ok(ScrcpyConnection {
        serial: serial.to_string(),
        local_port: actual_port,
        stream,
        control,
        server_child,
        scid,
    })
}

impl ScrcpyConnection {
    pub fn read_some(&mut self, max: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; max];
        let n = self
            .stream
            .read(&mut buf)
            .map_err(|e| format!("tcp read failed: {e}"))?;
        buf.truncate(n);
        Ok(buf)
    }

    pub fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(data)
            .map_err(|e| format!("tcp write failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_start_cmd_includes_options() {
        let opts = StreamOptions {
            max_size: 1080,
            max_fps: 60,
            bit_rate: 8_000_000,
        };
        let cmd = build_start_cmd("/data/local/tmp/scrcpy-server.jar", "3.2", 0xabcd, &opts);
        assert!(cmd.contains("max_size=1080"), "cmd={cmd}");
        assert!(cmd.contains("max_fps=60"), "cmd={cmd}");
        assert!(cmd.contains("video_bit_rate=8000000"), "cmd={cmd}");
        assert!(cmd.contains("scid=0000abcd"), "cmd={cmd}");
        assert!(
            cmd.contains("CLASSPATH=/data/local/tmp/scrcpy-server.jar"),
            "cmd={cmd}"
        );
        assert!(
            cmd.contains("com.genymobile.scrcpy.Server 3.2"),
            "cmd={cmd}"
        );
    }

    #[test]
    fn build_start_cmd_defaults() {
        let opts = StreamOptions::default();
        let cmd = build_start_cmd("/x.jar", "3.2", 1, &opts);
        assert!(cmd.contains("max_size=720"));
        assert!(cmd.contains("max_fps=30"));
        assert!(cmd.contains("video_bit_rate=4000000"));
        assert!(cmd.contains("audio=false"));
        assert!(cmd.contains("control=true"));
        assert!(cmd.contains("send_frame_meta=true"));
    }

    #[test]
    fn build_start_cmd_scid_padded_hex() {
        let opts = StreamOptions::default();
        // scid 0x1 should render as 8-char zero-padded hex
        let cmd = build_start_cmd("/x.jar", "3.2", 1, &opts);
        assert!(cmd.contains("scid=00000001"), "cmd={cmd}");
    }

    #[test]
    fn build_start_cmd_has_tunnel_forward() {
        // Missing tunnel_forward=true makes the server default to reverse
        // tunnel, which exits immediately for remote ADB. Regression guard.
        let cmd = build_start_cmd("/x.jar", "3.2", 1, &StreamOptions::default());
        assert!(cmd.contains("tunnel_forward=true"), "cmd={cmd}");
    }

    #[test]
    fn scrcpy_local_port_avoids_ws_server_port() {
        // WS server listens on 127.0.0.1:32199. Forward port must never land
        // on it, regardless of the serial.
        for serial in ["", "a", "3B65BQ01MW300000", "device-测试", "emulator-5554"] {
            let p = scrcpy_local_port(serial);
            assert_ne!(p, 32199, "serial={serial} -> port {p} collides with WS");
            assert!(
                (32200..=39999).contains(&p),
                "serial={serial} -> port {p} out of range"
            );
        }
    }

    #[test]
    fn scrcpy_local_port_is_deterministic() {
        // Start/cleanup paths compute the port independently; they must agree.
        let s = "3B65BQ01MW300000";
        assert_eq!(scrcpy_local_port(s), scrcpy_local_port(s));
    }

    #[test]
    fn forward_connect_addr_uses_server_host() {
        // For a remote ADB server, `adb forward` listens on the REMOTE host.
        // Connecting to 127.0.0.1 here was the "Connection refused" bug.
        assert_eq!(
            forward_connect_addr("192.168.0.136", 32278),
            "192.168.0.136:32278"
        );
        assert_eq!(forward_connect_addr("10.0.0.5", 32200), "10.0.0.5:32200");
    }

    #[test]
    fn forward_connect_addr_local_adb() {
        // Local ADB path still works: the server host IS 127.0.0.1.
        assert_eq!(forward_connect_addr("127.0.0.1", 32250), "127.0.0.1:32250");
        assert_eq!(forward_connect_addr("localhost", 32250), "localhost:32250");
    }

    #[test]
    fn build_start_argv_is_individual_tokens() {
        // scrcpy CLI uses individual argv tokens — NOT `sh -c "..."`. The
        // sh -c wrapper was observed to swallow server stderr on OPPO PKG110.
        // Each argument must stand alone so `adb shell` gets them as separate
        // args, the way the CLI does.
        let argv = build_start_argv(
            "/data/local/tmp/scrcpy-server.jar",
            "3.2",
            0xabcd,
            &StreamOptions::default(),
        );

        // Must NOT contain spaces in any single token (would indicate a joined blob).
        // Note: some scrcpy key=value args legitimately contain multiple '=' signs
        // (e.g. "video_codec_options=i-frame-interval=2"), so we only check for spaces.
        for token in &argv {
            assert!(
                !token.contains(' '),
                "token {token:?} contains a space — still a joined blob"
            );
        }

        // Required tokens, in argv form.
        assert_eq!(argv[0], "CLASSPATH=/data/local/tmp/scrcpy-server.jar");
        assert_eq!(argv[1], "app_process");
        assert_eq!(argv[2], "/");
        assert_eq!(argv[3], "com.genymobile.scrcpy.Server");
        assert_eq!(argv[4], "3.2");
        assert!(argv.iter().any(|a| a == "scid=0000abcd"));
        assert!(argv.iter().any(|a| a == "tunnel_forward=true"));
        assert!(argv.iter().any(|a| a == "audio=false"));
        assert!(argv.iter().any(|a| a == "control=true"));
        assert!(argv.iter().any(|a| a == "stay_awake=true"));
    }

    #[test]
    fn build_start_argv_threads_stream_options() {
        let opts = StreamOptions {
            max_size: 1080,
            max_fps: 60,
            bit_rate: 8_000_000,
        };
        let argv = build_start_argv("/x.jar", "3.2", 1, &opts);
        assert!(argv.iter().any(|a| a == "max_size=1080"));
        assert!(argv.iter().any(|a| a == "max_fps=60"));
        assert!(argv.iter().any(|a| a == "video_bit_rate=8000000"));
    }
}
