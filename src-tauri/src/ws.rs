use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{accept_async, tungstenite::Message};

/// Wire format:
/// - binary message
/// - first line (utf8): "serial=<SERIAL>\nwidth=<W>\nheight=<H>\nformat=rgba\n\n"
/// - then raw RGBA bytes (W*H*4)
pub type FrameSender = mpsc::UnboundedSender<Bytes>;

#[derive(Clone, Default)]
pub struct WsHub {
    inner: Arc<Mutex<HashMap<String, Vec<(usize, FrameSender)>>>>,
    next_id: Arc<std::sync::atomic::AtomicUsize>,
}

impl WsHub {
    pub fn pack_jpeg_frame(serial: &str, width: u32, height: u32, jpeg: &[u8]) -> Vec<u8> {
        // v2 frame format (JPEG payload instead of raw RGBA):
        // [u8 version=2]
        // [u16 serial_len_be]
        // [serial bytes]
        // [u32 width_be]
        // [u32 height_be]
        // [jpeg bytes]
        let serial_bytes = serial.as_bytes();
        let serial_len: u16 = serial_bytes.len().try_into().unwrap_or(u16::MAX);

        let mut out = Vec::with_capacity(1 + 2 + serial_bytes.len() + 4 + 4 + jpeg.len());
        out.push(2);
        out.extend_from_slice(&serial_len.to_be_bytes());
        out.extend_from_slice(serial_bytes);
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(jpeg);
        out
    }

