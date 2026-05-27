use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::TcpListener,
    sync::mpsc::{self, error::TrySendError},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const WS_FRAME_QUEUE_CAPACITY: usize = 16;

pub type FrameSender = mpsc::Sender<Bytes>;

#[derive(Clone, Default)]
pub struct WsHub {
    inner: Arc<Mutex<HashMap<String, Vec<(usize, FrameSender)>>>>,
    next_id: Arc<std::sync::atomic::AtomicUsize>,
}

impl WsHub {
    /// v4 frame format — raw H.264 NAL data for WebCodecs decoding.
    ///
    /// `packet_type`: 0 = config (SPS/PPS), 1 = keyframe, 2 = delta.
    /// `seq` lets the browser detect dropped packets and wait for a keyframe.
    pub fn pack_h264_frame(
        serial: &str,
        packet_type: u8,
        seq: u64,
        pts: u64,
        width: u32,
        height: u32,
        nal_data: &[u8],
    ) -> Vec<u8> {
        let serial_bytes = serial.as_bytes();
        let serial_len: u16 = serial_bytes.len().try_into().unwrap_or(u16::MAX);

        let mut out =
            Vec::with_capacity(1 + 2 + serial_bytes.len() + 1 + 8 + 8 + 4 + 4 + nal_data.len());
        out.push(4); // version
        out.extend_from_slice(&serial_len.to_be_bytes());
        out.extend_from_slice(serial_bytes);
        out.push(packet_type);
        out.extend_from_slice(&seq.to_be_bytes());
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
        list.retain(|(_id, tx)| match tx.try_send(shared.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                // Realtime video must not build latency. If the browser is
                // behind, drop this frame and keep the subscription alive.
                true
            }
            Err(TrySendError::Closed(_)) => false,
        });
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
            let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(WS_FRAME_QUEUE_CAPACITY);
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

    fn parse_v4(frame: &[u8]) -> (String, u8, u64, u64, u32, u32, &[u8]) {
        assert_eq!(frame[0], 4, "version");
        let serial_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
        let mut off = 3;
        let serial = std::str::from_utf8(&frame[off..off + serial_len])
            .unwrap()
            .to_string();
        off += serial_len;
        let packet_type = frame[off];
        off += 1;
        let seq = u64::from_be_bytes(frame[off..off + 8].try_into().unwrap());
        off += 8;
        let pts = u64::from_be_bytes(frame[off..off + 8].try_into().unwrap());
        off += 8;
        let width = u32::from_be_bytes(frame[off..off + 4].try_into().unwrap());
        off += 4;
        let height = u32::from_be_bytes(frame[off..off + 4].try_into().unwrap());
        off += 4;
        (serial, packet_type, seq, pts, width, height, &frame[off..])
    }

    #[test]
    fn pack_h264_frame_header_fields() {
        let nal = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42];
        let packed = WsHub::pack_h264_frame("dev1", 1, 7, 12345, 320, 720, &nal);
        let (serial, ptype, seq, pts, w, h, payload) = parse_v4(&packed);
        assert_eq!(serial, "dev1");
        assert_eq!(ptype, 1);
        assert_eq!(seq, 7);
        assert_eq!(pts, 12345);
        assert_eq!(w, 320);
        assert_eq!(h, 720);
        assert_eq!(payload, nal.as_slice());
    }

    #[test]
    fn pack_h264_frame_config_type() {
        let packed = WsHub::pack_h264_frame("s", 0, 0, 0, 1080, 1920, &[0xFF]);
        let (_, ptype, _, _, _, _, _) = parse_v4(&packed);
        assert_eq!(ptype, 0);
    }

    #[test]
    fn hub_broadcast_drops_closed_receivers() {
        let hub = WsHub::default();
        let (tx1, mut rx1) = mpsc::channel::<Bytes>(1);
        let (tx2, rx2) = mpsc::channel::<Bytes>(1);
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
    fn hub_broadcast_drops_frames_when_receiver_is_full() {
        let hub = WsHub::default();
        let (tx, mut rx) = mpsc::channel::<Bytes>(1);
        {
            let mut map = hub.inner.lock().unwrap();
            map.entry("s1".into()).or_default().push((0, tx));
        }

        hub.broadcast("s1", vec![1]);
        hub.broadcast("s1", vec![2]);

        let got = rx.try_recv().unwrap();
        assert_eq!(got.as_ref(), &[1u8]);
        assert!(rx.try_recv().is_err());

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
