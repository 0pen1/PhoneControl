use serde::{Deserialize, Serialize};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use super::device::server_args;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub serial: String,
    pub success: bool,
    pub message: String,
}

fn scale(value: f64, source_dim: u32, target_dim: u32) -> i32 {
    if target_dim == 0 {
        return value.round() as i32;
    }

    let scaled = if source_dim == 0 {
        value
    } else {
        (value / source_dim as f64) * target_dim as f64
    };
    let max = target_dim.saturating_sub(1) as f64;
    scaled.round().clamp(0.0, max) as i32
}

struct AdbOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_adb_once(args: &[String], timeout: Duration) -> Result<AdbOutput, String> {
    let mut child = Command::new("adb")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn adb: {}", e))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("Failed to collect adb output: {}", e))?;
                return Ok(AdbOutput {
                    status: output.status,
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("ADB command timeout (>{}ms)", timeout.as_millis()));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("Failed to wait for adb: {}", e)),
        }
    }
}

fn run_adb_device_once(
    host: &str,
    port: u16,
    serial: &str,
    shell_args: &[&str],
    timeout: Duration,
) -> Result<AdbOutput, String> {
    let mut args = server_args(host, port);
    args.extend(["-s".into(), serial.into(), "shell".into()]);
    args.extend(shell_args.iter().map(|s| s.to_string()));

    run_adb_once(&args, timeout)
}

fn run_adb_shell_script(
    host: &str,
    port: u16,
    serial: &str,
    script: &str,
    timeout: Duration,
) -> Result<AdbOutput, String> {
    let mut args = server_args(host, port);
    args.extend([
        "-s".into(),
        serial.into(),
        "shell".into(),
        "sh".into(),
        "-c".into(),
        script.into(),
    ]);

    run_adb_once(&args, timeout)
}

fn run_adb_device(host: &str, port: u16, serial: &str, shell_args: &[&str]) -> Result<(), String> {
    let start = Instant::now();
    let out = run_adb_device_once(host, port, serial, shell_args, Duration::from_secs(5))?;

    if !out.stdout.trim().is_empty() || !out.stderr.trim().is_empty() {
        println!(
            "[ADB] serial={} status={} out={:?} err={:?} ({:.0}ms)",
            serial,
            out.status,
            out.stdout.trim(),
            out.stderr.trim(),
            start.elapsed().as_millis()
        );
    }
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ADB command failed: status={} err={} ({:.0}ms)",
            out.status,
            out.stderr.trim(),
            start.elapsed().as_millis()
        ))
    }
}

fn get_state(host: &str, port: u16, serial: &str) -> Result<String, String> {
    let mut args = server_args(host, port);
    args.extend(["-s".into(), serial.into(), "get-state".into()]);
    let out = run_adb_once(&args, Duration::from_secs(2))?;
    if out.status.success() {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(out.stderr.trim().to_string())
    }
}

fn wait_for_device_online(host: &str, port: u16, serial: &str) -> bool {
    // A tap on Android's USB mode dialog can make the device disappear from ADB
    // shortly after `input tap` exits successfully. Probe after a small settle
    // delay, then wait for that device before moving the group operation on.
    std::thread::sleep(Duration::from_millis(180));

    let start = Instant::now();
    let timeout = Duration::from_secs(8);

    loop {
        let last_err = match get_state(host, port, serial) {
            Ok(state) if state == "device" => {
                let waited = start.elapsed().as_millis();
                if waited > 400 {
                    println!("[ADB-STATE] serial={} online after {}ms", serial, waited);
                }
                return true;
            }
            Ok(state) => format!("state={state}"),
            Err(e) => e,
        };

        if start.elapsed() >= timeout {
            println!(
                "[ADB-STATE] serial={} not online after {}ms: {}",
                serial,
                timeout.as_millis(),
                last_err
            );
            return false;
        }

        std::thread::sleep(Duration::from_millis(300));
    }
}

fn usb_props_show_mtp(output: &str) -> bool {
    output.lines().any(|line| {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        matches!(key.trim(), "sys.usb.config" | "sys.usb.state")
            && value.to_ascii_lowercase().contains("mtp")
    })
}

fn read_usb_props(host: &str, port: u16, serial: &str) -> Result<String, String> {
    let script = r#"
config="$(getprop sys.usb.config 2>/dev/null)"
state="$(getprop sys.usb.state 2>/dev/null)"
persist="$(getprop persist.sys.usb.config 2>/dev/null)"
echo "sys.usb.config=$config"
echo "sys.usb.state=$state"
echo "persist.sys.usb.config=$persist"
"#;

    let out = run_adb_shell_script(host, port, serial, script, Duration::from_secs(2))?;
    let stdout = out.stdout.trim();
    let stderr = out.stderr.trim();
    if out.status.success() {
        Ok(stdout.to_string())
    } else if stderr.is_empty() {
        Err(format!("get USB props failed: status={}", out.status))
    } else {
        Err(format!("get USB props failed: {}", stderr))
    }
}