    /// v3 frame format — raw H.264 NAL data for WebCodecs decoding.
    ///
    /// `packet_type`: 0 = config (SPS/PPS), 1 = keyframe, 2 = delta.
    pub fn pack_h264_frame(
        serial: &str,
        packet_type: u8,
        pts: u64,
        width: u32,
        height: u32,
        nal_data: &[u8],
    ) -> Vec<u8> {
        let serial_bytes = serial.as_bytes();
        let serial_len: u16 = serial_bytes.len().try_into().unwrap_or(u16::MAX);

        let mut out =
            Vec::with_capacity(1 + 2 + serial_bytes.len() + 1 + 8 + 4 + 4 + nal_data.len());
        out.push(3); // version
        out.extend_from_slice(&serial_len.to_be_bytes());
        out.extend_from_slice(serial_bytes);
        out.push(packet_type);
        out.extend_from_slice(&pts.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(nal_data);
        out
    }

    pub fn broadcast(&self, serial: &str, bytes: Vec<u8>) {
        let mut map = self.inner.lock().unwrap();
        let Some(list) = map.get_mut(serial) else {
            return;
        };
        let shared: Bytes = bytes.into();
        list.retain(|(_id, tx)| tx.send(shared.clone()).is_ok());
    }
}

pub async fn run_ws_server(hub: WsHub, addr: SocketAddr) -> Result<(), String> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("ws bind failed: {e}"))?;

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("ws accept failed: {e}"))?;
        stream.set_nodelay(true).ok();
        let hub = hub.clone();
        tokio::spawn(async move {
            let ws = match accept_async(stream).await {
                Ok(ws) => ws,
                Err(_) => return,
            };
            let (mut write, mut read) = ws.split();

            // One connection can subscribe to multiple devices.
            let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Bytes>();
            let out_task = tokio::spawn(async move {
                while let Some(frame) = out_rx.recv().await {
                    if write.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
            });

            // Track active subscriptions on this connection: serial -> subscription id.
            let mut subscribed: HashMap<String, usize> = HashMap::new();

            while let Some(Ok(msg)) = read.next().await {
                match msg {
                    Message::Text(t) => {
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
                            continue;
                        };
                        let Some(typ) = v.get("type").and_then(|x| x.as_str()) else {
                            continue;
                        };
                        let Some(serial) = v.get("serial").and_then(|x| x.as_str()) else {
                            continue;
                        };

                        match typ {
                            "subscribe" => {
                                if !subscribed.contains_key(serial) {
                                    let id = hub
                                        .next_id
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    let mut map = hub.inner.lock().unwrap();
                                    map.entry(serial.to_string())
                                        .or_default()
                                        .push((id, out_tx.clone()));
                                    subscribed.insert(serial.to_string(), id);
                                }
                            }
                            "unsubscribe" => {
                                if let Some(id) = subscribed.remove(serial) {
                                    let mut map = hub.inner.lock().unwrap();
                                    if let Some(list) = map.get_mut(serial) {
                                        list.retain(|(sid, _)| *sid != id);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }

            // Clean up all subscriptions for this connection
            {
                let mut map = hub.inner.lock().unwrap();
                for (serial, id) in &subscribed {
                    if let Some(list) = map.get_mut(serial) {
                        list.retain(|(sid, _)| *sid != *id);
                    }
                }
            }

            drop(out_tx);
            let _ = out_task.await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_v2(frame: &[u8]) -> (String, u32, u32, &[u8]) {
        assert_eq!(frame[0], 2, "version");
        let serial_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
        let mut off = 3;
        let serial = std::str::from_utf8(&frame[off..off + serial_len])
            .unwrap()
            .to_string();
        off += serial_len;
        let width = u32::from_be_bytes(frame[off..off + 4].try_into().unwrap());
        off += 4;
        let height = u32::from_be_bytes(frame[off..off + 4].try_into().unwrap());
        off += 4;
        (serial, width, height, &frame[off..])
    }

    #[test]
    fn pack_jpeg_frame_header_fields() {
        let jpeg = vec![0xFFu8, 0xD8, 0xFF, 0xE0]; // fake JPEG header
        let packed = WsHub::pack_jpeg_frame("abc", 2, 3, &jpeg);
        let (serial, w, h, payload) = parse_v2(&packed);
        assert_eq!(serial, "abc");
        assert_eq!(w, 2);
        assert_eq!(h, 3);
        assert_eq!(payload, jpeg.as_slice());
    }

    #[test]
    fn pack_jpeg_frame_preserves_bytes() {
        let jpeg: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let packed = WsHub::pack_jpeg_frame("dev-1234", 640, 480, &jpeg);
        let (_s, _w, _h, payload) = parse_v2(&packed);
        assert_eq!(payload.len(), jpeg.len());
        assert_eq!(payload, jpeg.as_slice());
    }

    #[test]
    fn pack_jpeg_frame_big_endian_dimensions() {
        let packed = WsHub::pack_jpeg_frame("x", 256, 1, &[0xFFu8, 0xD8]);
        // header: [2, 0, 1, 'x', 0, 0, 1, 0,   0, 0, 0, 1, ...]
        assert_eq!(&packed[4..8], &[0, 0, 1, 0]);
        assert_eq!(&packed[8..12], &[0, 0, 0, 1]);
    }

    fn parse_v3(frame: &[u8]) -> (String, u8, u64, u32, u32, &[u8]) {
        assert_eq!(frame[0], 3, "version");
        let serial_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
        let mut off = 3;
        let serial = std::str::from_utf8(&frame[off..off + serial_len])
            .unwrap()
            .to_string();
        off += serial_len;
        let packet_type = frame[off];
        off += 1;
        let pts = u64::from_be_bytes(frame[off..off + 8].try_into().unwrap());
        off += 8;
        let width = u32::from_be_bytes(frame[off..off + 4].try_into().unwrap());
        off += 4;
        let height = u32::from_be_bytes(frame[off..off + 4].try_into().unwrap());
        off += 4;
        (serial, packet_type, pts, width, height, &frame[off..])
    }

    #[test]
    fn pack_h264_frame_header_fields() {
        let nal = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42];
        let packed = WsHub::pack_h264_frame("dev1", 1, 12345, 320, 720, &nal);
        let (serial, ptype, pts, w, h, payload) = parse_v3(&packed);
        assert_eq!(serial, "dev1");
        assert_eq!(ptype, 1);
        assert_eq!(pts, 12345);
        assert_eq!(w, 320);
        assert_eq!(h, 720);
        assert_eq!(payload, nal.as_slice());
    }

    #[test]
    fn pack_h264_frame_config_type() {
        let packed = WsHub::pack_h264_frame("s", 0, 0, 1080, 1920, &[0xFF]);
        let (_, ptype, _, _, _, _) = parse_v3(&packed);
        assert_eq!(ptype, 0);
    }

    #[test]
    fn hub_broadcast_drops_closed_receivers() {
        let hub = WsHub::default();
        let (tx1, mut rx1) = mpsc::unbounded_channel::<Bytes>();
        let (tx2, rx2) = mpsc::unbounded_channel::<Bytes>();
        {
            let mut map = hub.inner.lock().unwrap();
            map.entry("s1".into()).or_default().push((0, tx1));
            map.entry("s1".into()).or_default().push((1, tx2));
        }
        // Close receiver 2 — its send should fail and it should be retained only if ok.
        drop(rx2);
        hub.broadcast("s1", vec![1, 2, 3]);
        // rx1 still gets the frame
        let got = rx1.try_recv().unwrap();
        assert_eq!(got.as_ref(), &[1u8, 2, 3]);
        // Hub should have dropped the dead sender
        let map = hub.inner.lock().unwrap();
        assert_eq!(map.get("s1").unwrap().len(), 1);
    }

    #[test]
    fn hub_broadcast_to_unknown_serial_is_noop() {
        let hub = WsHub::default();
        // Must not panic when no subscribers.
        hub.broadcast("nobody", vec![1]);
    }
}
