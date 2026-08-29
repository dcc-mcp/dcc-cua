<p align="center">
  <img src="assets/brand/dcc-cua-logo.png" alt="CUA logo" width="900">
</p>

<p align="center">
  <a href="README.md">English</a> | 简体中文
</p>

# dcc-cua

`dcc-cua` 是基于开源 [CUA SDK](https://github.com/trycua/cua) 的跨平台
Computer Use Automation 运行时和命令行工具，支持 Windows、Linux 和 macOS。

项目最初来自 `dcc-mcp-core`，现在由独立仓库维护通用的 Host 协议、安全边界和
平台能力。`dcc-mcp-core` 是它的使用方，而不是运行时依赖。发布包内只有一个
`dcc-cua` 可执行文件，不需要额外安装或分发 `cua-driver`。

> 本文帮助中文用户完成安装、Profile 选择和常用控制。完整 CLI 参数、Host IPC
> 方法和协议细节以[英文 README](README.md)为准；命令、字段名和稳定 ID 不翻译。

## 安全契约

- 所有窗口操作必须绑定精确的 PID、窗口 ID 和标题范围，Agent 不能扩大目标。
- 每次修改操作都需要新的 observation ID；操作后必须重新观察并独立验证结果。
- 文本、按键、拖拽和坐标输入都有边界限制；目标应用身份缺失或不可读取时会拒绝控制，
  并通过一套共享的纵深防御策略拒绝已知终端、命令解释器、认证、密码与安全应用身份。
- Windows 物理 `Escape` 和跨平台 `interrupt_all` 会停止 Host 中的活动连接。
- Windows、Linux 和打包后的 macOS Host 都提供可见且不截获点击的控制指示。
  是否排除在截图之外取决于捕获后端：精确窗口捕获会排除指示层，Windows 的
  verified-visible 桌面 `BitBlt` 回退可能包含安全横幅或边框。指示层仅用于安全提示，
  不能作为目标内容或操作证据。
- Cloudflare 等真人验证、安全确认、账号、购买和下载边界需要可信人工确认授权；
  Profile 只声明路由，不能绕过网站或操作系统的安全策略。

即使 Agent 获得完整访问权限，上述精确窗口、最新观察和可信确认边界仍然有效。
完整访问允许 Agent 在授权窗口中使用点击等输入能力，不代表可以静默处理真人验证。

## 架构与集成边界

主要 crate 的职责如下：

| crate | 职责 |
| --- | --- |
| `dcc-cua-core` | 作用域、观察、策略、会话和 CUA 执行边界 |
| `dcc-cua-host` | 长生命周期、版本化 IPC 和请求路由 |
| `dcc-cua-client` | 供 `dcc-mcp-core` 等调用方复用的 Host 客户端 |
| `dcc-cua-browser` | 精确窗口浏览器绑定、CDP 操作和受限文件传输 |
| `dcc-cua-semantic-profiles` | 应用选择器、语义表面、目标、路由和回退声明 |
| `dcc-cua-indicator` | 控制提示、停止代次和 Windows 物理 Escape 边界 |
| `dcc-cua-cli` | 组合工作区能力的单一命令行入口 |

应用专用适配器位于本工作区之上。DCC 操作应优先使用 Maya、Houdini、Unreal 等
应用的 typed API，再使用 `dcc-cua` 的精确窗口语义或视觉控制。浏览器流程应走
`dcc-cua` 的 `browser_dom` 路由，不要替换成 in-app Browser skill。

一个逻辑任务应复用同一条 Host 连接和同一个 `open_session`。窗口会话默认空闲
15 分钟后停止，也可通过 `idle_timeout_ms` 设置 1 秒到 24 小时的边界；每次通过
授权的会话请求都会续租。嵌入方可直接使用
`dcc_cua_client::LogicalTaskSession`，由它统一注入该任务私有的 session、grant 和
window capability，避免跨任务误用。

浏览器以 CDP 为默认 provider。只有 CDP 不可用，或必须控制用户已登录且明确配对
的标签页时，才选择可选扩展：

```powershell
dcc-cua browser-extension plan --browser chrome --extension-id PUBLISHED_ID --cdp-state unavailable
dcc-cua browser-extension install-native-host --browser chrome --extension-id PUBLISHED_ID
```

该安装命令只为精确的已发布扩展 ID 注册 Native Messaging Host，不会静默旁加载
扩展。普通用户仍从浏览器商店安装签名扩展并授权，然后在目标标签页点击一次扩展
图标完成配对。Agent 随后在原有逻辑任务会话上调用
`browser_extension_status` 和 `browser_extension_call`；权限、来源、身份、配对或
协议失败后不得静默切回 CDP。

## 独立语义 Profile

`dcc-cua-semantic-profiles` 当前内置 `ue`、`maya` 和 `fab`。Profile 是声明式的
路由和词汇契约，不是自动化脚本：它不会启动应用、自动切换回退、执行操作或证明
结果。Core 和 Host 仍负责窗口作用域、观察、授权、输入和验证。

| 字段 | Agent 应如何理解 |
| --- | --- |
| `selectors` | 候选应用、窗口或 URL 身份；对象之间是 OR，对象内部约束是 AND |
| `surfaces[]` | 稳定任务区域，例如 `outliner`、`dialog`、`launcher_download` |
| `targets[]` | 稳定意图词汇；`supported_actions` 是允许列表，不是执行指令 |
| `fallback` | 指向其他 `profile_id`/`surface_id`；切换后必须重新发现、绑定和观察 |
| `settings` | 默认 locale、首选路由、对话框风格和破坏性操作确认策略 |

路由所有权必须明确：

| 路由 | 执行方 |
| --- | --- |
| `accessibility` | Profile CLI；仅在一个最新的真实元素精确匹配时操作 |
| `unreal_typed_api` | Unreal 适配器或 Skill |
| `browser_dom` | 已绑定精确浏览器的适配器 |
| `os_native_dialog` | 平台原生对话框控制路径 |
| `visual_fallback` | `dcc-cua` 的最新精确窗口视觉观察 |

### 多语言规则

Profile 使用 BCP-47 locale 标签维护可见文本别名。所有语言别名都参与匹配，Agent
不需要先猜测 UI 语言；`dcc-cua profiles` 会通过 `supported_locales` 暴露覆盖范围。

- `default_locale` 标记未带 locale 的已有别名。
- `localized_names` 和本地化窗口标题只保存用户可见文本。
- `profile_id`、`surface_id`、`target_id`、role、action 和 automation ID 必须稳定，
  不随语言翻译。
- 匹配会压缩空白并执行 Unicode 小写转换。
- 只包含 `window_title_contains` 和 `names` 的旧 Profile 仍然有效。

Agent 应按以下顺序使用 Profile：

1. 运行 `dcc-cua profiles`，查看 `supported_locales`，再用
   `dcc-cua profile --id ID` 检查候选 Profile。
2. 用 `dcc-cua list --on-screen` 发现真实 PID 和窗口 ID，并绑定精确身份。
3. 选择 surface 和稳定 target ID；本地化名称只用于匹配当前 UI。
4. 根据 surface 的 `route` 分发；`profile --action` 只执行 `accessibility` 路由。
5. 遇到 `fallback` 时重新发现、绑定、观察和验证，不能沿用旧窗口状态。
6. 每次修改后重新观察，并验证业务状态；输入已送达不等于任务成功。

例如，UE 编辑器内 Fab 下载不可用时，`ue/fab/download` 会声明回退到
`fab/launcher_download`。Agent 必须重新绑定 Epic Games Launcher；账号、购买、
Cloudflare 验证和安全确认仍需人工授权。

查看 Profile 不需要启动 DCC：

```powershell
dcc-cua profiles
dcc-cua profile --id maya
dcc-cua profile --profile-file C:\profiles\maya-studio.json
```

`dcc-cua profiles` 默认只列出可用的内置和已安装 Profile。使用 `dcc-cua
profiles --state invalid` 可检查被拒绝的安装包及其路径、校验原因和修复提示；仅在
需要一次性审计可用与无效条目时使用 `--state all`。

发现窗口后，按稳定 target 查询或执行一个 accessibility 操作：

```powershell
dcc-cua list --app maya.exe --on-screen
dcc-cua profile --id maya --pid $pid --window-id $hwnd --surface home --query new_scene
dcc-cua profile --id maya --pid $pid --window-id $hwnd --surface home --query new_scene --action click --activate
```

发布包还包含两个独立 Agent Skill：

- `skills/cua-cli`：CLI 控制循环、验证、安全策略和长任务恢复；
- `skills/cua-profile-authoring`：官方及用户自定义 Profile 的编写规范。

## 常用 CLI

开发环境可用 `cargo run -p dcc-cua-cli --` 替换下面命令中的 `dcc-cua`：

```powershell
dcc-cua manifest
dcc-cua doctor
dcc-cua doctor --route visual
dcc-cua apps
dcc-cua list --app maya.exe --on-screen
dcc-cua snapshot --pid 4242 --window-id 123456 --output before.png
dcc-cua snapshot --pid 4242 --window-id 123456 --pixels-only --output game.png
dcc-cua accessibility --pid 4242 --window-id 123456
dcc-cua click --pid 4242 --window-id 123456 --element-index 12
dcc-cua type --pid 4242 --window-id 123456 --text "hello" --focused
dcc-cua snapshot --pid 4242 --window-id 123456 --output after.png
dcc-cua verify --pid 4242 --window-id 123456 --expect-json '[{"window":{"exists":true}}]'
dcc-cua clipboard-write --pid 4242 --window-id 123456 --text "bounded text"
dcc-cua clipboard-read --pid 4242 --window-id 123456 --include-text
dcc-cua interrupt-all
dcc-cua update --check
```

所有命令都不会自动输出动态更新提示，确保机器可读结果在任意 TTY 与重定向组合下都不受
stderr 状态文本污染。需要检查并安装完整发布包时，请显式执行 `dcc-cua update`。

`snapshot`、`act`、`verify`、`clipboard-read` 和 `clipboard-write` 接受同一组
`--app`、`--pid`、`--window-id`、`--title` 窗口选择器。多个选择器按 AND 同时校验；
重复冲突、缺失或为零的原生身份、零匹配、多匹配，以及 PID/HWND/title 漂移都会在
变更或剪贴板回读前 fail closed。优先复用 `list` 返回的 PID/HWND，并在动作前重新截图。

`act` 成功回执只证明受限输入已送达，不证明应用状态已按预期改变。验收变更时必须要求
`post_snapshot` 成功，并校验树/值、像素或应用自身状态确实变化；
`window.exists=true` 只证明窗口仍存活。`clipboard-write` 的成功回执也不是后置条件：
对于非敏感、受限的测试值，应使用同一精确绑定的 `clipboard-read --include-text` 做值比较，
或验证应用中粘贴后的值/状态，且不得暴露私人剪贴板内容。

`manifest` 是供 Core 和独立调用方使用的稳定机器入口，包含平台、协议、能力、端点、
图像传输和推荐启动参数，并明确声明不需要单独 driver。若同一应用有多个窗口，操作时
必须传入 `--pid` 和 `--window-id`。优先使用观察结果中的 `element_token` 或
`element_index`，自绘界面才使用坐标。窗口像素动作使用最新精确窗口截图内的非负局部
坐标；UIA 元素 `bounds` 标记为 `virtual_desktop`，桌面动作则使用可为负数的虚拟桌面
绝对坐标，因此无需为了操作左侧或上方显示器而移动用户窗口。携带 x/y/path 的窗口动作
必须同时传入 `--observation-width` 和 `--observation-height`，值取自生成坐标的那次
`snapshot.coordinate_space`；缺少任一尺寸会在 session 启动前拒绝动作。

单次 CLI 命令会把正常的结构化成功或错误信封写入 stdout。命令错误保留非零退出码，
并且只输出一个 JSON 信封，因此 stdout 管道、重定向和命令替换无需合并 stderr 即可
解析失败。stderr 仅用于固定且安全的进程诊断，例如内部 panic 或 stdout 管道不可用；
它不会重复输出命令信封。公开错误码只来自有界的本地类别，消息文本固定；原始命令或
选项文本、错误字符串、路径、参数、token 和远端 payload 都不会复制到单次命令信封。
长时间运行的 `host-jsonl`、Native Messaging 和 MCP 命令继续使用各自协议规定的 stdout
帧格式。

在 Windows 上，对于 accessibility provider 缺失或无响应的自绘窗口，使用
`snapshot --pid PID --window-id HWND --pixels-only`。该显式模式不会启动
accessibility provider，只会捕获指定 PID/HWND；发布前后会复核窗口身份、原生边界、
DPI、可见性、遮挡和 capture generation。窗口移动、缩放、PID/HWND 复用、最小化、
隐藏、遮挡或跨窗口替换都会 fail closed，且绝不会返回或裁剪整桌面截图。普通 snapshot
遇到 typed bounded accessibility timeout 时可安全降级到同一精确窗口捕获路径，但
provenance 会标记为 `accessibility_timeout_degraded`；显式模式标记为 `pixels_only`。
macOS 与 Linux 的 manifest 不会广告 `runtime.exact_window_pixels` 或对应 Host capability，
该路由在这些平台会返回 `BackendUnavailable`。
provider 缺失则标记为 `accessibility_unavailable_degraded`，不会误报成 timeout。

独立的 `accessibility` 命令会区分“自绘窗口没有可用的语义 provider”和“provider 或
worker 运行失败”。前者返回 `no_accessibility_provider`，并明确说明该窗口类不可通过
重试获得语义树，调用方应改用 `snapshot --pixels-only`，再结合 OCR 或其他感知层。
`backend_unavailable` 仍表示 provider 或后端执行失败，不能据此断言该窗口类永久不支持
accessibility。

成功的窗口 `snapshot` 会返回独立的 `coordinate_space`。其中 `width`/`height` 直接来自
编码 PNG 的 IHDR，表示 `--output` 实际写入图像的像素尺寸，而不是应用 render target
尺寸。若图像点为 `(x, y)`，`window-state` 的设备像素边界为 `bounds`，对应屏幕点为
`screen_x = bounds.x + x * bounds.width / observation_width`、
`screen_y = bounds.y + y * bounds.height / observation_height`。`window-state` 已是设备像素，
包含最大化窗口边框和负坐标显示器；不得再次乘以
`screen-size.structuredContent.scale_factor`，否则会造成二次缩放。

当 `snapshot --activate` 收到带有 `background_delivery_viable: true` 的类型化
`foreground_activation_refused` 错误时，会保持原有精确 PID/HWND session 并直接执行后台
截图，不再丢弃可用观察。成功结果会返回
`activation.status: "refused_fallback_background"`，且 activation receipt 中
`degraded: true`。若缺少后台可行性证明、激活超时或出现其他激活错误，仍会失败关闭。

`doctor` 默认继续执行严格的完整健康检查。对于 Houdini、Unreal 等自绘 DCC 界面，
可用 `doctor --route visual` 单独验收精确窗口枚举、WGC 截图和受限坐标输入；输出仍会
保留 UIA 降级信息，并分别报告 `full`、`visual`、`semantic` 三条路由，避免一个挂起的
UIA provider 错误阻断健康的视觉控制路径。
Windows 全局 UIA 枚举超时时，如果 UIA 权限仍有效，`semantic` 会明确报告为降级的
`exact_window_uia_fallback`；该路径仍强制精确 PID/HWND 和 fresh observation。

元素索引和 token 只属于生成它们的那一次无障碍快照，不能与像素快照、其他后端、
其他窗口或旧持久会话的索引/token 混用。友好的语义 CLI 命令会先获取最新无障碍
快照，再把索引绑定为当前 token 后投递。

Windows 后台 UIA 动作可能已经成功，但系统拒绝恢复原前台窗口，或原窗口在动作中
消失。此时结果仍为成功并带有 `action_executed: true`，恢复失败单独记录在
`foreground_restore.success: false`。调用方不得重试输入，应重新观察并验证应用状态。
UIA worker 脚本通过私有 stdin 管道直接加载，不再写入同用户可修改的临时文件，也不再
使用 `ExecutionPolicy Bypass`。readiness、请求和响应都携带并校验协议版本；PowerShell
中的策略分级、敏感目标和 stale fence 判定由夹具驱动的行为测试覆盖。

单次窗口操作会尝试生成新的操作后截图。如果输入已执行但截图失败，结果会返回
`action_was_executed: true`，调用方不得盲目重试。完整命令和参数见
[CLI 参考](README.md#cli)。

## Host IPC

多步骤 Agent 应复用一个持久 Host 连接，让同一 PID/窗口 ID 保持控制指示、会话和
observation fence：

```powershell
dcc-cua host --stdio
dcc-cua host-ensure
dcc-cua ping
dcc-cua host-call --method list_apps --json '{}'
dcc-cua host-jsonl --output-dir artifacts
```

`dcc-cua-client` 是 `dcc-mcp-core` 的直接嵌入路径：

```rust,no_run
let mut host = dcc_cua_client::HostClient::connect_default("dcc-mcp-core").await?;
let response = host.request("list_windows", serde_json::json!({})).await?;
let stopped = host.interrupt_all().await?;
```

每个连接先执行 `hello`；请求使用 `request_id` 关联响应，图像可通过共享内存或二进制
附件传输。修改请求保持串行，无状态发现请求可做有界并行。Host 不自动重放失败请求；
重启后必须重新打开会话并获取新观察。完整方法、grant、JSONL 和协议限制见
[Host IPC 参考](README.md#host-ipc)。

需要标准 MCP 工具结果的消费端可以显式传入 `--response-format mcp`。此时每行响应
包含 `content`、`structuredContent` 和 `isError`，窗口、桌面、操作后、浏览器以及
原生工具返回的图像附件会提升为 MCP 原生 `image` content。默认 `host` 格式保持不变；
该选项只投影 Host 响应，不会把 JSONL 传输伪装成完整的 MCP JSON-RPC Server。

## 开发门槛

Windows 的 `vx.toml` 固定 MSVC 14.44、Spectre 缓解库和 Windows SDK 环境。
工作区使用 Hakari 统一四个构建目标的依赖 feature，同时保持 target/host 图隔离。

```powershell
vx cargo install cargo-nextest cargo-hakari --locked
vx cargo hakari generate --diff
vx cargo hakari manage-deps --dry-run
pwsh -NoProfile -File scripts/check-rust-layout.ps1
vx cargo fmt --all -- --check
vx cargo nextest run --workspace --all-targets --locked
vx cargo test --workspace --doc --locked
```

可选的真实 GUI E2E：

```powershell
vx cargo nextest run --locked -p dcc-cua-e2e --features gui-e2e --no-run
pwsh -NoProfile -File scripts/run-gui-e2e.ps1 -Binary target/debug/dcc-cua.exe
```

## CI/CD 与发布

CI 在 Windows、Linux 和 macOS 上检查代码布局、格式、工作区测试、锁定 release
构建和真实发布二进制 E2E。发布归档包含单个 `dcc-cua` 可执行文件、assets、Skills、
英文/简体中文 README 以及项目和上游许可证。

原生发布契约为每个支持的 Rust target 提供一份归档、SHA-256 sidecar 和安装 manifest：

| 平台 | 发布 target | GitHub runner |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-latest` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `ubuntu-latest` |
| macOS Apple silicon | `aarch64-apple-darwin` | `macos-26` |
| macOS Intel | `x86_64-apple-darwin` | `macos-26-intel` |

新建 tag、该 tag 解析到的 commit、构建 HEAD 和 GitHub Release target 必须绑定同一
commit。每个 target 只构建一次，完整资产集合随后绑定到一个 workflow artifact ID
和内容 digest；已有 tag、release 或 asset 不能作为重建或覆盖目标。
每个消费 job 都先校验 exact raw workflow artifact ZIP 的真实 SHA-256，再解包使用；
action 自身的下载只进入隔离目录，不作为发布源。create-only 上传完成后会执行发布后回读，
严格核对 release target、asset 名称、大小、digest、无额外文件，并确认原生 release 的
Latest 身份没有被扩展 release 污染。

只有四个 target 的归档、checksum、manifest 和聚合 provenance 全部匹配时，上传 job
才会继续。当前原生可执行文件没有平台代码签名；公开验证契约仅证明 SHA-256 checksum，
并在 provenance 中明确记录 signing `not_performed`，不会声称二进制已签名。
官方 `macos-26-intel` runner 可用期间，Intel macOS 仍是公开支持目标。两个 macOS
发布目标统一选择 Xcode 26.6，并在 runner 架构或 macOS 26 SDK 不匹配时 fail closed；
若该托管镜像或固定工具链退役，必须同时调整 runner 和发布契约。

发布门槛在产品验收前保持关闭；不要提前设置
`DCC_CUA_RELEASE_READY=true`。Release Please 负责版本和变更日志，不发布 crates。

CUA SDK revision 固定在 `Cargo.toml` 和 `Cargo.lock`。发布完整性检查不会操作用户桌面，
也不能证明真实用户环境中的 raw-input 行为。真实截图和输入仍要求当前操作系统具有原生
桌面权限、交互式会话和独立的运行时验收。
