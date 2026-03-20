![截屏2026-03-20 14.17.22](https://github.com/0pen1/PhoneControl/blob/main/docs/img/%E6%88%AA%E5%B1%8F2026-03-20%2014.17.22.png)

# Phone Control

多设备 Android 群控桌面应用，支持实时截屏预览、批量操作、scrcpy 投屏。基于 **Tauri 2 + React + TypeScript** 构建。

## 功能

- **多 ADB 服务器管理** — 支持本地和远程 ADB 服务器，配置持久化
- **实时截屏预览** — 当前页设备自动截屏刷新，FPS 可调（1-30）
- **批量控制** — 点击、滑动、文本输入、按键事件广播到所有选中设备，自动坐标缩放
- **scrcpy 集成** — 一键启动 scrcpy 投屏，支持远程 ADB 服务器
- **ADB Shell** — 对选中设备执行 shell 命令，逐设备显示输出
- **分页显示** — 每页设备数可选（6/8/10/12/16/20/24），自动管理预览
- **设备筛选** — 按设备 ID 或型号实时过滤
- **深色主题** — 现代化深色 UI

## 技术栈

| 层级 | 技术 |
|------|------|
| 后端 | Rust, Tauri 2.x, Tokio |
| 前端 | React 19, TypeScript, Zustand, Vite |
| 通信 | Tauri Commands + Events |
| 样式 | CSS Modules + Custom Properties |

## 项目结构

```
phone-control/
├── src-tauri/
│   └── src/
│       ├── lib.rs               # Tauri 命令、应用入口
│       ├── state.rs             # AppState
│       ├── config.rs            # 配置持久化
│       └── adb/
│           ├── device.rs        # 设备结构、ADB 输出解析
│           ├── server.rs        # ADB 服务器轮询
│           ├── commands.rs      # tap/swipe/text/keyevent + 坐标缩放
│           └── screenshot.rs    # 异步截屏循环（JPEG 压缩）
├── src/
│   ├── App.tsx
│   ├── store/index.ts           # Zustand 状态管理
│   ├── hooks/                   # useDevices, useScreenshot, useAdbCommands
│   ├── components/
│   │   ├── Sidebar/             # 服务器列表、FPS 滑块、设备列表
│   │   ├── DeviceGrid/          # 设备网格（分页）、设备卡片
│   │   └── Toolbar/             # 文本/Shell 模式、按键按钮
│   └── types/index.ts
├── package.json
└── vite.config.ts
```

## 环境要求

- [Node.js](https://nodejs.org/) >= 18
- [Rust](https://www.rust-lang.org/tools/install) >= 1.70
- [ADB](https://developer.android.com/tools/adb)
- [scrcpy](https://github.com/Genymobile/scrcpy)（可选，用于投屏）

## 快速开始

```bash
# 安装依赖
cd phone-control
npm install

# 开发模式
npm run tauri dev

# 构建生产版本
npm run tauri build
```

构建产物：
- **macOS**: `src-tauri/target/release/bundle/macos/phone-control.app`
- **DMG**: `src-tauri/target/release/bundle/dmg/phone-control_0.1.0_x64.dmg`

## 使用说明

1. 在左侧添加 ADB 服务器（默认 `127.0.0.1:5037`）
2. 设备自动发现，每 3 秒轮询
3. 当前页设备自动开始截屏预览
4. 点击设备或使用 All/None 按钮选择设备
5. 底部工具栏发送文本、按键或 Shell 命令
6. 点击 ▶ 按钮启动 scrcpy 投屏
7. 使用搜索框按 ID 筛选设备
8. 底部 Per page 调整每页显示数量

## 配置文件

服务器配置保存在 `~/.phone_control/servers.json`：

```json
{
  "servers": [
    { "host": "127.0.0.1", "port": 5037, "enabled": true }
  ]
}
```

## 架构说明

- 截屏在 Rust 端解码 PNG → 缩小至 360px → 重编码 JPEG（~30-60KB），防止 WebView 内存溢出
- 坐标缩放在 Rust 端按目标设备分辨率自动计算
- 设备轮询完全在 Rust 后台任务运行，前端通过 Tauri 事件被动接收

## License

MIT
