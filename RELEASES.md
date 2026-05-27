# Phone Control Releases

本文档记录面向使用者的版本发布说明。开发过程中的细节记录见 `CHANGELOG.md`。

## v0.2.1 - 2026-05-27

### 发布重点

- 修复多窗口或多视图同时打开时，同一设备 scrcpy 底层流被反复取消和重启导致的投屏闪动。
- 提高多设备投屏在短暂丢帧、WebSocket 重连和 decoder 重建时的画面稳定性。
- 修复打 tag 后 GitHub Actions 只上传 workflow artifact、不上传 GitHub Release 附件的问题。

### 修复与优化

- 后端同一 `serial + host + port` 的视频流改为复用单个 scrcpy 会话，并用 webview client lease 做引用计数，最后一个窗口释放后才停止底层流。
- 前端遇到可恢复的 H.264 序号跳变、decoder 重置或本地 WebSocket 重连时保留最后一帧，不再立即隐藏 canvas。
- `starting` 和 `reconnecting` 状态不再清空画面，只有 `disconnected` 或 `stopped` 才清除视频帧。
- WebSocket 发送队列容量提升，降低多路投屏瞬时背压造成的丢包概率。
- Release workflow 新增独立 `release` job，会在 tag 构建完成后统一下载产物并上传 `.dmg`、`.msi`、`.exe` 到 GitHub Release。

### 验证

本版本已通过以下检查：

```bash
npm test
npm run build
cargo test
```

---

## v0.2.0 - 2026-04-28

### 发布重点

- 优化多设备 scrcpy 视频流稳定性和断流重连速度。
- 提升批量点击 USB 模式弹窗时的触发成功率。
- 优化初次加载和翻页时的视频流启动速度。
- 改进断流时的前端状态展示，避免旧画面误导操作判断。

### 新增与优化

#### 视频流与重连

- 视频流 EOF 或读取错误后会立即向前端显示断开状态。
- 稳定运行超过 2 秒的视频流断开后，先等待 500ms 再重连，给 USB 模式切换和 ADB 重枚举留出缓冲。
- USB 模式切换后的 `device not found` 使用快速重试窗口，减少长时间卡在重连状态的情况。
- 重连成功后会重新建立 scrcpy server、TCP 视频流和 control socket。
- 前端在断流清帧后，新流首帧到达时会重新显示 canvas，修复 `Receiving...` 但黑屏的问题。

#### 批量点击与控制

- 批量点击优先使用 scrcpy control socket，减少 ADB `input tap` 的延迟和不稳定。
- scrcpy tap 的 `ACTION_DOWN` 和 `ACTION_UP` 合并为一次 socket 写入，降低 USB 模式弹窗点击时事件被重枚举打断的概率。
- 去除批量点击后的 USB/MTP 状态确认，点击写入成功后立即进入下一台设备，显著缩短 29 台设备一轮点击耗时。
- control socket 在视频流 EOF 时会及时移除，避免复用失效连接。

#### 页面加载与翻页

- scrcpy 启动并发从 2 提升到 4，提高初次加载当前页视频流的连接速度。
- 当前页设备优先启动视频流。
- 翻页时保留当前页并预热相邻页设备视频流，减少下一页等待时间。
- 自动唤醒设备延后执行，避免和 scrcpy 启动阶段抢占 ADB 资源。

#### 前端体验

- 视频流断开、重连、接收状态更直接显示在设备卡片上。
- 断流后清除旧视频帧，避免误以为仍在实时显示。
- 恢复接收后首帧自动重新显示，无需手动刷新页面。

### 修复

- 修复部分设备批量点击时 control socket 已写入但后续状态判断拖慢整轮操作的问题。
- 修复 USB 模式切换导致视频 EOF 后重连等待过长的问题。
- 修复断流后前端 `streamFrames` 被清理但同尺寸新流不会重新显示的问题。
- 修复批量控制和视频重连争抢 ADB 信号量导致重连被饿死的问题。

### 行为变化

- 批量点击不再验证 USB 是否最终切到 MTP；结果中的 `ok` 表示点击事件写入成功。
- USB 模式切换导致的视频流断开属于预期现象，应用会自动重连。
- 翻页时相邻页设备会被后台预热，因此同时保持的视频流数量会比当前页设备数更多。

### 验证

本版本已通过以下检查：

```bash
npm test -- --run
npm run build
cargo test
```

验证结果：

- 前端单元测试：31 passed
- Rust 单元测试：49 passed
- 前端生产构建：通过

### 已知事项

- 批量点击 USB 弹窗时，如果需要确认设备最终 USB 状态，需重新启用或另行实现后台并发验证。
- 大批量设备同时 USB 重枚举时，部分设备仍可能短暂显示 `device not found`，属于 ADB 枚举过程。
- `src-tauri/-` 是当前工作区内未跟踪的临时文件，不属于发布内容。

---

## v0.1.1 - 2026-03-31

### 修复

- 修复设备信息获取不完整的问题。
- 通过 `adb shell wm size` 获取真实屏幕分辨率。
- 通过 `adb shell getprop ro.product.model` 获取设备型号。
- 通过 `adb shell dumpsys battery` 获取电池电量。
- 修复点击和滑动坐标映射因设备分辨率为 0 失效的问题。
- 修复前端传递浮点 `sourceWidth` / `sourceHeight` 导致后端 `u32` 校验失败的问题。

---

## v0.1.0 - 2026-03-25

### 首次发布

- 多 ADB 服务器管理。
- 多设备列表与在线状态展示。
- 设备禁用/启用。
- 分页设备网格。
- 实时截屏预览。
- 批量点击、滑动、文本输入、按键事件和 shell 命令。
- 单设备 scrcpy 独立投屏入口。
- 深色主题界面。
