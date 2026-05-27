import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useStream } from './useStream';
import { useStore } from '../store';
import type { Device } from '../types';

// ─── Fake WebSocket ──────────────────────────────────────────────────────────

const OPEN = 1;
const CLOSED = 3;

interface FakeWs {
  url: string;
  readyState: number;
  binaryType: string;
  onopen: ((ev?: unknown) => void) | null;
  onclose: ((ev?: unknown) => void) | null;
  onerror: ((ev?: unknown) => void) | null;
  onmessage: ((ev: { data: ArrayBuffer | string }) => void) | null;
  sent: string[];
  close: () => void;
  send: (data: string) => void;
  /** Test-only helper: simulate server opening the connection. */
  _open: () => void;
  /** Test-only helper: simulate a drop from the server. */
  _serverClose: () => void;
}

let instances: FakeWs[] = [];

function installFakeWebSocket() {
  instances = [];
  (globalThis as any).WebSocket = class implements Partial<FakeWs> {
    static OPEN = OPEN;
    static CLOSED = CLOSED;
    url: string;
    readyState = 0;
    binaryType = 'blob';
    onopen: FakeWs['onopen'] = null;
    onclose: FakeWs['onclose'] = null;
    onerror: FakeWs['onerror'] = null;
    onmessage: FakeWs['onmessage'] = null;
    sent: string[] = [];
    constructor(url: string) {
      this.url = url;
      // Register on the *instance* object so the test can access the real reference.
      (this as unknown as FakeWs)._open = () => {
        this.readyState = OPEN;
        this.onopen?.();
      };
      (this as unknown as FakeWs)._serverClose = () => {
        this.readyState = CLOSED;
        this.onclose?.();
      };
      instances.push(this as unknown as FakeWs);
    }
    send(data: string) { this.sent.push(data); }
    close() {
      this.readyState = CLOSED;
      this.onclose?.();
    }
  };
  // Also expose the constant on the constructor the way browsers do.
  (globalThis as any).WebSocket.OPEN = OPEN;
  (globalThis as any).WebSocket.CLOSED = CLOSED;
}

function device(serial: string, status: 'online' | 'offline' = 'online'): Device {
  return {
    serial,
    status,
    model: 'Test',
    battery: 100,
    screen_width: 1080,
    screen_height: 1920,
    server_host: '127.0.0.1',
    server_port: 5037,
  };
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('useStream', () => {
  beforeEach(() => {
    installFakeWebSocket();
    useStore.setState({
      devices: [],
      disabledSerials: new Set(),
      selectedSerials: new Set(),
      streamFrames: {},
      fps: 10,
    });
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('opens one WebSocket per desired device', () => {
    useStore.setState({ devices: [device('a'), device('b')] });
    renderHook(() => useStream());
    expect(instances.length).toBe(2);
    expect(instances[0].url).toBe('ws://127.0.0.1:32199');
    expect(instances[0].binaryType).toBe('arraybuffer');
  });

  it('subscribes to online non-disabled devices on open', () => {
    useStore.setState({
      devices: [device('a'), device('b', 'offline'), device('c')],
      disabledSerials: new Set(['c']),
    });
    renderHook(() => useStream());
    act(() => { instances[0]._open(); });
    expect(instances[0].sent).toEqual([
      JSON.stringify({ type: 'subscribe', serial: 'a' }),
    ]);
  });

  it('does not re-open WebSocket when fps changes', () => {
    useStore.setState({ devices: [device('a')] });
    const { rerender } = renderHook(() => useStream());
    act(() => { instances[0]._open(); });
    expect(instances.length).toBe(1);

    act(() => { useStore.setState({ fps: 25 }); });
    rerender();
    expect(instances.length).toBe(1);
  });

  it('opens and closes sockets as devices change', () => {
    useStore.setState({ devices: [device('a')] });
    renderHook(() => useStream());
    act(() => { instances[0]._open(); });
    expect(instances[0].sent).toEqual([
      JSON.stringify({ type: 'subscribe', serial: 'a' }),
    ]);

    // Add device b
    act(() => { useStore.setState({ devices: [device('a'), device('b')] }); });
    expect(instances.length).toBe(2);
    act(() => { instances[1]._open(); });
    expect(instances[1].sent).toContain(JSON.stringify({ type: 'subscribe', serial: 'b' }));

    // Remove device a
    act(() => { useStore.setState({ devices: [device('b')] }); });
    expect(instances[0].readyState).toBe(CLOSED);
  });

  it('closes the device socket when a device is disabled', () => {
    useStore.setState({ devices: [device('a'), device('b')] });
    renderHook(() => useStream());
    act(() => { instances[0]._open(); });

    act(() => { useStore.setState({ disabledSerials: new Set(['a']) }); });
    expect(instances[0].readyState).toBe(CLOSED);
  });

  it('reconnects after the server drops the connection', () => {
    useStore.setState({ devices: [device('a')] });
    renderHook(() => useStream());
    act(() => { instances[0]._open(); });
    expect(instances.length).toBe(1);

    // Drop the connection
    act(() => { instances[0]._serverClose(); });
    // Reconnect is scheduled via setTimeout — advance timers
    act(() => { vi.advanceTimersByTime(200); });
    expect(instances.length).toBe(2);

    // New socket should re-subscribe on open
    act(() => { instances[1]._open(); });
    expect(instances[1].sent).toEqual([
      JSON.stringify({ type: 'subscribe', serial: 'a' }),
    ]);
  });

  it('backs off exponentially on repeated failures', () => {
    useStore.setState({ devices: [device('a')] });
    renderHook(() => useStream());
    // 1st close → ~200ms
    act(() => { instances[0]._serverClose(); });
    act(() => { vi.advanceTimersByTime(199); });
    expect(instances.length).toBe(1);
    act(() => { vi.advanceTimersByTime(10); });
    expect(instances.length).toBe(2);

    // 2nd close → ~400ms
    act(() => { instances[1]._serverClose(); });
    act(() => { vi.advanceTimersByTime(399); });
    expect(instances.length).toBe(2);
    act(() => { vi.advanceTimersByTime(10); });
    expect(instances.length).toBe(3);
  });

  it('does not reconnect after unmount', () => {
    useStore.setState({ devices: [device('a')] });
    const { unmount } = renderHook(() => useStream());
    unmount();
    act(() => { vi.advanceTimersByTime(10_000); });
    // Initial socket is counted; no second one should spawn
    expect(instances.length).toBe(1);
  });
});
