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

function deviceStreamSource(d: { serial: string; server_host: string; server_port: number }) {
  return `${d.server_host}:${d.server_port}:${d.serial}`;
}

interface DecoderState {
  decoder: VideoDecoder;
  configured: boolean;
  ctx: CanvasRenderingContext2D | null;
  lastWidth: number;
  lastHeight: number;
  lastConfig: VideoDecoderConfig | null;
  waitingForKeyframe: boolean;
  pendingFrame: VideoFrame | null;
  lastSeq: bigint | null;
}

interface SocketState {
  serial: string;
  ws: WebSocket | null;
  attempt: number;
  reconnectTimer: ReturnType<typeof setTimeout> | null;
  closed: boolean;
}

interface ParsedV3Frame {
  serial: string;
  packetType: number; // 0=config, 1=key, 2=delta
  seq: bigint | null;
  pts: bigint;
  width: number;
  height: number;
  nalData: Uint8Array;
}

export function parseV3Frame(buf: ArrayBuffer): ParsedV3Frame {
  const view = new DataView(buf);
  let off = 0;
  const version = view.getUint8(off); off += 1;
  if (version !== 3 && version !== 4) throw new Error(`Unsupported frame version: ${version}`);
  const serialLen = view.getUint16(off, false); off += 2;
  const serialBytes = new Uint8Array(buf, off, serialLen); off += serialLen;
  const serial = new TextDecoder().decode(serialBytes);
  const packetType = view.getUint8(off); off += 1;
  const seq = version >= 4 ? view.getBigUint64(off, false) : null;
  if (version >= 4) off += 8;
  const pts = view.getBigUint64(off, false); off += 8;
  const width = view.getUint32(off, false); off += 4;
  const height = view.getUint32(off, false); off += 4;
  const nalData = new Uint8Array(buf, off);
  return { serial, packetType, seq, pts, width, height, nalData };
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
  const sourceBySerialRef = useRef<Map<string, string>>(new Map());
  const socketsRef = useRef<Map<string, SocketState>>(new Map());
  const decodersRef = useRef<Map<string, DecoderState>>(new Map());
  const connectSerialRef = useRef<((serial: string) => void) | null>(null);
  const closeSerialRef = useRef<((serial: string) => void) | null>(null);

  useEffect(() => {
    desiredRef.current = desired;
    const nextSources = new Map(
      devices
        .filter((d) => d.status === 'online' && !disabledSerials.has(d.serial))
        .map((d) => [d.serial, deviceStreamSource(d)]),
    );

    for (const serial of desired) {
      const previousSource = sourceBySerialRef.current.get(serial);
      const nextSource = nextSources.get(serial);
      if (previousSource && nextSource && previousSource !== nextSource) {
        closeSerialRef.current?.(serial);
        const decoder = decodersRef.current.get(serial);
        if (decoder?.pendingFrame) { decoder.pendingFrame.close(); decoder.pendingFrame = null; }
        if (decoder && decoder.decoder.state !== 'closed') {
          try { decoder.decoder.close(); } catch { /* noop */ }
        }
        decodersRef.current.delete(serial);
        clearStreamFrame(serial);
      }
    }
    sourceBySerialRef.current = nextSources;

    for (const serial of Array.from(socketsRef.current.keys())) {
      if (!desired.has(serial)) {
        closeSerialRef.current?.(serial);
      }
    }
    for (const serial of desired) {
      if (!socketsRef.current.has(serial)) {
        connectSerialRef.current?.(serial);
      }
    }

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
    let rafId: number | null = null;
    const decoders = decodersRef.current;

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

    function getOrCreateDecoder(serial: string): DecoderState {
      let state = decoders.get(serial);
      if (state && state.decoder.state !== 'closed') return state;

      const decoder = new VideoDecoder({
        output: (frame) => {
          const s = decoders.get(serial);
          if (!s) { frame.close(); return; }
          if (s.pendingFrame) s.pendingFrame.close();
          s.pendingFrame = frame;
        },
        error: (e) => {
          console.error(`[WebCodecs] decoder error serial=${serial}:`, e.message);
          const current = decoders.get(serial);
          if (current?.decoder === decoder) {
            resetDecoder(serial);
          }
        },
      });

      state = {
        decoder, configured: false, ctx: null,
        lastWidth: 0, lastHeight: 0, lastConfig: null,
        waitingForKeyframe: false, pendingFrame: null, lastSeq: null,
      };
      decoders.set(serial, state);
      return state;
    }

    function closeDecoderState(state: DecoderState) {
      if (state.pendingFrame) { state.pendingFrame.close(); state.pendingFrame = null; }
      if (state.decoder.state !== 'closed') {
        try { state.decoder.close(); } catch { /* noop */ }
      }
    }

    function resetDecoder(serial: string) {
      const state = decoders.get(serial);
      if (!state) return;
      closeDecoderState(state);
      decoders.delete(serial);
    }

    function configureDecoder(
      state: DecoderState,
      frame: ParsedV3Frame,
      sps: Uint8Array[],
      pps: Uint8Array[],
    ): boolean {
      const codecString = parseSPSCodecString(sps[0]);
      const description = buildAvcC(sps, pps);
      const modes: NonNullable<VideoDecoderConfig['hardwareAcceleration']>[] = [
        'prefer-hardware',
        'no-preference',
        'prefer-software',
      ];

      for (const hardwareAcceleration of modes) {
        try {
          const config: VideoDecoderConfig = {
            codec: codecString,
            codedWidth: frame.width,
            codedHeight: frame.height,
            description,
            hardwareAcceleration,
          };
          state.decoder.configure(config);
          state.configured = true;
          state.lastConfig = config;
          state.waitingForKeyframe = false;
          return true;
        } catch (e) {
          console.warn(
            `[WebCodecs] configure failed serial=${frame.serial} acceleration=${hardwareAcceleration}:`,
            e,
          );
        }
      }
      return false;
    }

    function handleFrame(frame: ParsedV3Frame) {
      let state = getOrCreateDecoder(frame.serial);
      if (frame.seq !== null) {
        const expected = state.lastSeq === null ? null : state.lastSeq + 1n;
        const hasGap = expected !== null && frame.seq !== expected;
        state.lastSeq = frame.seq;
        if (hasGap && frame.packetType === 2) {
          resetDecoder(frame.serial);
          return;
        }
        if (hasGap && frame.packetType === 1) {
          resetDecoder(frame.serial);
          state = getOrCreateDecoder(frame.serial);
          state.lastSeq = frame.seq;
        }
      }

      if (frame.packetType === 0) {
        const { sps, pps } = extractSPSPPS(frame.nalData);
        if (sps.length === 0) return;
        try {
          configureDecoder(state, frame, sps, pps);
        } catch (e) {
          console.error(`[WebCodecs] configure failed serial=${frame.serial}:`, e);
          resetDecoder(frame.serial);
        }
        return;
      }

      if (frame.packetType === 1 && !state.configured) {
        const { sps, pps } = extractSPSPPS(frame.nalData);
        if (sps.length > 0) {
          try {
            configureDecoder(state, frame, sps, pps);
          } catch {
            resetDecoder(frame.serial);
          }
        }
      }

      if (!state.configured) return;

      if (state.waitingForKeyframe) {
        if (frame.packetType !== 1) return;
        state.waitingForKeyframe = false;
      }

      const queueSize = state.decoder.decodeQueueSize;
      if (frame.packetType === 2 && queueSize > 2) {
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
        resetDecoder(frame.serial);
      }
    }

    function closeSocket(serial: string) {
      const state = socketsRef.current.get(serial);
      if (!state) return;
      state.closed = true;
      if (state.reconnectTimer) {
        clearTimeout(state.reconnectTimer);
        state.reconnectTimer = null;
      }
      const ws = state.ws;
      state.ws = null;
      socketsRef.current.delete(serial);
      if (ws) {
        ws.onopen = null;
        ws.onmessage = null;
        ws.onclose = null;
        ws.onerror = null;
        try { ws.close(); } catch { /* noop */ }
      }
    }

    function connectSerial(serial: string) {
      if (cancelled || socketsRef.current.has(serial)) return;
      const state: SocketState = {
        serial,
        ws: null,
        attempt: 0,
        reconnectTimer: null,
        closed: false,
      };
      socketsRef.current.set(serial, state);
      openSocket(state);
    }

    function openSocket(state: SocketState) {
      if (cancelled || state.closed || !desiredRef.current.has(state.serial)) return;
      let ws: WebSocket;
      try {
        ws = new WebSocket(WS_URL);
      } catch {
        scheduleReconnect(state);
        return;
      }
      ws.binaryType = 'arraybuffer';
      state.ws = ws;

      ws.onopen = () => {
        state.attempt = 0;
        if (ws.readyState === WebSocket.OPEN && desiredRef.current.has(state.serial)) {
          ws.send(JSON.stringify({ type: 'subscribe', serial: state.serial }));
        }
      };

      ws.onmessage = (ev) => {
        if (!(ev.data instanceof ArrayBuffer)) return;
        try {
          const frame = parseV3Frame(ev.data);
          if (frame.serial !== state.serial || !desiredRef.current.has(frame.serial)) return;
          handleFrame(frame);
        } catch {
          // ignore malformed frames
        }
      };

      ws.onclose = () => {
        if (state.ws === ws) state.ws = null;
        const decoderState = decoders.get(state.serial);
        if (decoderState) {
          closeDecoderState(decoderState);
          decoders.delete(state.serial);
        }
        setStreamStatus(state.serial, 'reconnecting', 'local video socket disconnected');
        scheduleReconnect(state);
      };

      ws.onerror = () => {};
    }

    function scheduleReconnect(state: SocketState) {
      if (cancelled || state.closed || !desiredRef.current.has(state.serial)) return;
      state.attempt += 1;
      const delay = Math.min(
        MAX_RECONNECT_DELAY_MS,
        BASE_RECONNECT_DELAY_MS * Math.pow(2, state.attempt - 1),
      );
      state.reconnectTimer = setTimeout(() => {
        state.reconnectTimer = null;
        openSocket(state);
      }, delay);
    }

    function cleanupDecoders() {
      for (const [serial, state] of decoders) {
        closeDecoderState(state);
        clearStreamFrame(serial);
      }
      decoders.clear();
    }

    connectSerialRef.current = connectSerial;
    closeSerialRef.current = closeSocket;
    for (const serial of desiredRef.current) {
      connectSerial(serial);
    }

    return () => {
      cancelled = true;
      connectSerialRef.current = null;
      closeSerialRef.current = null;
      if (rafId !== null) cancelAnimationFrame(rafId);
      for (const serial of Array.from(socketsRef.current.keys())) {
        closeSocket(serial);
      }
      cleanupDecoders();
    };
  }, [setStreamFrame, clearStreamFrame, setStreamStatus]);
}

