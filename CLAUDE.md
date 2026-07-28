# CLAUDE.md

本文件为 Claude Code 提供在本仓库协作所需的核心上下文。**面向 AI 阅读，请保持简洁、准确、可执行。**

## 项目概述

**Phone Control** — 多设备 Android 群控桌面应用。支持多 ADB 服务器管理、实时 scrcpy 投屏预览、批量点击/滑动/文本/按键广播、单设备直接交互。

技术栈：**Tauri 2 + React 19 + TypeScript + Zustand**（前端）/ **Rust + Tokio**（后端）。设计目标是**扩展到 50~100+ 台设备**，因此后端一切 IO 尽量并行（`tokio::spawn`），并用信号量限流。

## 常用命令

```bash
npm run tauri dev      # 开发（Vite:1420 + Tauri，Rust 错误直出）
npm run tauri build    # 生产构建（产物见 src-tauri/target/release/bundle/）
npm test               # 前端单测（vitest，配置内联在 vite.config.ts）
npm run test:watch     # 前端单测 watch
```

Rust 端（在 `src-tauri/` 下，或用 `--manifest-path src-tauri/Cargo.toml`）：

```bash
cargo check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml
```

改动 Rust 后**至少跑 `cargo check`**；改动前端 hook/util 后跑 `npm test`。

## 架构

### 后端 `src-tauri/src/`
- `lib.rs` — 所有 `#[tauri::command]` 定义 + `invoke_handler` 注册 + `setup`（启动 WS server 和设备轮询循环）。**新增前端可调用的命令必须在此处的 `generate_handler!` 里注册。**
- `state.rs` — `AppState`：`servers`、`stream_tokens`、`control_sockets`、`adb_semaphore`。`ADB_PERMITS = 12` 限制全局并发 adb 调用，防止设备多时 adb 打爆。
- `config.rs` — 服务器配置持久化到 `~/.phone_control/servers.json`。
- `ws.rs` — 本地 WebSocket server（`127.0.0.1:32199`），向前端广播 H.264 帧。帧头 `packet_type`：0=config(SPS/PPS) / 1=keyframe / 2=delta，用 `bytes::Bytes` 零拷贝广播。
- `adb/` 子模块：
  - `binaries.rs` — 定位 `adb`/`scrcpy` 可执行文件（打包成 `.app` 后 GUI 进程 PATH 极简，不能靠裸名 spawn；解析顺序：`ADB_PATH`/`SCRCPY_PATH` 环境变量 → PATH → 常见安装路径 → 裸名兜底）。**任何 spawn adb/scrcpy 都应走这里，不要 `Command::new("adb")`。**
  - `commands.rs` — tap/swipe/text/keyevent，含坐标缩放逻辑。
  - `server.rs` — ADB 服务器轮询、`fetch_device_info`（并发跑 `wm size` / `getprop` / `dumpsys battery`）。
  - `scrcpy_client.rs` — 异步启动 scrcpy server 并连接（并行拉起多设备）；scrcpy 本地端口范围 `32200..=39999`（防端口碰撞）。
  - `scrcpy_control.rs` — 通过 scrcpy control socket 下发点击/滑动（群控走此路，比逐台 adb 快）。
  - `stream.rs` — H.264 视频流主循环、`StreamTokens`（可取消）、`ControlSockets`。
  - `device.rs` — 设备结构体与 `adb devices` 输出解析。

### 前端 `src/`
- `store/index.ts` — Zustand 全局状态（servers / devices / 选中集 / 禁用集 / 流状态 / 每设备帧尺寸）。
- `hooks/` — `useDevices`（拉设备）、`useStream`（**WebCodecs `VideoDecoder` 解 H.264 → canvas**）、`useStreamEvents`、`useAdbCommands`（群控命令）。
- `components/` — `Sidebar/`（服务器列表、FPS 滑块、设备列表）、`DeviceGrid/`（分页网格 + 全屏预览 + 设备卡片 `<canvas>`）、`Toolbar/`（文本/Shell 模式、按键）。
- `utils/` — `h264Utils.ts`（NAL 解析）、`imageLayout.ts`（`object-fit: contain` 布局与坐标映射，有单测）、`canvasRegistry.ts`。

### 视频流管线（关键，现状 ≠ 部分旧文档）
scrcpy server → **原始 H.264 NAL** → WebSocket(`:32199`) → 前端 **WebCodecs 硬解** → `<canvas>`。
⚠️ README/CHANGELOG 里描述的「Rust 端解 PNG→缩小→JPEG」是**已被 P0 优化替换的旧路径**（见 `OPTIMIZATION.md`，全部标记 DONE）。以代码为准。

### 坐标映射（两级）
1. 前端：处理 `object-fit: contain` 的显示偏移，得到设备坐标（`imageLayout.ts` / `DeviceCard.tsx`，浮点需 `Math.round()` 再传给后端，否则 Rust `u32` 校验失败）。
2. 后端：按设备真实分辨率（`wm size`）等比缩放（`commands.rs`）。
> 历史坑：`fetch_device_info` 未取到真实分辨率会导致 `screen_width/height=0`，坐标全部落在左上角。

## 约定与注意事项
- **并发优先**：新增多设备操作用 `tokio::spawn` 并行，别用 `.iter().map()` 串行；受 `adb_semaphore` 限流。
- **群控走 scrcpy control socket**（`scrcpy_control.rs`）而非逐台 adb，延迟从 O(N) 到 O(1)。
- 前端 ↔ 后端：命令用 Tauri `invoke`，事件用 Tauri events，**高频视频帧走独立 WebSocket**（不走 Tauri IPC）。
- 提交/发布流程见 `RELEASES.md`；进度记录见 `CHANGELOG.md`。
- 当前工作分支 `main` 有针对多设备流稳定性/进程清理的未提交改动。

## 环境要求
Node ≥ 18、Rust ≥ 1.70、`adb`、`scrcpy`（macOS 上通常 Homebrew 安装于 `/opt/homebrew/bin`）。测试常用 `emulator-5554`。
