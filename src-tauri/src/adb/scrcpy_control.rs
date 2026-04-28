use std::io::Write;
use std::net::TcpStream;

const INJECT_TOUCH_EVENT: u8 = 2;
const ACTION_DOWN: u8 = 0;
const ACTION_UP: u8 = 1;
const ACTION_MOVE: u8 = 2;
const POINTER_ID_FINGER: u64 = 0xFFFF_FFFF_FFFF_FFFE;
const PRESSURE_MAX: u16 = 0xFFFF;

pub(crate) fn build_touch_msg(
    action: u8,
    x: i32,
    y: i32,
    screen_w: u16,
    screen_h: u16,
    pressure: u16,
) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0] = INJECT_TOUCH_EVENT;
    buf[1] = action;
    buf[2..10].copy_from_slice(&POINTER_ID_FINGER.to_be_bytes());
    buf[10..14].copy_from_slice(&x.to_be_bytes());
    buf[14..18].copy_from_slice(&y.to_be_bytes());
    buf[18..20].copy_from_slice(&screen_w.to_be_bytes());
    buf[20..22].copy_from_slice(&screen_h.to_be_bytes());
    buf[22..24].copy_from_slice(&pressure.to_be_bytes());
    // action_button [24..28] = 0 (already zeroed)
    // buttons [28..32] = 0 (already zeroed)
    buf
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

pub fn build_touch_msg_scaled(
    action: u8,
    x: f64,
    y: f64,
    source_w: u32,
    source_h: u32,
    target_w: u32,
    target_h: u32,
) -> [u8; 32] {
    let tx = scale(x, source_w, target_w);
    let ty = scale(y, source_h, target_h);
    let pressure = if action == ACTION_UP { 0 } else { PRESSURE_MAX };
    build_touch_msg(action, tx, ty, target_w as u16, target_h as u16, pressure)
}

