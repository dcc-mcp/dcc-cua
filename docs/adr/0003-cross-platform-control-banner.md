# ADR 0003: 跨平台 Control Banner 设计

- Status: Proposed
- Date: 2026-08-06

## 背景与现状

`dcc-cua-indicator` 是 Host 自有的控制会话安全指示层，当前由三部分组成：

1. **Banner 条**：非激活、点击穿透的置顶条，显示本地化标签
   （"Agent 正在操作 App"），带 per-session 颜色（`session_color` 哈希取色，
   避开默认蓝 209°±45°）。
2. **Target frame**：受控窗口四边的呼吸渐变描边（`breathing_frame_alpha` /
   `gradient_frame_alpha`）。
3. **Cursor halo** + **Escape 全局停止热键**（`EscapeHub`，广播
   `INTERRUPT_GENERATION`）。

跨平台现状：`platform.rs`（约 1000 行）是 Win32 layered-window 实现
（`WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_NOACTIVATE` + `RegisterHotKey`）。
非 Windows 平台是 no-op stub，`status().backend == "unavailable"`，
操作者**看不到任何受控提示，也没有 Escape 停止边界**——这是安全能力缺口，
不只是外观问题。

公共层（`lib.rs`）已经平台无关且有单元测试：颜色/HSV、标签本地化（7 语言）、
interrupt generation、几何与 alpha 动画的纯函数。

## 目标

各平台提供同等安全语义：可见归因标签、受控窗口视觉标识、Escape 协作停止、
非激活/点击穿透（不干扰 CUA 输入）、per-session 颜色。

## 设计

### 阶段 0：共享核心抽取（纯 Rust，全平台可测）

把 `platform.rs` 中与 Win32 无关的部分抽到 `geometry.rs`（banner/frame/halo
几何计算、DPI 缩放）与 `animation.rs`（呼吸/渐变 alpha、缓动），以像素无关
单位表达；Win32 后端改为消费者。现有 `breathing_frame_alpha` 等测试随迁。

### 阶段 1：视觉/交互友好化（先落在 Windows，规范全平台）

- Banner 增加**停止提示**后缀：`… — Esc 停止`（随标签语言本地化）。
- 圆角胶囊造型 + agent 颜色底、白字；banner 不遮挡目标窗口标题栏
  （放在窗口上缘外侧，屏幕顶部不够时移到下缘）。
- 鼠标悬停 banner 时降低不透明度（150ms 缓动），操作者可看清被遮内容。
- 出现/消失使用 120ms fade，避免闪烁。

### 阶段 2：macOS 后端

- `NSPanel`（`.nonactivatingPanel`）+ `ignoresMouseEvents=true` +
  `.statusBar` window level；跟踪目标窗口用 AX observer
  （`kAXMovedNotification`/`kAXResizedNotification`），退化为轮询
  `CGWindowListCopyWindowInfo`。
- Escape 用 `CGEventTap`（listen-only）；Accessibility 权限已是 CUA 输入
  的前置条件，无新增授权面。
- 约束：AppKit 必须主线程。打包 macOS Host 走 upstream private worker，
  banner 须在 Host 进程内自建 main-thread runloop（与 Win32 后端的专用
  message-loop 线程同构）。

### 阶段 3：Linux 后端

- **X11**：override-redirect ARGB 窗口 + XShape 空输入区（点击穿透）+
  `_NET_WM_STATE_ABOVE`；Escape 用 `XGrabKey`。CI 的 xvfb+openbox+picom
  GUI E2E 环境可直接回归。
- **Wayland**：无全局定位/全局热键。有 `wlr-layer-shell` 时渲染屏幕
  上缘通栏 banner（无 target frame）；否则保持 `backend:"unavailable"`。
  Escape 不可全局捕获——`status().stop_key` 按后端如实上报，宿主/CLI
  提示改用 `interrupt-all` IPC 停止路径。

### 状态如实上报

`BannerStatus.backend` 取值扩展：`win32` / `appkit` / `x11` /
`wayland-layer-shell` / `unavailable`；`stop_key` 在无全局热键的后端返回
空并由调用方提示替代停止方式。诊断（`doctor`）透出该字段，避免"以为有
banner 实际没有"。

## 分阶段交付

| 阶段 | 内容 | 验证 |
|---|---|---|
| 0 | 共享 geometry/animation 抽取 | 现有单测跨平台跑 |
| 1 | 友好化视觉（Windows 先行） | Windows 手动 + 快照测试 |
| 2 | macOS AppKit 后端 | macOS CI GUI E2E |
| 3 | Linux X11 后端（Wayland 顶栏可选） | Linux CI xvfb GUI E2E |

## 决策

待评审。阶段 0/1 可与当前工作同 PR 或紧随其后；阶段 2/3 需要对应平台
实机验证，建议独立 PR。
