import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useStore } from '../store';

interface ScreenshotPayload {
  serial: string;
  data: string;
}

export function useScreenshot() {
  const setScreenshot = useStore((s) => s.setScreenshot);
  const setStreamHeartbeat = useStore((s) => s.setStreamHeartbeat);
  const setStreamStatus = useStore((s) => s.setStreamStatus);
  const clearStreamFrame = useStore((s) => s.clearStreamFrame);

  useEffect(() => {
    const unlisten = listen<ScreenshotPayload>('screenshot', (event) => {
      setScreenshot(event.payload.serial, event.payload.data);
    });

    const unlistenHb = listen<{ serial: string; bytes: number }>('stream-heartbeat', (event) => {
      setStreamHeartbeat(event.payload.serial, event.payload.bytes);
    });

    const unlistenStatus = listen<{ serial: string; status: string; error?: string }>('stream-status', (event) => {
      const { serial, status, error } = event.payload;
      setStreamStatus(serial, status, error);
      if (status !== 'connected' && status !== 'receiving') {
        clearStreamFrame(serial);
      }
    });

    const unlistenError = listen<{ serial: string; error?: string }>('stream-error', (event) => {
      const { serial, error } = event.payload;
      setStreamStatus(serial, 'disconnected', error ?? 'video stream error');
      clearStreamFrame(serial);
    });

    return () => {
      unlisten.then((fn) => fn());
      unlistenHb.then((fn) => fn());
      unlistenStatus.then((fn) => fn());
      unlistenError.then((fn) => fn());
    };
  }, [setScreenshot, setStreamHeartbeat, setStreamStatus, clearStreamFrame]);
}
