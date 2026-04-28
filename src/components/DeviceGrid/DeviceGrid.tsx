import { useEffect, useRef, useState, useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useStore } from '../../store';
import { useAdbCommands } from '../../hooks/useAdbCommands';
import { DeviceCard } from './DeviceCard';
import styles from './DeviceGrid.module.css';

const SCREEN_ASPECT = 9 / 20; // typical phone width:height
const SCREEN_BASE_HEIGHT = 356;
const FIXED_HEADER_HEIGHT = 33;
const FIXED_FOOTER_HEIGHT = 30;
const GRID_GAP = 10;
const GRID_PADDING = 12;

function streamOptionsFor(count: number, overviewMode: boolean, fps: number) {
  if (overviewMode || count > 20) {
    return {
      max_size: 360,
      max_fps: Math.max(1, Math.min(fps, 2)),
      bit_rate: 400_000,
    };
  }
  if (count > 12) {
    return {
      max_size: 480,
      max_fps: Math.max(1, Math.min(fps, 4)),
      bit_rate: 900_000,
    };
  }
  return { max_size: 720, max_fps: fps, bit_rate: 4_000_000 };
}

function useAutoScale(
  containerRef: React.RefObject<HTMLDivElement | null>,
  count: number,
) {
  const [scale, setScale] = useState(1);

  const calcScale = useCallback(() => {
    if (count === 0 || !containerRef.current) {
      setScale(1);
      return;
    }
    const rect = containerRef.current.getBoundingClientRect();
    const availW = rect.width - GRID_PADDING * 2;
    const availH = rect.height - GRID_PADDING * 2;
    if (availW <= 0 || availH <= 0) return;

    // Try different column counts, pick the one that uses the largest scale
    // while fitting all cards without overflow.
    // Card width = SCREEN_BASE_HEIGHT * scale * SCREEN_ASPECT
    // Card height = FIXED_HEADER_HEIGHT + SCREEN_BASE_HEIGHT * scale + FIXED_FOOTER_HEIGHT
    let best = 0;
    for (let cols = 1; cols <= count; cols++) {
      const rows = Math.ceil(count / cols);
      const maxCardW = (availW - GRID_GAP * (cols - 1)) / cols;
      const maxCardH = (availH - GRID_GAP * (rows - 1)) / rows;
      // Screen height limited by card width (via aspect ratio)
      const maxScreenFromW = maxCardW / SCREEN_ASPECT;
      // Screen height limited by card height (minus fixed header/footer)
      const maxScreenFromH = maxCardH - FIXED_HEADER_HEIGHT - FIXED_FOOTER_HEIGHT;
      const screenH = Math.min(maxScreenFromW, maxScreenFromH);
      if (screenH <= 0) continue;
      const s = screenH / SCREEN_BASE_HEIGHT;
      if (s > best) best = s;
    }
    setScale(Math.max(best, 0.1));
  }, [count, containerRef]);

  useEffect(() => {
    calcScale();
    const ro = new ResizeObserver(calcScale);
    if (containerRef.current) ro.observe(containerRef.current);
    return () => ro.disconnect();
  }, [calcScale, containerRef]);

  return scale;
}