pub fn inject_tap(
    stream: &mut TcpStream,
    x: f64,
    y: f64,
    source_w: u32,
    source_h: u32,
    target_w: u32,
    target_h: u32,
) -> Result<(), String> {
    let tx = scale(x, source_w, target_w);
    let ty = scale(y, source_h, target_h);
    let w = target_w as u16;
    let h = target_h as u16;

    println!(
        "[SCRCPY-CTRL] inject_tap scaled=({},{}) screen={}x{} local={:?} peer={:?}",
        tx,
        ty,
        w,
        h,
        stream.local_addr().ok(),
        stream.peer_addr().ok()
    );

    let down = build_touch_msg(ACTION_DOWN, tx, ty, w, h, PRESSURE_MAX);
    let up = build_touch_msg(ACTION_UP, tx, ty, w, h, 0);

    let mut tap = [0u8; 64];
    tap[..32].copy_from_slice(&down);
    tap[32..].copy_from_slice(&up);

    stream
        .write_all(&tap)
        .map_err(|e| format!("control write failed: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("control flush failed: {e}"))?;

    // Verify socket is still alive by checking for errors
    match stream.take_error() {
        Ok(Some(e)) => {
            println!("[SCRCPY-CTRL] socket error after write: {}", e);
            return Err(format!("socket error: {e}"));
        }
        Ok(None) => {}
        Err(e) => {
            println!("[SCRCPY-CTRL] take_error failed: {}", e);
        }
    }

    Ok(())
}

pub fn inject_swipe(
    stream: &mut TcpStream,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    duration_ms: u32,
    source_w: u32,
    source_h: u32,
    target_w: u32,
    target_h: u32,
) -> Result<(), String> {
    let tx1 = scale(x1, source_w, target_w);
    let ty1 = scale(y1, source_h, target_h);
    let tx2 = scale(x2, source_w, target_w);
    let ty2 = scale(y2, source_h, target_h);
    let w = target_w as u16;
    let h = target_h as u16;

    let steps = 20u32.max(duration_ms / 16);
    let step_delay = std::time::Duration::from_millis((duration_ms as u64) / (steps as u64));

    let down = build_touch_msg(ACTION_DOWN, tx1, ty1, w, h, PRESSURE_MAX);
    stream
        .write_all(&down)
        .map_err(|e| format!("control write failed: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("control flush failed: {e}"))?;

    for i in 1..steps {
        let t = i as f64 / steps as f64;
        let mx = tx1 + ((tx2 - tx1) as f64 * t).round() as i32;
        let my = ty1 + ((ty2 - ty1) as f64 * t).round() as i32;
        let msg = build_touch_msg(ACTION_MOVE, mx, my, w, h, PRESSURE_MAX);
        stream
            .write_all(&msg)
            .map_err(|e| format!("control write failed: {e}"))?;
        stream
            .flush()
            .map_err(|e| format!("control flush failed: {e}"))?;
        std::thread::sleep(step_delay);
    }

    let up = build_touch_msg(ACTION_UP, tx2, ty2, w, h, 0);
    stream
        .write_all(&up)
        .map_err(|e| format!("control write failed: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("control flush failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_msg_is_32_bytes() {
        let msg = build_touch_msg(ACTION_DOWN, 100, 200, 1080, 1920, PRESSURE_MAX);
        assert_eq!(msg.len(), 32);
    }

    #[test]
    fn touch_msg_type_is_2() {
        let msg = build_touch_msg(ACTION_DOWN, 0, 0, 100, 100, 0);
        assert_eq!(msg[0], 2);
    }

    #[test]
    fn touch_msg_action_field() {
        assert_eq!(build_touch_msg(ACTION_DOWN, 0, 0, 1, 1, 0)[1], 0);
        assert_eq!(build_touch_msg(ACTION_UP, 0, 0, 1, 1, 0)[1], 1);
        assert_eq!(build_touch_msg(ACTION_MOVE, 0, 0, 1, 1, 0)[1], 2);
    }

    #[test]
    fn touch_msg_pointer_id_is_finger() {
        let msg = build_touch_msg(ACTION_DOWN, 0, 0, 1, 1, 0);
        let pid = u64::from_be_bytes(msg[2..10].try_into().unwrap());
        assert_eq!(pid, POINTER_ID_FINGER);
    }

    #[test]
    fn touch_msg_position_big_endian() {
        let msg = build_touch_msg(ACTION_DOWN, 540, 960, 1080, 1920, PRESSURE_MAX);
        let x = i32::from_be_bytes(msg[10..14].try_into().unwrap());
        let y = i32::from_be_bytes(msg[14..18].try_into().unwrap());
        assert_eq!(x, 540);
        assert_eq!(y, 960);
    }

    #[test]
    fn touch_msg_screen_size_big_endian() {
        let msg = build_touch_msg(ACTION_DOWN, 0, 0, 1080, 1920, 0);
        let w = u16::from_be_bytes(msg[18..20].try_into().unwrap());
        let h = u16::from_be_bytes(msg[20..22].try_into().unwrap());
        assert_eq!(w, 1080);
        assert_eq!(h, 1920);
    }

    #[test]
    fn touch_msg_pressure() {
        let msg = build_touch_msg(ACTION_DOWN, 0, 0, 1, 1, PRESSURE_MAX);
        let p = u16::from_be_bytes(msg[22..24].try_into().unwrap());
        assert_eq!(p, 0xFFFF);
    }

    #[test]
    fn touch_msg_buttons_are_zero() {
        let msg = build_touch_msg(ACTION_DOWN, 0, 0, 1, 1, 0);
        let action_button = u32::from_be_bytes(msg[24..28].try_into().unwrap());
        let buttons = u32::from_be_bytes(msg[28..32].try_into().unwrap());
        assert_eq!(action_button, 0);
        assert_eq!(buttons, 0);
    }

    #[test]
    fn scale_same_resolution() {
        assert_eq!(scale(100.0, 1080, 1080), 100);
    }

    #[test]
    fn scale_half_resolution() {
        assert_eq!(scale(100.0, 200, 1080), 540);
    }

    #[test]
    fn scale_zero_source() {
        assert_eq!(scale(123.0, 0, 1080), 123);
    }

    #[test]
    fn scale_clamps_to_target_bounds() {
        assert_eq!(scale(-10.0, 200, 1080), 0);
        assert_eq!(scale(200.0, 200, 1080), 1079);
    }
}
