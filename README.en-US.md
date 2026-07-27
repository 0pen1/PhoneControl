![截屏2026-03-20 14.17.22](https://github.com/0pen1/PhoneControl/blob/main/docs/img/%E6%88%AA%E5%B1%8F2026-03-20%2014.17.22.png)

# Phone Control

A multi-device Android group control desktop application supporting real-time screenshot previews, batch operations, and scrcpy mirroring. Built with **Tauri 2 + React + TypeScript**.

## Features

- **Multi-ADB Server Management** — Supports local and remote ADB servers with persistent configuration; servers can be enabled/disabled.
- **Real-time Screenshot Preview** — Automatic screenshot refreshing for devices on the current page, with adjustable FPS (1-30).
- **Batch Control** — Broadcast clicks, swipes, text input, and key events to all selected devices with automatic coordinate scaling.
- **Direct Single-Device Interaction** — Interact via click/swipe on unselected device cards without affecting the group control selection.
- **Full-Screen Preview Mode** — Switch with one click to display all devices scaled down on a single page, eliminating the need for pagination scrolling.
- **Device Management** — Enable/disable devices via the left sidebar or right-side cards; disabled devices are hidden from the mirror list.
- **Device ID Copy** — Quickly copy device IDs to the clipboard with a click.
- **scrcpy Integration** — Launch scrcpy mirroring with one click, supporting remote ADB servers.
- **ADB Shell** — Execute shell commands on selected devices with per-device output display.
- **Paginated Display** — Configurable devices per page (6/8/10/12/14/16/20/24), defaulting to 14, with automatic preview management.
- **Device Info Display** — Automatically retrieves device model and battery level.
- **Device Filtering** — Real-time filtering by device ID.
- **Auto-Refresh** — Automatically refreshes the device list upon app startup and server state changes.
- **Dark Theme** — Modern dark UI.

## Tech Stack

| Layer | Technology |
|------|------|
| Backend | Rust, Tauri 2.x, Tokio |
| Frontend | React 19, TypeScript, Zustand, Vite |
| Communication | Tauri Commands + Events |
| Styling | CSS Modules + Custom Properties |

## Project Structure

```
phone-control/
├── src-tauri/
│   └── src/
│       ├── lib.rs               # Tauri commands, app entry point
│       ├── state.rs             # AppState
│       ├── config.rs            # Configuration persistence
│       └── adb/
│           ├── device.rs        # Device structures, ADB output parsing
│           ├── server.rs        # ADB server polling
│           ├── commands.rs      # tap/swipe/text/keyevent + coordinate scaling
│           └── screenshot.rs    # Async screenshot loop (JPEG compression)
├── src/
│   ├── App.tsx
│   ├── store/index.ts           # Zustand state management
│   ├── hooks/                   # useDevices, useScreenshot, useAdbCommands
│   ├── components/
│   │   ├── Sidebar/             # Server list, FPS slider, device list
│   │   ├── DeviceGrid/          # Device grid (pagination + full-screen), device cards
│   │   └── Toolbar/             # Text/Shell mode, key buttons
│   └── types/index.ts
├── package.json
└── vite.config.ts
```

## Requirements

- [Node.js](https://nodejs.org/) >= 18
- [Rust](https://www.rust-lang.org/tools/install) >= 1.70
- [ADB](https://developer.android.com/tools/adb)
- [scrcpy](https://github.com/Genymobile/scrcpy) (Optional, for mirroring)

## Quick Start

```bash
# Install dependencies
cd phone-control
npm install

# Development mode
npm run tauri dev

# Build production version
npm run tauri build
```

Build artifacts:
- **macOS**: `src-tauri/target/release/bundle/macos/phone-control.app`
- **DMG**: `src-tauri/target/release/bundle/dmg/phone-control_0.1.0_x64.dmg`

## Usage Guide

### Adding ADB Servers
1. Enter the server address and port (default `127.0.0.1:5037`) in the "ADB Servers" area on the left.
2. Click "+" to add the server.
3. Click the server status indicator to enable/disable the server.
4. The device list refreshes automatically when switching server states.

### Device Management
- **Left Device List**:
  - Automatically refreshes devices upon app startup.
  - Click a device to select/deselect.
  - Click the status indicator to disable/enable a device (disabled devices are hidden from the mirror list).
  - Click the ▶ button to launch a standalone scrcpy mirror.
  - Filter by device ID using the search box.
  - Click the ↻ button for a manual refresh.

- **Right Device Cards**:
  - Click the card header to select the device and start mirroring.
  - Interact with the mirror screen via clicks and swipes.
  - Click the device ID to copy it to the clipboard.
  - Click the ✕ button to disable the device.
  - Click the ▶ button to launch a standalone scrcpy mirror.
  - Adjust "Per page" at the bottom to change the number of displayed devices (default 14).
  - Click the ⊞ button at the bottom to enter Full-Screen Preview mode, where all devices are automatically scaled to fit one page.

### Batch Operations
1. Use the "All" button to select all online devices.
2. Use the "None" button to clear all selections.
3. Use the bottom toolbar to send text, key events, or Shell commands to all selected devices.

## Configuration File

Server configurations are saved in `~/.phone_control/servers.json`:

```json
{
  "servers": [
    { "host": "127.0.0.1", "port": 5037, "enabled": true }
  ]
}
```

## Architecture Notes

- Screenshots are decoded as PNG on the Rust side $\rightarrow$ resized to 360px $\rightarrow$ re-encoded as JPEG (~30-60KB) to prevent WebView memory overflow.
- Coordinate mapping uses a two-stage conversion: the frontend handles `object-fit: contain` offset calculations, and the backend scales coordinates proportionally based on the device's actual resolution.
- Device polling runs entirely as Rust background tasks, retrieving real device information via `wm size`/`getprop`/`dumpsys battery`; the frontend receives updates passively via Tauri events.
- Full-screen preview mode utilizes `ResizeObserver` to monitor container size changes, dynamically calculating the optimal number of columns and scaling ratio.

## License

MIT
