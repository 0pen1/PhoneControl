import { useEffect, useRef, useState, useCallback, useMemo } from 'react';
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
const GRID_FIT_SAFETY = 8;
const OVERVIEW_FIT_SAFETY = 14;
const PREFETCH_PAGE_RADIUS = 1;
const STREAM_START_BATCH_SIZE = 8;
const STREAM_START_BATCH_GAP_MS = 25;
const BACKGROUND_STREAM_DELAY_MS = 600;
const NATURAL_LAYOUT_MIN_SCALE_RATIO = 0.82;

function deviceStreamKey(d: { serial: string; server_host: string; server_port: number }) {
  return `${d.server_host}:${d.server_port}:${d.serial}`;
}

function streamOptionsFor(count: number, overviewMode: boolean, fps: number) {
  if (overviewMode || count > 20) {
    return {
      max_size: 480,
      max_fps: Math.max(3, Math.min(fps, 5)),
      bit_rate: 900_000,
    };
  }
  if (count > 12) {
    return {
      max_size: 540,
      max_fps: Math.max(3, Math.min(fps, 8)),
      bit_rate: 1_400_000,
    };
  }
  return { max_size: 720, max_fps: fps, bit_rate: 4_000_000 };
}

function useCardLayout(
  containerRef: React.RefObject<HTMLDivElement | null>,
  count: number,
  overviewMode: boolean,
) {
  const [scale, setScale] = useState(1);
  const [columns, setColumns] = useState(1);

  const calcScale = useCallback(() => {
    if (count === 0 || !containerRef.current) {
      setScale(1);
      setColumns(1);
      return;
    }
    const rect = containerRef.current.getBoundingClientRect();
    const availW = rect.width - GRID_PADDING * 2 - GRID_FIT_SAFETY;
    const availH =
      rect.height
      - GRID_PADDING * 2
      - GRID_FIT_SAFETY
      - (overviewMode ? OVERVIEW_FIT_SAFETY : 0);
    if (availW <= 0 || availH <= 0) return;

    let maxScale = 0;
    const candidates: Array<{
      cols: number;
      rows: number;
      scale: number;
      widthUse: number;
      heightUse: number;
      areaUse: number;
      lastRowFill: number;
    }> = [];

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
      const cardW = screenH * SCREEN_ASPECT;
      const cardH = FIXED_HEADER_HEIGHT + screenH + FIXED_FOOTER_HEIGHT;
      const usedW = cardW * cols + GRID_GAP * (cols - 1);
      const usedH = cardH * rows + GRID_GAP * (rows - 1);
      const widthUse = Math.min(1, usedW / availW);
      const heightUse = Math.min(1, usedH / availH);
      const lastRow = count % cols || cols;
      maxScale = Math.max(maxScale, s);
      candidates.push({
        cols,
        rows,
        scale: s,
        widthUse,
        heightUse,
        areaUse: widthUse * heightUse,
        lastRowFill: lastRow / cols,
      });
    }

    const minNaturalScale = maxScale * NATURAL_LAYOUT_MIN_SCALE_RATIO;
    const best = candidates
      .filter((c) => c.scale >= minNaturalScale)
      .sort((a, b) => {
        const score = (c: typeof a) => {
          const sparsePenalty = c.lastRowFill < 0.6 ? (0.6 - c.lastRowFill) * 3 : 0;
          return (
            c.areaUse * 4
            + c.widthUse * 0.7
            + c.heightUse * 0.7
            + c.lastRowFill * 0.6
            + (c.scale / maxScale) * 1.2
            - sparsePenalty
          );
        };
        return score(b) - score(a);
      })[0] ?? candidates[0];

    setScale(Math.max(best?.scale ?? 1, 0.1));
    setColumns(best?.cols ?? 1);
  }, [count, containerRef, overviewMode]);

  useEffect(() => {
    calcScale();
    const ro = new ResizeObserver(calcScale);
    if (containerRef.current) ro.observe(containerRef.current);
    return () => ro.disconnect();
  }, [calcScale, containerRef]);

  return { scale, columns };
}

