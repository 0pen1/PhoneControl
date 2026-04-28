import { useEffect, useMemo, useRef } from 'react';
import { useStore } from '../store';
import { getCanvas } from '../utils/canvasRegistry';
import {
  parseSPSCodecString,
  buildAvcC,
  annexBToAvc,
  extractSPSPPS,
} from '../utils/h264Utils';

export const WS_URL = 'ws://127.0.0.1:32199';
const MAX_RECONNECT_DELAY_MS = 5000;
const BASE_RECONNECT_DELAY_MS = 200;

interface DecoderState {
  decoder: VideoDecoder;
  configured: boolean;
  ctx: CanvasRenderingContext2D | null;
  lastWidth: number;
  lastHeight: number;
  lastConfig: VideoDecoderConfig | null;
  waitingForKeyframe: boolean;
  pendingFrame: VideoFrame | null;
}

interface ParsedV3Frame {
  serial: string;
  packetType: number; // 0=config, 1=key, 2=delta
  pts: bigint;
  width: number;
  height: number;
  nalData: Uint8Array;
}

export function parseV3Frame(buf: ArrayBuffer): ParsedV3Frame {
  const view = new DataView(buf);
  let off = 0;
  const version = view.getUint8(off); off += 1;
  if (version !== 3) throw new Error(`Unsupported frame version: ${version}`);
  const serialLen = view.getUint16(off, false); off += 2;
  const serialBytes = new Uint8Array(buf, off, serialLen); off += serialLen;
  const serial = new TextDecoder().decode(serialBytes);
  const packetType = view.getUint8(off); off += 1;
  const pts = view.getBigUint64(off, false); off += 8;
  const width = view.getUint32(off, false); off += 4;
  const height = view.getUint32(off, false); off += 4;
  const nalData = new Uint8Array(buf, off);
  return { serial, packetType, pts, width, height, nalData };
}

