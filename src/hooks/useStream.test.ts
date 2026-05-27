import { describe, it, expect, vi } from 'vitest';
import { parseV3Frame, reconcile } from './useStream';

function makeV3Frame(
  serial: string,
  packetType: number,
  pts: bigint,
  width: number,
  height: number,
  nalData: Uint8Array,
): ArrayBuffer {
  const serialBytes = new TextEncoder().encode(serial);
  const buf = new ArrayBuffer(1 + 2 + serialBytes.length + 1 + 8 + 4 + 4 + nalData.length);
  const view = new DataView(buf);
  let off = 0;
  view.setUint8(off, 3); off += 1;
  view.setUint16(off, serialBytes.length, false); off += 2;
  new Uint8Array(buf, off, serialBytes.length).set(serialBytes); off += serialBytes.length;
  view.setUint8(off, packetType); off += 1;
  view.setBigUint64(off, pts, false); off += 8;
  view.setUint32(off, width, false); off += 4;
  view.setUint32(off, height, false); off += 4;
  new Uint8Array(buf, off, nalData.length).set(nalData);
  return buf;
}

function makeV4Frame(
  serial: string,
  packetType: number,
  seq: bigint,
  pts: bigint,
  width: number,
  height: number,
  nalData: Uint8Array,
): ArrayBuffer {
  const serialBytes = new TextEncoder().encode(serial);
  const buf = new ArrayBuffer(1 + 2 + serialBytes.length + 1 + 8 + 8 + 4 + 4 + nalData.length);
  const view = new DataView(buf);
  let off = 0;
  view.setUint8(off, 4); off += 1;
  view.setUint16(off, serialBytes.length, false); off += 2;
  new Uint8Array(buf, off, serialBytes.length).set(serialBytes); off += serialBytes.length;
  view.setUint8(off, packetType); off += 1;
  view.setBigUint64(off, seq, false); off += 8;
  view.setBigUint64(off, pts, false); off += 8;
  view.setUint32(off, width, false); off += 4;
  view.setUint32(off, height, false); off += 4;
  new Uint8Array(buf, off, nalData.length).set(nalData);
  return buf;
}

describe('parseV3Frame', () => {
  it('parses a valid v3 config frame', () => {
    const nal = new Uint8Array([0x00, 0x00, 0x00, 0x01, 0x67]);
    const frame = makeV3Frame('abc', 0, 0n, 720, 1280, nal);
    const out = parseV3Frame(frame);
    expect(out.serial).toBe('abc');
    expect(out.packetType).toBe(0);
    expect(out.seq).toBeNull();
    expect(out.pts).toBe(0n);
    expect(out.width).toBe(720);
    expect(out.height).toBe(1280);
    expect(out.nalData).toEqual(nal);
  });

  it('parses a key frame', () => {
    const nal = new Uint8Array([0x00, 0x00, 0x01, 0x65, 0xAA]);
    const frame = makeV3Frame('dev1', 1, 12345n, 1080, 1920, nal);
    const out = parseV3Frame(frame);
    expect(out.serial).toBe('dev1');
    expect(out.packetType).toBe(1);
    expect(out.pts).toBe(12345n);
    expect(out.width).toBe(1080);
    expect(out.height).toBe(1920);
  });

  it('parses a v4 frame sequence number', () => {
    const nal = new Uint8Array([0x00, 0x00, 0x01, 0x65]);
    const frame = makeV4Frame('dev1', 1, 42n, 12345n, 1080, 1920, nal);
    const out = parseV3Frame(frame);
    expect(out.serial).toBe('dev1');
    expect(out.packetType).toBe(1);
    expect(out.seq).toBe(42n);
    expect(out.pts).toBe(12345n);
    expect(out.nalData).toEqual(nal);
  });

  it('handles multi-byte utf-8 serials', () => {
    const nal = new Uint8Array([0xFF]);
    const frame = makeV3Frame('device-测试-42', 2, 0n, 1, 1, nal);
    expect(parseV3Frame(frame).serial).toBe('device-测试-42');
  });

  it('rejects unknown version', () => {
    const nal = new Uint8Array([0xFF]);
    const frame = makeV3Frame('x', 0, 0n, 1, 1, nal);
    new DataView(frame).setUint8(0, 99);
    expect(() => parseV3Frame(frame)).toThrow(/version/i);
  });

  it('handles empty NAL payload', () => {
    const frame = makeV3Frame('x', 0, 0n, 0, 0, new Uint8Array(0));
    const out = parseV3Frame(frame);
    expect(out.nalData.length).toBe(0);
  });
});

function makeFakeWs(readyState = 1 /* OPEN */) {
  const send = vi.fn();
  return { ws: { readyState, send } as unknown as WebSocket, send };
}

describe('reconcile', () => {
  it('subscribes to new serials', () => {
    const { ws, send } = makeFakeWs();
    const subscribed = new Set<string>();
    const desired = new Set(['a', 'b']);
    reconcile(ws, subscribed, desired);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenCalledWith(JSON.stringify({ type: 'subscribe', serial: 'a' }));
    expect(send).toHaveBeenCalledWith(JSON.stringify({ type: 'subscribe', serial: 'b' }));
    expect(subscribed).toEqual(new Set(['a', 'b']));
  });

  it('unsubscribes from removed serials', () => {
    const { ws, send } = makeFakeWs();
    const subscribed = new Set(['a', 'b']);
    const desired = new Set(['a']);
    reconcile(ws, subscribed, desired);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(JSON.stringify({ type: 'unsubscribe', serial: 'b' }));
    expect(subscribed).toEqual(new Set(['a']));
  });

  it('sends no messages when already in sync', () => {
    const { ws, send } = makeFakeWs();
    const subscribed = new Set(['a', 'b']);
    const desired = new Set(['a', 'b']);
    reconcile(ws, subscribed, desired);
    expect(send).not.toHaveBeenCalled();
  });

  it('handles add+remove in one pass', () => {
    const { ws, send } = makeFakeWs();
    const subscribed = new Set(['a', 'b']);
    const desired = new Set(['b', 'c']);
    reconcile(ws, subscribed, desired);
    expect(send).toHaveBeenCalledTimes(2);
    expect(subscribed).toEqual(new Set(['b', 'c']));
  });

  it('is a no-op when ws is null', () => {
    const subscribed = new Set<string>();
    expect(() => reconcile(null, subscribed, new Set(['a']))).not.toThrow();
    expect(subscribed.size).toBe(0);
  });

  it('is a no-op when ws is not OPEN', () => {
    const { ws, send } = makeFakeWs(0 /* CONNECTING */);
    const subscribed = new Set<string>();
    reconcile(ws, subscribed, new Set(['a']));
    expect(send).not.toHaveBeenCalled();
    expect(subscribed.size).toBe(0);
  });
});