export function DeviceGrid() {
  const devices = useStore((s) => s.devices);
  const disabledSerials = useStore((s) => s.disabledSerials);
  const selectedSerials = useStore((s) => s.selectedSerials);
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
  const { scale, columns } = useCardLayout(gridRef, scaleCount, overviewMode);
  const enabledOnlineDevices = enabledDevices.filter((d) => d.status === 'online');
  const currentOnlineDevices = pageDevices.filter((d) => d.status === 'online');
  const activeStreamCount = overviewMode
    ? enabledOnlineDevices.length
    : currentOnlineDevices.length;
  const streamOptions = useMemo(
    () => streamOptionsFor(activeStreamCount, overviewMode, fps),
    [activeStreamCount, overviewMode, fps]
  );

  // Track streams that have actually been started and kept warm.
  const warmedStreamsRef = useRef<Map<string, string>>(new Map());

  useEffect(() => {
    return () => {
      for (const [key, serial] of warmedStreamsRef.current) {
        const [serverHost, serverPort] = key.split(':');
        cmds.stopStream(serial, serverHost, Number(serverPort)).catch(() => {});
      }
      warmedStreamsRef.current.clear();
    };
  }, [cmds]);

  useEffect(() => {
    const currentStreamKeySet = new Set(currentOnlineDevices.map(deviceStreamKey));
    const prefetchDevices = overviewMode
      ? []
      : enabledDevices
          .slice(
            Math.max(0, (page - PREFETCH_PAGE_RADIUS) * pageSize),
            Math.min(enabledDevices.length, (page + PREFETCH_PAGE_RADIUS + 1) * pageSize),
          )
          .filter((d) => d.status === 'online' && !currentStreamKeySet.has(deviceStreamKey(d)));
    const backgroundDevices = prefetchDevices;
    const targetDevices = [...currentOnlineDevices, ...backgroundDevices];
    const currentStreamKeys = new Set(targetDevices.map(deviceStreamKey));
    const warmed = warmedStreamsRef.current;
    let cancelled = false;
    const startTimers: number[] = [];

    // Stop streams only when the device is no longer eligible. Pagination
    // should not tear down scrcpy; it only changes the WS subscription.
    for (const [key, serial] of Array.from(warmed)) {
      if (!currentStreamKeys.has(key)) {
        const [serverHost, serverPort] = key.split(':');
        cmds.stopStream(serial, serverHost, Number(serverPort)).catch(() => {});
        warmed.delete(key);
      }
    }

    const startDevice = (d: typeof targetDevices[number]) => {
      const key = deviceStreamKey(d);
      if (warmedStreamsRef.current.has(key)) return;
      warmedStreamsRef.current.set(key, d.serial);
      cmds.startStream(d.serial, d.server_host, d.server_port, streamOptions).catch(() => {
        warmedStreamsRef.current.delete(key);
      });
    };

    const newCurrentDevices = currentOnlineDevices.filter((d) => !warmed.has(deviceStreamKey(d)));
    const newBackgroundDevices = backgroundDevices.filter((d) => !warmed.has(deviceStreamKey(d)));

    const scheduleStarts = (
      devicesToStart: typeof targetDevices,
      initialDelay: number,
    ) => {
      devicesToStart.forEach((d, index) => {
        const batch = Math.floor(index / STREAM_START_BATCH_SIZE);
        const timer = window.setTimeout(() => {
          if (!cancelled) startDevice(d);
        }, initialDelay + batch * STREAM_START_BATCH_GAP_MS);
        startTimers.push(timer);
      });
    };

    // Start visible streams first. Adjacent-page prefetch is delayed and
    // batched so a large device list does not stampede the ADB server.
    scheduleStarts(newCurrentDevices, 0);
    scheduleStarts(newBackgroundDevices, BACKGROUND_STREAM_DELAY_MS);

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
      for (const timer of startTimers) window.clearTimeout(timer);
      window.clearTimeout(wakeTimer);
    };
  }, [
    page,
    overviewMode,
    enabledDevices.map((d) => `${deviceStreamKey(d)}:${d.status}`).join(','),
    fps,
    pageSize,
    streamOptions.max_size,
    streamOptions.max_fps,
    streamOptions.bit_rate,
    cmds,
    currentOnlineDevices.map(deviceStreamKey).join(','),
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
  const cardW = Math.floor(screenH * SCREEN_ASPECT);
  const cardTotalH = FIXED_HEADER_HEIGHT + screenH + FIXED_FOOTER_HEIGHT;

  const gridStyle = {
    '--card-width': `${cardW}px`,
    '--card-height': `${screenH}px`,
    '--card-total-height': `${cardTotalH}px`,
    '--grid-columns': columns,
    fontSize: `${scale * 100}%`,
  } as React.CSSProperties;

  return (
    <div className={styles.wrapper}>
      <div
        ref={gridRef}
        className={`${styles.grid} ${overviewMode ? styles.gridOverview : ''}`}
        style={gridStyle}
      >
        {pageDevices.map((device) => (
          <div key={device.serial}>
            <DeviceCard
              device={device}
              selected={selectedSerials.has(device.serial)}
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