export function DeviceGrid() {
  const devices = useStore((s) => s.devices);
  const disabledSerials = useStore((s) => s.disabledSerials);
  const selectedSerials = useStore((s) => s.selectedSerials);
  const screenshots = useStore((s) => s.screenshots);
  const page = useStore((s) => s.page);
  const pageSize = useStore((s) => s.pageSize);
  const setPage = useStore((s) => s.setPage);
  const fps = useStore((s) => s.fps);
  const overviewMode = useStore((s) => s.overviewMode);
  const setOverviewMode = useStore((s) => s.setOverviewMode);

  const setPageSize = useStore((s) => s.setPageSize);
  const cmds = useAdbCommands();

  const enabledDevices = devices.filter((d) => !disabledSerials.has(d.serial));
  const totalPages = Math.max(1, Math.ceil(enabledDevices.length / pageSize));
  const pageDevices = overviewMode
    ? enabledDevices
    : enabledDevices.slice(page * pageSize, (page + 1) * pageSize);

  const gridRef = useRef<HTMLDivElement>(null);
  const scaleCount = overviewMode ? enabledDevices.length : pageDevices.length;
  const scale = useAutoScale(gridRef, scaleCount);
  const currentOnlineCount = pageDevices.filter((d) => d.status === 'online').length;
  const streamOptions = useMemo(
    () => streamOptionsFor(currentOnlineCount, overviewMode, fps),
    [currentOnlineCount, overviewMode, fps]
  );

  // Track streams that have actually been started and kept warm.
  const warmedSerialsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    const currentOnlineDevices = pageDevices.filter((d) => d.status === 'online');
    const prefetchDevices = overviewMode
      ? []
      : enabledDevices
          .slice(
            Math.max(0, (page - 1) * pageSize),
            Math.min(enabledDevices.length, (page + 2) * pageSize),
          )
          .filter(
            (d) => d.status === 'online' && !currentOnlineDevices.some((cur) => cur.serial === d.serial),
          );
    const targetDevices = [...currentOnlineDevices, ...prefetchDevices];
    const currentSerials = new Set(targetDevices.map((d) => d.serial));
    const warmed = warmedSerialsRef.current;
    let cancelled = false;

    // Stop streams that are no longer on the current or adjacent pages.
    for (const serial of Array.from(warmed)) {
      if (!currentSerials.has(serial)) {
        invoke('stop_stream', { serial }).catch(() => {});
        warmed.delete(serial);
      }
    }

    const startDevice = (d: typeof targetDevices[number]) => {
      if (warmedSerialsRef.current.has(d.serial)) return;
      warmedSerialsRef.current.add(d.serial);
      invoke('start_stream', {
        serial: d.serial,
        serverHost: d.server_host,
        serverPort: d.server_port,
        options: streamOptions,
      }).catch(() => {
        warmedSerialsRef.current.delete(d.serial);
      });
    };

    const newCurrentDevices = currentOnlineDevices.filter((d) => !warmed.has(d.serial));
    const newPrefetchDevices = prefetchDevices.filter((d) => !warmed.has(d.serial));

    // Start visible streams first; prefetch neighbors shortly after so page
    // flips are warm without stealing the first ADB slots from the current page.
    for (const d of newCurrentDevices) startDevice(d);
    const prefetchTimer = window.setTimeout(() => {
      if (cancelled) return;
      for (const d of newPrefetchDevices) startDevice(d);
    }, 800);

    // Auto wake after stream startup has had a chance to claim ADB capacity.
    const wakeTimer = window.setTimeout(() => {
      if (cancelled || newCurrentDevices.length === 0) return;
      cmds.wakeUpDevices(
        newCurrentDevices.map((d) => ({
          serial: d.serial,
          width: d.screen_width,
          height: d.screen_height,
          server_host: d.server_host,
          server_port: d.server_port,
        }))
      ).catch(() => {});
    }, 1200);

    return () => {
      cancelled = true;
      window.clearTimeout(prefetchTimer);
      window.clearTimeout(wakeTimer);
    };
  }, [
    page,
    overviewMode,
    enabledDevices.map((d) => `${d.serial}:${d.status}`).join(','),
    fps,
    pageSize,
    streamOptions.max_size,
    streamOptions.max_fps,
    streamOptions.bit_rate,
    cmds,
  ]);

  if (devices.length === 0) {
    return (
      <div className={styles.empty}>
        <div className={styles.emptyIcon}>📱</div>
        <div className={styles.emptyText}>No devices connected</div>
        <div className={styles.emptyHint}>Add an ADB server in the sidebar to get started</div>
      </div>
    );
  }

  const screenH = SCREEN_BASE_HEIGHT * scale;
  const cardW = Math.round(screenH * SCREEN_ASPECT);
  const cardTotalH = FIXED_HEADER_HEIGHT + screenH + FIXED_FOOTER_HEIGHT;

  const cardStyle = {
    '--card-width': `${cardW}px`,
    '--card-height': `${screenH}px`,
    '--card-total-height': `${cardTotalH}px`,
    fontSize: `${scale * 100}%`,
  } as React.CSSProperties;

  return (
    <div className={styles.wrapper}>
      <div
        ref={gridRef}
        className={`${styles.grid} ${overviewMode ? styles.gridOverview : ''}`}
      >
        {pageDevices.map((device) => (
          <div key={device.serial} style={cardStyle}>
            <DeviceCard
              device={device}
              screenshot={screenshots[device.serial]}
              selected={selectedSerials.has(device.serial)}
              streamOptions={streamOptions}
            />
          </div>
        ))}
      </div>
      {devices.length > 0 && (
        <div className={styles.pagination}>
          {!overviewMode && (
            <div className={styles.pageSizeWrap}>
              <span className={styles.pageSizeLabel}>Per page</span>
              <input
                type="number"
                className={styles.pageSizeInput}
                value={pageSize}
                min={1}
                max={999}
                onChange={(e) => {
                  const v = parseInt(e.target.value, 10);
                  if (v > 0) setPageSize(v);
                }}
                onKeyDown={(e) => e.stopPropagation()}
              />
            </div>
          )}
          {!overviewMode && totalPages > 1 && (
            <>
              <button
                className={styles.pageBtn}
                disabled={page === 0}
                onClick={() => setPage(page - 1)}
              >
                ◀
              </button>
              <span className={styles.pageInfo}>
                {page + 1} / {totalPages}
              </span>
              <button
                className={styles.pageBtn}
                disabled={page >= totalPages - 1}
                onClick={() => setPage(page + 1)}
              >
                ▶
              </button>
            </>
          )}
          <button
            className={`${styles.overviewBtn} ${overviewMode ? styles.overviewActive : ''}`}
            onClick={() => setOverviewMode(!overviewMode)}
            title={overviewMode ? 'Exit overview' : 'Overview all devices'}
          >
            {overviewMode ? '⊟' : '⊞'}
          </button>
        </div>
      )}
    </div>
  );
}
