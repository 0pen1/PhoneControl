import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useStore } from './store';
import { useDevices } from './hooks/useDevices';
import { useStreamEvents } from './hooks/useStreamEvents';
import { useStream } from './hooks/useStream';
import { Sidebar } from './components/Sidebar/Sidebar';
import { DeviceGrid } from './components/DeviceGrid/DeviceGrid';
import { Toolbar } from './components/Toolbar/Toolbar';
import type { AdbServer } from './types';
import styles from './App.module.css';

export default function App() {
  const setServers = useStore((s) => s.setServers);

  useDevices();
  useStreamEvents();
  useStream();

  useEffect(() => {
    const timers: ReturnType<typeof setTimeout>[] = [];
    invoke<AdbServer[]>('load_config').then((servers) => {
      setServers(servers);
      // Refresh immediately, then retry shortly after in case the webview
      // registered listeners while the first backend poll was already running.
      invoke('refresh_devices').catch(() => {});
      timers.push(setTimeout(() => invoke('refresh_devices').catch(() => {}), 800));
      timers.push(setTimeout(() => invoke('refresh_devices').catch(() => {}), 2500));
    }).catch(() => {});
    return () => {
      for (const timer of timers) clearTimeout(timer);
    };
  }, [setServers]);

  return (
    <div className={styles.layout}>
      <Sidebar />
      <div className={styles.main}>
        <div className={styles.content}>
          <DeviceGrid />
        </div>
        <Toolbar />
      </div>
    </div>
  );
}
