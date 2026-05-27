import { create } from 'zustand';
import type { AdbServer, Device } from '../types';

const PAGE_SIZE = 10;

interface AppStore {
  // Servers
  servers: AdbServer[];
  setServers: (servers: AdbServer[]) => void;

  // Devices
  devices: Device[];
  setDevices: (devices: Device[]) => void;

  // Disabled devices
  disabledSerials: Set<string>;
  toggleDisableDevice: (serial: string) => void;

  // Selection
  selectedSerials: Set<string>;
  toggleSelect: (serial: string) => void;
  selectAll: () => void;
  clearSelection: () => void;

  // Stream status (scrcpy)
  streamHeartbeats: Record<string, number>;
  setStreamHeartbeat: (serial: string, bytes: number) => void;

  // Latest frame dimensions per device (set when WebCodecs decodes a frame)
  streamFrames: Record<string, { width: number; height: number }>;
  setStreamFrame: (serial: string, width: number, height: number) => void;
  clearStreamFrame: (serial: string) => void;

  // Stream status
  streamStatus: Record<string, { status: string; error?: string }>;
  setStreamStatus: (serial: string, status: string, error?: string) => void;

  // Group input
  groupInputBusy: boolean;
  setGroupInputBusy: (busy: boolean) => void;

  // FPS
  fps: number;
  setFps: (fps: number) => void;

  // Sidebar
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;

  // Pagination
  page: number;
  pageSize: number;
  setPage: (page: number) => void;
  setPageSize: (size: number) => void;

  // Overview mode
  overviewMode: boolean;
  setOverviewMode: (on: boolean) => void;
}

export const useStore = create<AppStore>((set) => ({
  servers: [],
  setServers: (servers) => set({ servers }),

  devices: [],
  setDevices: (devices) => set((s) => {
    const totalPages = Math.max(1, Math.ceil(devices.length / s.pageSize));
    return { devices, page: Math.min(s.page, totalPages - 1) };
  }),

  disabledSerials: new Set(),
  toggleDisableDevice: (serial) =>
    set((s) => {
      const next = new Set(s.disabledSerials);
      if (next.has(serial)) next.delete(serial);
      else next.add(serial);
      return { disabledSerials: next };
    }),

  selectedSerials: new Set(),
  toggleSelect: (serial) =>
    set((s) => {
      const next = new Set(s.selectedSerials);
      if (next.has(serial)) next.delete(serial);
      else next.add(serial);
      return { selectedSerials: next };
    }),
  selectAll: () =>
    set((s) => ({
      selectedSerials: new Set(
        s.devices.filter((d) => d.status === 'online').map((d) => d.serial)
      ),
    })),
  clearSelection: () => set({ selectedSerials: new Set() }),

  streamHeartbeats: {},
  setStreamHeartbeat: (serial, bytes) =>
    set((s) => ({ streamHeartbeats: { ...s.streamHeartbeats, [serial]: bytes } })),

  streamFrames: {},
  setStreamFrame: (serial, width, height) =>
    set((s) => ({ streamFrames: { ...s.streamFrames, [serial]: { width, height } } })),
  clearStreamFrame: (serial) =>
    set((s) => {
      const next = { ...s.streamFrames };
      delete next[serial];
      return { streamFrames: next };
    }),

  streamStatus: {},
  setStreamStatus: (serial, status, error) =>
    set((s) => ({ streamStatus: { ...s.streamStatus, [serial]: { status, error } } })),

  groupInputBusy: false,
  setGroupInputBusy: (busy) => set({ groupInputBusy: busy }),

  fps: 10,
  setFps: (fps) => set({ fps }),

  sidebarCollapsed: false,
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),

  page: 0,
  pageSize: PAGE_SIZE,
  setPage: (page) => set({ page }),
  setPageSize: (size) => set((s) => {
    const totalPages = Math.max(1, Math.ceil(s.devices.length / size));
    return { pageSize: size, page: Math.min(s.page, totalPages - 1) };
  }),

  overviewMode: false,
  setOverviewMode: (on) => set({ overviewMode: on }),
}));
