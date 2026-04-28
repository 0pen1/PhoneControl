import { invoke } from '@tauri-apps/api/core';
import { useStore } from '../store';
import type { CommandResult, DeviceResolution, Device } from '../types';

export function useAdbCommands() {
  const selectedDevices = (primarySerial?: string): DeviceResolution[] => {
    const { devices, selectedSerials } = useStore.getState();
    const serials = devices
      .filter((d) => selectedSerials.has(d.serial) && d.status === 'online')
      .map((d) => ({
        serial: d.serial,
        width: d.screen_width,
        height: d.screen_height,
        server_host: d.server_host,
        server_port: d.server_port,
      }));

    if (!primarySerial) return serials;

    const idx = serials.findIndex((d) => d.serial === primarySerial);
    if (idx <= 0) return serials;

    const [primary] = serials.splice(idx, 1);
    serials.unshift(primary);
    return serials;
  };

  return {
    async tapDevices(x: number, y: number, sourceWidth: number, sourceHeight: number, primarySerial?: string): Promise<CommandResult[]> {
      const serials = selectedDevices(primarySerial);
      return invoke<CommandResult[]>('tap_devices', { serials, x, y, sourceWidth, sourceHeight });
    },

    async tapDevice(device: Device, x: number, y: number, sourceWidth: number, sourceHeight: number): Promise<CommandResult[]> {
      const serials: DeviceResolution[] = [{
        serial: device.serial,
        width: device.screen_width,
        height: device.screen_height,
        server_host: device.server_host,
        server_port: device.server_port,
      }];
      return invoke<CommandResult[]>('tap_devices', { serials, x, y, sourceWidth, sourceHeight });
    },

    async swipeDevices(
      x1: number, y1: number, x2: number, y2: number,
      durationMs: number, sourceWidth: number, sourceHeight: number, primarySerial?: string
    ): Promise<CommandResult[]> {
      const serials = selectedDevices(primarySerial);
      return invoke<CommandResult[]>('swipe_devices', { serials, x1, y1, x2, y2, durationMs, sourceWidth, sourceHeight });
    },

    async swipeDevice(
      device: Device,
      x1: number, y1: number, x2: number, y2: number,
      durationMs: number, sourceWidth: number, sourceHeight: number
    ): Promise<CommandResult[]> {
      const serials: DeviceResolution[] = [{
        serial: device.serial,
        width: device.screen_width,
        height: device.screen_height,
        server_host: device.server_host,
        server_port: device.server_port,
      }];
      return invoke<CommandResult[]>('swipe_devices', { serials, x1, y1, x2, y2, durationMs, sourceWidth, sourceHeight });
    },

    async sendText(text: string): Promise<CommandResult[]> {
      const { devices, selectedSerials } = useStore.getState();
      const serials: DeviceResolution[] = devices
        .filter((d) => selectedSerials.has(d.serial) && d.status === 'online')
        .map((d) => ({
          serial: d.serial,
          width: d.screen_width,
          height: d.screen_height,
          server_host: d.server_host,
          server_port: d.server_port,
        }));
      return invoke<CommandResult[]>('send_text_devices', { serials, text });
    },

    async keyevent(keycode: number): Promise<CommandResult[]> {
      const { devices, selectedSerials } = useStore.getState();
      const serials: DeviceResolution[] = devices
        .filter((d) => selectedSerials.has(d.serial) && d.status === 'online')
        .map((d) => ({
          serial: d.serial,
          width: d.screen_width,
          height: d.screen_height,
          server_host: d.server_host,
          server_port: d.server_port,
        }));
      return invoke<CommandResult[]>('keyevent_devices', { serials, keycode });
    },

    async setUsbFileTransfer(): Promise<CommandResult[]> {
      const { devices, selectedSerials } = useStore.getState();
      const serials: DeviceResolution[] = devices
        .filter((d) => selectedSerials.has(d.serial) && d.status === 'online')
        .map((d) => ({
          serial: d.serial,
          width: d.screen_width,
          height: d.screen_height,
          server_host: d.server_host,
          server_port: d.server_port,
        }));
      return invoke<CommandResult[]>('set_usb_file_transfer_devices', { serials });
    },

    startPreview(serial: string, fps: number, serverHost: string, serverPort: number) {
      return invoke<void>('start_preview', { serial, fps, serverHost, serverPort });
    },

    stopPreview(serial: string) {
      return invoke<void>('stop_preview', { serial });
    },

    setFps(serial: string, fps: number, serverHost: string, serverPort: number) {
      return invoke<void>('set_fps', { serial, fps, serverHost, serverPort });
    },

    startStream(
      serial: string,
      serverHost: string,
      serverPort: number,
      options?: { max_size: number; max_fps: number; bit_rate: number }
    ) {
      return invoke<void>('start_stream', {
        serial,
        serverHost,
        serverPort,
        options: options ?? { max_size: 720, max_fps: 30, bit_rate: 4_000_000 },
      });
    },

    stopStream(serial: string) {
      return invoke<void>('stop_stream', { serial });
    },

    launchScrcpy(serial: string, serverHost: string, serverPort: number) {
      return invoke<void>('launch_scrcpy', { serial, serverHost, serverPort });
    },

    async runShell(cmd: string): Promise<CommandResult[]> {
      const { devices, selectedSerials } = useStore.getState();
      const serials: DeviceResolution[] = devices
        .filter((d) => selectedSerials.has(d.serial) && d.status === 'online')
        .map((d) => ({
          serial: d.serial,
          width: d.screen_width,
          height: d.screen_height,
          server_host: d.server_host,
          server_port: d.server_port,
        }));
      return invoke<CommandResult[]>('run_shell_devices', { serials, cmd });
    },

    async wakeUpDevices(serials: DeviceResolution[]): Promise<CommandResult[]> {
      return invoke<CommandResult[]>('wake_up_devices', { serials });
    },
  };
}