export function useStream() {
  const setStreamFrame = useStore((s) => s.setStreamFrame);
  const clearStreamFrame = useStore((s) => s.clearStreamFrame);
  const setStreamStatus = useStore((s) => s.setStreamStatus);
  const devices = useStore((s) => s.devices);
  const disabledSerials = useStore((s) => s.disabledSerials);
  const page = useStore((s) => s.page);
  const pageSize = useStore((s) => s.pageSize);
  const overviewMode = useStore((s) => s.overviewMode);

  const desired = useMemo(() => {
    const enabled = devices.filter(
      (d) => d.status === 'online' && !disabledSerials.has(d.serial),
    );
    const visible = overviewMode
      ? enabled
      : enabled.slice(page * pageSize, (page + 1) * pageSize);
    return new Set(visible.map((d) => d.serial));
  }, [devices, disabledSerials, page, pageSize, overviewMode]);

  const desiredRef = useRef<Set<string>>(desired);
  const subscribedRef = useRef<Set<string>>(new Set());
  const wsRef = useRef<WebSocket | null>(null);
  const decodersRef = useRef<Map<string, DecoderState>>(new Map());

  useEffect(() => {
    desiredRef.current = desired;
    reconcile(wsRef.current, subscribedRef.current, desired);
    for (const [serial, state] of decodersRef.current) {
      if (!desired.has(serial)) {
        if (state.pendingFrame) { state.pendingFrame.close(); state.pendingFrame = null; }
        if (state.decoder.state !== 'closed') {
          try { state.decoder.close(); } catch { /* noop */ }
        }
        clearStreamFrame(serial);
        decodersRef.current.delete(serial);
      }
    }
  }, [desired, clearStreamFrame]);

  useEffect(() => {
    let cancelled = false;
    let attempt = 0;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let rafId: number | null = null;
    const decoders = decodersRef.current;

    // ---- rAF render loop: only draw the latest frame per device ----
    function renderLoop() {
      for (const [serial, state] of decoders) {
        const frame = state.pendingFrame;
        if (!frame) continue;
        state.pendingFrame = null;

        const canvas = getCanvas(serial);
        if (canvas) {
          if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
            canvas.width = frame.displayWidth;
            canvas.height = frame.displayHeight;
            state.ctx = canvas.getContext('2d');
          }
          if (!state.ctx) state.ctx = canvas.getContext('2d');
          if (state.ctx) state.ctx.drawImage(frame, 0, 0);
        }
        const frameMissing = !useStore.getState().streamFrames[serial];
        if (
          frameMissing ||
          state.lastWidth !== frame.displayWidth ||
          state.lastHeight !== frame.displayHeight
        ) {
          state.lastWidth = frame.displayWidth;
          state.lastHeight = frame.displayHeight;
          setStreamFrame(serial, frame.displayWidth, frame.displayHeight);
        }
        frame.close();
      }
      if (!cancelled) rafId = requestAnimationFrame(renderLoop);
    }
    rafId = requestAnimationFrame(renderLoop);

    // ---- decoder management ----
    function getOrCreateDecoder(serial: string): DecoderState {
      let state = decoders.get(serial);
      if (state && state.decoder.state !== 'closed') return state;

      const decoder = new VideoDecoder({
        output: (frame) => {
          const s = decoders.get(serial);
          if (!s) { frame.close(); return; }
          // Only keep the latest decoded frame — close any stale pending frame
          if (s.pendingFrame) s.pendingFrame.close();
          s.pendingFrame = frame;
        },
        error: (e) => {
          console.error(`[WebCodecs] decoder error serial=${serial}:`, e.message);
        },
      });

      state = {
        decoder, configured: false, ctx: null,
        lastWidth: 0, lastHeight: 0, lastConfig: null,
        waitingForKeyframe: false, pendingFrame: null,
      };
      decoders.set(serial, state);
      return state;
    }

    function handleFrame(frame: ParsedV3Frame) {
      const state = getOrCreateDecoder(frame.serial);

      if (frame.packetType === 0) {
        const { sps, pps } = extractSPSPPS(frame.nalData);
        if (sps.length === 0) return;
        const codecString = parseSPSCodecString(sps[0]);
        try {
          const description = buildAvcC(sps, pps);
          const config: VideoDecoderConfig = {
            codec: codecString,
            codedWidth: frame.width,
            codedHeight: frame.height,
            description,
            hardwareAcceleration: 'prefer-hardware',
          };
          state.decoder.configure(config);
          state.configured = true;
          state.lastConfig = config;
          state.waitingForKeyframe = false;
        } catch (e) {
          console.error(`[WebCodecs] configure failed serial=${frame.serial}:`, e);
        }
        return;
      }

      // Keyframe self-configure for late subscribers
      if (frame.packetType === 1 && !state.configured) {
        const { sps, pps } = extractSPSPPS(frame.nalData);
        if (sps.length > 0) {
          const codecString = parseSPSCodecString(sps[0]);
          try {
            const description = buildAvcC(sps, pps);
            const config: VideoDecoderConfig = {
              codec: codecString,
              codedWidth: frame.width,
              codedHeight: frame.height,
              description,
              hardwareAcceleration: 'prefer-hardware',
            };
            state.decoder.configure(config);
            state.configured = true;
            state.lastConfig = config;
          } catch { /* noop */ }
        }
      }

      if (!state.configured) return;

      // After reset, skip until keyframe
      if (state.waitingForKeyframe) {
        if (frame.packetType !== 1) return;
        state.waitingForKeyframe = false;
      }

      // Backpressure: if decode queue is building up, skip delta frames.
      // Keyframes always go through — they let the decoder catch up cleanly.
      const queueSize = state.decoder.decodeQueueSize;
      if (frame.packetType === 2 && queueSize > 3) {
        state.waitingForKeyframe = true;
        return;
      }

      try {
        const avcData = annexBToAvc(frame.nalData);
        const chunk = new EncodedVideoChunk({
          type: frame.packetType === 1 ? 'key' : 'delta',
          timestamp: Number(frame.pts),
          data: avcData,
        });
        state.decoder.decode(chunk);
      } catch (e) {
        console.error(`[WebCodecs] decode error serial=${frame.serial}:`, e);
      }
    }

    function cleanupDecoders(reason?: string) {
      for (const [serial, state] of decoders) {
        if (state.pendingFrame) { state.pendingFrame.close(); state.pendingFrame = null; }
        if (state.decoder.state !== 'closed') {
          try { state.decoder.close(); } catch { /* noop */ }
        }
        clearStreamFrame(serial);
        if (reason) {
          setStreamStatus(serial, 'disconnected', reason);
        }
      }
      decoders.clear();
    }

    function connect() {
      if (cancelled) return;
      let ws: WebSocket;
      try {
        ws = new WebSocket(WS_URL);
      } catch {
        scheduleReconnect();
        return;
      }
      ws.binaryType = 'arraybuffer';
      wsRef.current = ws;

      ws.onopen = () => {
        attempt = 0;
        subscribedRef.current = new Set();
        reconcile(ws, subscribedRef.current, desiredRef.current);
      };

      ws.onmessage = (ev) => {
        if (!(ev.data instanceof ArrayBuffer)) return;
        try {
          const frame = parseV3Frame(ev.data);
          if (!desiredRef.current.has(frame.serial)) return;
          handleFrame(frame);
        } catch {
          // ignore malformed frames
        }
      };

      ws.onclose = () => {
        if (wsRef.current === ws) wsRef.current = null;
        subscribedRef.current = new Set();
        cleanupDecoders('local video socket disconnected');
        scheduleReconnect();
      };

      ws.onerror = () => {};
    }

    function scheduleReconnect() {
      if (cancelled) return;
      attempt += 1;
      const delay = Math.min(
        MAX_RECONNECT_DELAY_MS,
        BASE_RECONNECT_DELAY_MS * Math.pow(2, attempt - 1),
      );
      reconnectTimer = setTimeout(connect, delay);
    }

    connect();

    return () => {
      cancelled = true;
      if (rafId !== null) cancelAnimationFrame(rafId);
      if (reconnectTimer) clearTimeout(reconnectTimer);
      const ws = wsRef.current;
      wsRef.current = null;
      if (ws) {
        ws.onopen = null;
        ws.onmessage = null;
        ws.onclose = null;
        ws.onerror = null;
        try { ws.close(); } catch { /* noop */ }
      }
      cleanupDecoders();
    };
  }, [setStreamFrame, clearStreamFrame, setStreamStatus]);
}

export function reconcile(
  ws: WebSocket | null,
  subscribed: Set<string>,
  desired: Set<string>,
): void {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  for (const s of desired) {
    if (!subscribed.has(s)) {
      ws.send(JSON.stringify({ type: 'subscribe', serial: s }));
      subscribed.add(s);
    }
  }
  for (const s of Array.from(subscribed)) {
    if (!desired.has(s)) {
      ws.send(JSON.stringify({ type: 'unsubscribe', serial: s }));
      subscribed.delete(s);
    }
  }
}
