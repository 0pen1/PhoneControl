import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useStore } from '../store';

interface StreamEventPayload {
  serial: string;
  serverHost?: string;
  serverPort?: number;
  sessionId?: number;
  status?: string;
  error?: string;
  bytes?: number;
}

function isCurrentDeviceEvent(payload: StreamEventPayload): boolean {
  if (!payload.serverHost || payload.serverPort === undefined) return true;
  const device = useStore.getState().devices.find((d) => d.serial === payload.serial);
  return !!device
    && device.server_host === payload.serverHost
    && device.server_port === payload.serverPort;
}

export function useStreamEvents() {
  const setStreamHeartbeat = useStore((s) => s.setStreamHeartbeat);
  const setStreamStatus = useStore((s) => s.setStreamStatus);
  const clearStreamFrame = useStore((s) => s.clearStreamFrame);

  useEffect(() => {
    const unlistenHb = listen<StreamEventPayload>('stream-heartbeat', (event) => {
      if (!isCurrentDeviceEvent(event.payload) || event.payload.bytes === undefined) return;
      setStreamHeartbeat(event.payload.serial, event.payload.bytes);
    });

    const unlistenStatus = listen<StreamEventPayload>('stream-status', (event) => {
      if (!isCurrentDeviceEvent(event.payload) || !event.payload.status) return;
      const { serial, status, error } = event.payload;
      setStreamStatus(serial, status, error);
      if (status === 'disconnected' || status === 'stopped') {
        clearStreamFrame(serial);
      }
    });

    const unlistenError = listen<StreamEventPayload>('stream-error', (event) => {
      if (!isCurrentDeviceEvent(event.payload)) return;
      const { serial, error } = event.payload;
      setStreamStatus(serial, 'disconnected', error ?? 'video stream error');
      clearStreamFrame(serial);
    });

    return () => {
      unlistenHb.then((fn) => fn());
      unlistenStatus.then((fn) => fn());
      unlistenError.then((fn) => fn());
    };
  }, [setStreamHeartbeat, setStreamStatus, clearStreamFrame]);
}