export function reconcile(
  ws: WebSocket | null,
  subscribed: Set<string>,
  desired: Set<string>,
  options?: {
    subscribeBatchSize?: number;
    subscribeBatchGapMs?: number;
    timers?: ReturnType<typeof setTimeout>[];
    getDesired?: () => Set<string>;
  },
): void {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  for (const s of Array.from(subscribed)) {
    if (!desired.has(s)) {
      ws.send(JSON.stringify({ type: 'unsubscribe', serial: s }));
      subscribed.delete(s);
    }
  }

  const toSubscribe = Array.from(desired).filter((s) => !subscribed.has(s));
  const batchSize = options?.subscribeBatchSize ?? toSubscribe.length;
  const batchGapMs = options?.subscribeBatchGapMs ?? 0;
  const timers = options?.timers;

  for (const [index, s] of toSubscribe.entries()) {
    subscribed.add(s);
    const sendSubscribe = () => {
      const stillDesired = options?.getDesired?.().has(s) ?? desired.has(s);
      if (!stillDesired) {
        subscribed.delete(s);
        return;
      }
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'subscribe', serial: s }));
      } else {
        subscribed.delete(s);
      }
    };

    if (!timers || batchGapMs <= 0 || batchSize <= 0) {
      sendSubscribe();
      continue;
    }

    const batch = Math.floor(index / batchSize);
    const delay = batch * batchGapMs;
    if (delay <= 0) {
      sendSubscribe();
    } else {
      timers.push(setTimeout(sendSubscribe, delay));
    }
  }
}