pub fn verify_usb_file_transfer_after_tap(host: &str, port: u16, serial: &str) -> CommandResult {
    let start = Instant::now();
    let timeout = Duration::from_secs(18);
    let mut last_detail: String;

    println!(
        "[USB-VERIFY] serial={} waiting for USB file transfer state",
        serial
    );

    loop {
        match get_state(host, port, serial) {
            Ok(state) if state == "device" => match read_usb_props(host, port, serial) {
                Ok(props) => {
                    last_detail = props.replace('\n', "; ");
                    if usb_props_show_mtp(&props) {
                        println!(
                            "[USB-VERIFY] serial={} confirmed mtp after {}ms: {}",
                            serial,
                            start.elapsed().as_millis(),
                            last_detail
                        );
                        return CommandResult {
                            serial: serial.to_string(),
                            success: true,
                            message: format!("confirmed USB file transfer: {}", last_detail),
                        };
                    }
                }
                Err(e) => {
                    last_detail = e;
                }
            },
            Ok(state) => {
                last_detail = format!("adb state={state}");
            }
            Err(e) => {
                last_detail = e;
            }
        }

        if start.elapsed() >= timeout {
            println!(
                "[USB-VERIFY] serial={} not confirmed after {}ms: {}",
                serial,
                start.elapsed().as_millis(),
                last_detail
            );
            return CommandResult {
                serial: serial.to_string(),
                success: false,
                message: format!(
                    "tap sent, but USB file transfer was not confirmed: {}",
                    last_detail
                ),
            };
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn tap(
    host: &str,
    port: u16,
    serial: &str,
    x: f64,
    y: f64,
    source_w: u32,
    source_h: u32,
    target_w: u32,
    target_h: u32,
) -> CommandResult {
    let tx = scale(x, source_w, target_w);
    let ty = scale(y, source_h, target_h);
    let xs = tx.to_string();
    let ys = ty.to_string();
    let start = Instant::now();
    let result = run_adb_device(host, port, serial, &["input", "tap", &xs, &ys]);
    let stabilized = result
        .as_ref()
        .map(|_| wait_for_device_online(host, port, serial))
        .unwrap_or(false);
    println!(
        "[ADB-TAP] serial={} point=({}, {}) target={}x{} success={} stable={} ({:.0}ms)",
        serial,
        tx,
        ty,
        target_w,
        target_h,
        result.is_ok(),
        stabilized,
        start.elapsed().as_millis()
    );
    let success = result.is_ok() && stabilized;
    CommandResult {
        serial: serial.to_string(),
        success,
        message: result.err().unwrap_or_else(|| {
            if stabilized {
                String::new()
            } else {
                "tap sent, but device did not report online after input".to_string()
            }
        }),
    }
}

pub fn swipe(
    host: &str,
    port: u16,
    serial: &str,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u32,
    source_w: u32,
    source_h: u32,
    target_w: u32,
    target_h: u32,
) -> CommandResult {
    let tx1 = scale(x1, source_w, target_w).to_string();
    let ty1 = scale(y1, source_h, target_h).to_string();
    let tx2 = scale(x2, source_w, target_w).to_string();
    let ty2 = scale(y2, source_h, target_h).to_string();
    let dur = duration_ms.to_string();
    let start = Instant::now();
    let result = run_adb_device(
        host,
        port,
        serial,
        &["input", "swipe", &tx1, &ty1, &tx2, &ty2, &dur],
    );
    let stabilized = result
        .as_ref()
        .map(|_| wait_for_device_online(host, port, serial))
        .unwrap_or(false);
    println!(
        "[ADB-SWIPE] serial={} from=({}, {}) to=({}, {}) target={}x{} success={} stable={} ({:.0}ms)",
        serial,
        tx1,
        ty1,
        tx2,
        ty2,
        target_w,
        target_h,
        result.is_ok(),
        stabilized,
        start.elapsed().as_millis()
    );
    let success = result.is_ok() && stabilized;
    CommandResult {
        serial: serial.to_string(),
        success,
        message: result.err().unwrap_or_else(|| {
            if stabilized {
                String::new()
            } else {
                "swipe sent, but device did not report online after input".to_string()
            }
        }),
    }
}

pub fn send_text(host: &str, port: u16, serial: &str, text: &str) -> CommandResult {
    // Escape spaces for adb input text
    let escaped = text.replace(' ', "%s");
    let result = run_adb_device(host, port, serial, &["input", "text", &escaped]);
    CommandResult {
        serial: serial.to_string(),
        success: result.is_ok(),
        message: result.err().unwrap_or_default(),
    }
}

pub fn keyevent(host: &str, port: u16, serial: &str, keycode: u32) -> CommandResult {
    let kc = keycode.to_string();
    let result = run_adb_device(host, port, serial, &["input", "keyevent", &kc]);
    CommandResult {
        serial: serial.to_string(),
        success: result.is_ok(),
        message: result.err().unwrap_or_default(),
    }
}

pub fn set_usb_file_transfer(host: &str, port: u16, serial: &str) -> CommandResult {
    let script = r#"
set +e
for fn in mtp,adb mtp; do
  cmd usb set-functions "$fn" >/dev/null 2>&1
  sleep 0.8
  config="$(getprop sys.usb.config 2>/dev/null)"
  case "$config" in
    *mtp*) input keyevent 4 >/dev/null 2>&1; echo "sys.usb.config=$config"; exit 0 ;;
  esac

  svc usb setFunctions "$fn" true >/dev/null 2>&1
  sleep 0.8
  config="$(getprop sys.usb.config 2>/dev/null)"
  case "$config" in
    *mtp*) input keyevent 4 >/dev/null 2>&1; echo "sys.usb.config=$config"; exit 0 ;;
  esac

  svc usb setFunctions "$fn" >/dev/null 2>&1
  sleep 0.8
  config="$(getprop sys.usb.config 2>/dev/null)"
  case "$config" in
    *mtp*) input keyevent 4 >/dev/null 2>&1; echo "sys.usb.config=$config"; exit 0 ;;
  esac
done
config="$(getprop sys.usb.config 2>/dev/null)"
echo "failed to set USB file transfer; sys.usb.config=$config" >&2
exit 1
"#;

    let start = Instant::now();
    let result = run_adb_shell_script(host, port, serial, script, Duration::from_secs(8));
    match result {
        Ok(out) => {
            let stdout = out.stdout.trim();
            let stderr = out.stderr.trim();
            let message = if stdout.is_empty() {
                stderr.to_string()
            } else if stderr.is_empty() {
                stdout.to_string()
            } else {
                format!("{stdout}\n{stderr}")
            };
            println!(
                "[USB-MTP] serial={} success={} msg={:?} ({:.0}ms)",
                serial,
                out.status.success(),
                message,
                start.elapsed().as_millis()
            );
            CommandResult {
                serial: serial.to_string(),
                success: out.status.success(),
                message,
            }
        }
        Err(e) => {
            println!(
                "[USB-MTP] serial={} success=false err={} ({:.0}ms)",
                serial,
                e,
                start.elapsed().as_millis()
            );
            CommandResult {
                serial: serial.to_string(),
                success: false,
                message: e,
            }
        }
    }
}

