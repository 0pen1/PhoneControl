import React, { useCallback, useRef, useState, useEffect } from 'react';
import { useStore } from '../../store';
import { useAdbCommands } from '../../hooks/useAdbCommands';
import { getImageLayout } from '../../utils/imageLayout';
import { registerCanvas, unregisterCanvas } from '../../utils/canvasRegistry';
import type { Device } from '../../types';
import styles from './DeviceCard.module.css';

interface Props {
  device: Device;
  screenshot: string | undefined;
  selected: boolean;
  streamOptions: { max_size: number; max_fps: number; bit_rate: number };
}

function DeviceCardInner({ device, screenshot, selected, streamOptions }: Props) {
  const toggleSelect = useStore((s) => s.toggleSelect);
  const toggleDisableDevice = useStore((s) => s.toggleDisableDevice);
  const groupInputBusy = useStore((s) => s.groupInputBusy);
  const setGroupInputBusy = useStore((s) => s.setGroupInputBusy);
  const frame = useStore((s) => s.streamFrames[device.serial]);
  const status = useStore((s) => s.streamStatus[device.serial]);
  const cmds = useAdbCommands();
  const imgRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imgElementRef = useRef<HTMLImageElement>(null);
  const [copied, setCopied] = useState(false);
  const [tapMarker, setTapMarker] = useState<{ x: number; y: number } | null>(null);

  const isOnline = device.status === 'online';

  // Register/unregister canvas for direct VideoFrame rendering
  useEffect(() => {
    if (canvasRef.current) {
      registerCanvas(device.serial, canvasRef.current);
    }
    return () => unregisterCanvas(device.serial);
  }, [device.serial]);

  const handleSelect = useCallback(() => {
    if (!isOnline) return;
    const wasSelected = selected;
    toggleSelect(device.serial);
    if (!wasSelected) {
      cmds.startStream(device.serial, device.server_host, device.server_port, streamOptions);
    } else {
      cmds.stopStream(device.serial);
    }
  }, [device.serial, device.server_host, device.server_port, selected, isOnline, streamOptions, cmds, toggleSelect]);

  // Compute image-space coordinates from a mouse event on the screen div.
  // Works with both <canvas> (stream) and <img> (screenshot fallback).
  const showTapMarker = useCallback((x: number, y: number) => {
    setTapMarker({ x, y });
    window.setTimeout(() => setTapMarker(null), 650);
  }, []);

  const mapCoordinatesFromClient = useCallback((clientX: number, clientY: number) => {
    const el = imgRef.current;
    if (!el) {
      return null;
    }
    const rect = el.getBoundingClientRect();
    const containerX = clientX - rect.left;
    const containerY = clientY - rect.top;

    let x = containerX;
    let y = containerY;
    let sourceWidth = rect.width;
    let sourceHeight = rect.height;

    // Try canvas first (stream), then img (screenshot)
    let naturalW = 0;
    let naturalH = 0;
    if (canvasRef.current && canvasRef.current.width > 0 && canvasRef.current.height > 0) {
      naturalW = canvasRef.current.width;
      naturalH = canvasRef.current.height;
    } else if (imgElementRef.current && imgElementRef.current.naturalWidth > 0 && imgElementRef.current.naturalHeight > 0) {
      naturalW = imgElementRef.current.naturalWidth;
      naturalH = imgElementRef.current.naturalHeight;
    }

    if (naturalW > 0 && naturalH > 0) {
      const layout = getImageLayout(rect.width, rect.height, naturalW, naturalH);
      x = containerX - layout.offsetX;
      y = containerY - layout.offsetY;
      sourceWidth = layout.displayWidth;
      sourceHeight = layout.displayHeight;
      x = Math.max(0, Math.min(x, sourceWidth));
      y = Math.max(0, Math.min(y, sourceHeight));
    }

    return {
      x,
      y,
      sourceWidth: Math.round(sourceWidth),
      sourceHeight: Math.round(sourceHeight),
      markerX: Math.max(0, Math.min(containerX, rect.width)),
      markerY: Math.max(0, Math.min(containerY, rect.height)),
    };
  }, []);

  const mapCoordinates = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    return mapCoordinatesFromClient(e.clientX, e.clientY);
  }, [mapCoordinatesFromClient]);

  const swipeStart = useRef<{ clientX: number; clientY: number } | null>(null);
  const handleMouseDown = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    if (!isOnline || groupInputBusy) return;
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    swipeStart.current = { clientX: e.clientX, clientY: e.clientY };
  }, [isOnline, groupInputBusy]);

  const handleMouseUp = useCallback(
    async (e: React.MouseEvent<HTMLDivElement>) => {
      if (!swipeStart.current || !isOnline || groupInputBusy) return;
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();

      const dx = e.clientX - swipeStart.current.clientX;
      const dy = e.clientY - swipeStart.current.clientY;
      const dist = Math.sqrt(dx * dx + dy * dy);

      if (dist > 10) {
        const rect = e.currentTarget.getBoundingClientRect();
        const containerX1 = swipeStart.current.clientX - rect.left;
        const containerY1 = swipeStart.current.clientY - rect.top;
        const containerX2 = e.clientX - rect.left;
        const containerY2 = e.clientY - rect.top;

        let x1 = containerX1;
        let y1 = containerY1;
        let x2 = containerX2;
        let y2 = containerY2;
        let sourceWidth = rect.width;
        let sourceHeight = rect.height;

        let naturalW = 0;
        let naturalH = 0;
        if (canvasRef.current && canvasRef.current.width > 0 && canvasRef.current.height > 0) {
          naturalW = canvasRef.current.width;
          naturalH = canvasRef.current.height;
        } else if (imgElementRef.current && imgElementRef.current.naturalWidth > 0 && imgElementRef.current.naturalHeight > 0) {
          naturalW = imgElementRef.current.naturalWidth;
          naturalH = imgElementRef.current.naturalHeight;
        }

        if (naturalW > 0 && naturalH > 0) {
          const layout = getImageLayout(rect.width, rect.height, naturalW, naturalH);
          x1 = containerX1 - layout.offsetX;
          y1 = containerY1 - layout.offsetY;
          x2 = containerX2 - layout.offsetX;
          y2 = containerY2 - layout.offsetY;
          sourceWidth = layout.displayWidth;
          sourceHeight = layout.displayHeight;
          x1 = Math.max(0, Math.min(x1, sourceWidth));
          y1 = Math.max(0, Math.min(y1, sourceHeight));
          x2 = Math.max(0, Math.min(x2, sourceWidth));
          y2 = Math.max(0, Math.min(y2, sourceHeight));
        }

        sourceWidth = Math.round(sourceWidth);
        sourceHeight = Math.round(sourceHeight);

        try {
          setGroupInputBusy(true);
          const results = selected
            ? await cmds.swipeDevices(x1, y1, x2, y2, 300, sourceWidth, sourceHeight, device.serial)
            : await cmds.swipeDevice(device, x1, y1, x2, y2, 300, sourceWidth, sourceHeight);
          const failed = results.filter(r => !r.success);
          if (failed.length > 0) console.error('Swipe failed:', failed);
        } catch (err) {
          console.error('Swipe error:', err);
        } finally {
          setGroupInputBusy(false);
        }
      } else {
        const mapped = mapCoordinatesFromClient(
          swipeStart.current.clientX,
          swipeStart.current.clientY
        );
        if (!mapped) {
          swipeStart.current = null;
          return;
        }
        const { x, y, sourceWidth, sourceHeight, markerX, markerY } = mapped;
        showTapMarker(markerX, markerY);
        try {
          setGroupInputBusy(true);
          const results = selected
            ? await cmds.tapDevices(x, y, sourceWidth, sourceHeight, device.serial)
            : await cmds.tapDevice(device, x, y, sourceWidth, sourceHeight);
          const failed = results.filter(r => !r.success);
          if (failed.length > 0) console.error('Tap failed:', failed);
        } catch (err) {
          console.error('Tap error:', err);
        } finally {
          setGroupInputBusy(false);
        }
      }
      swipeStart.current = null;
    },
    [
      isOnline,
      groupInputBusy,
      selected,
      device,
      cmds,
      mapCoordinates,
      mapCoordinatesFromClient,
      showTapMarker,
      setGroupInputBusy,
    ]
  );

  const handleCopySerial = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(device.serial);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      console.error('Failed to copy:', err);
    }
  }, [device.serial]);

  const statusClass = styles[`status_${device.status}`] ?? styles.status_offline;
  const streamState = status?.status;
  const streamUnavailable = !!streamState && streamState !== 'connected' && streamState !== 'receiving';
  const hasStream = !!frame && !streamUnavailable;
  const showStreamStatus = streamUnavailable || (!hasStream && !screenshot);
  const streamStatusText = status?.error
    ? status.error
    : isOnline
      ? streamState === 'receiving'
        ? 'Receiving...'
        : streamState === 'connected'
          ? 'Connected...'
          : streamState === 'disconnected'
            ? 'Stream disconnected'
            : streamState === 'reconnecting'
              ? 'Reconnecting...'
              : streamState === 'stopped'
                ? 'Stream stopped'
                : 'Starting...'
      : device.status;

  return (
    <div className={`${styles.card} ${selected ? styles.cardSelected : ''}`}>
      {/* Header */}
      <div className={styles.header} onClick={handleSelect}>
        <div className={`${styles.statusDot} ${statusClass}`} />
        <span className={styles.name}>{device.model || device.serial}</span>
        {device.battery >= 0 && (
          <span className={styles.battery}>{device.battery}%</span>
        )}
      </div>

      {/* Screen */}
      <div
        ref={imgRef}
        className={`${styles.screen} ${isOnline ? styles.screenActive : ''}`}
        onMouseDown={handleMouseDown}
        onMouseUp={handleMouseUp}
        onDragStart={(e) => e.preventDefault()}
      >
        {tapMarker && (
          <div
            className={styles.tapMarker}
            style={{ left: tapMarker.x, top: tapMarker.y }}
          />
        )}
        {/* Canvas for WebCodecs stream — always mounted for registration, hidden when no frames */}
        <canvas
          ref={canvasRef}
          className={styles.img}
          style={{ display: hasStream ? 'block' : 'none', pointerEvents: 'none' }}
        />
        {!hasStream && screenshot && !streamUnavailable ? (
          <img
            ref={imgElementRef}
            src={screenshot}
            className={styles.img}
            alt="screen"
            draggable={false}
            style={{ pointerEvents: 'none' }}
          />
        ) : showStreamStatus ? (
          <div className={`${styles.placeholder} ${streamUnavailable ? styles.streamDisconnected : ''}`}>
            <div style={{ fontSize: 12, opacity: 0.85 }}>
              {status?.status ? `Stream: ${status.status}` : 'Stream: (none)'}
            </div>
            <div className={status?.error ? styles.streamErrorText : undefined}>
              {streamStatusText}
            </div>
          </div>
        ) : null}
      </div>

      {/* Footer */}
      <div className={styles.footer}>
        <span
          className={styles.serial}
          onClick={handleCopySerial}
          title={copied ? 'Copied!' : 'Click to copy'}
          style={{ cursor: 'pointer' }}
        >
          {copied ? '✓ Copied' : device.serial}
        </span>
        <div className={styles.footerActions}>
          <button
            className={styles.disableBtn}
            onClick={(e) => { e.stopPropagation(); toggleDisableDevice(device.serial); }}
            title="Disable device"
          >
            ✕
          </button>
          {isOnline && (
            <button
              className={styles.scrcpyBtn}
              onClick={(e) => { e.stopPropagation(); cmds.launchScrcpy(device.serial, device.server_host, device.server_port); }}
              title="Open in scrcpy"
            >
              ▶
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

export const DeviceCard = React.memo(DeviceCardInner);