pub fn wake_up_device(host: &str, port: u16, serial: &str) -> CommandResult {
    let mut check_args = server_args(host, port);
    check_args.extend([
        "-s".into(),
        serial.into(),
        "shell".into(),
        "dumpsys".into(),
        "power".into(),
    ]);

    let out = match Command::new("adb").args(&check_args).output() {
        Ok(out) => out,
        Err(e) => {
            return CommandResult {
                serial: serial.to_string(),
                success: false,
                message: e.to_string(),
            }
        }
    };

    if !out.status.success() {
        return CommandResult {
            serial: serial.to_string(),
            success: false,
            message: format!("Failed to check device state: {}", out.status),
        };
    }

    let output = String::from_utf8_lossy(&out.stdout);
    if output.contains("mWakefulness=Asleep") {
        let result = run_adb_device(host, port, serial, &["input", "keyevent", "26"]);
        CommandResult {
            serial: serial.to_string(),
            success: result.is_ok(),
            message: if result.is_ok() {
                "Device woken up".to_string()
            } else {
                result.err().unwrap_or_default()
            },
        }
    } else {
        CommandResult {
            serial: serial.to_string(),
            success: true,
            message: "Device already awake".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scale_same_resolution() {
        assert_eq!(scale(100.0, 1080, 1080), 100);
        assert_eq!(scale(540.0, 1080, 1080), 540);
    }

    #[test]
    fn test_scale_half_resolution() {
        // source 200px wide card, target 1080 device
        assert_eq!(scale(100.0, 200, 1080), 540);
        assert_eq!(scale(50.0, 200, 1080), 270);
    }

    #[test]
    fn test_scale_zero_source() {
        // zero source_dim → return value as-is
        assert_eq!(scale(123.0, 0, 1080), 123);
    }

    #[test]
    fn test_scale_clamps_to_target_bounds() {
        assert_eq!(scale(-10.0, 200, 1080), 0);
        assert_eq!(scale(200.0, 200, 1080), 1079);
    }

    #[test]
    fn test_scale_rounding() {
        // 1/3 of 1080 = 360
        assert_eq!(scale(1.0, 3, 1080), 360);
    }
}
