# ADR 0002: DCC/浏览器扩展策略对比 — 外层 SurfaceAdapter vs upstream `register_host_tools`

- Status: Superseded（方案 C 实测不可行，改为 Gateway 分工，见"实测修订"）
- Date: 2026-08-05（修订：2026-08-06）

## 背景

`dcc-cua-core` 通过 git pin 以 **rlib 形式进程内链接** `cua-driver-sdk`
（`CuaDriver::try_create_for_host`），核心会话（窗口截图、输入、`browser_*` 工具）
不经过任何外部进程。CLI 仅在 `daemon` / `mcp` / `recording` 三个代理子命令和
macOS private worker 场景才拉起独立的 `cua-driver` 二进制，发布包因此附带
`cua-driver(.exe)`、`cua-driver-uia.exe`、`cua-cursor-theme(.exe)` 三个 companion。

目标：在不 fork、不额外分发 upstream 二进制的前提下，让 dcc-cua 更好地支持
浏览器与 DCC 软件（Maya commandPort、Unreal Remote Control 等 typed API），
并消除现有硬编码（浏览器工具白名单、`dcc-wuia:` 前缀、错误字符串匹配触发 UIA fallback）。

## 方案 A：外层 SurfaceAdapter crate（原计划）

新建 `dcc-cua-adapter` crate，定义 `SurfaceAdapter` trait + `AdapterRegistry`，
按 `SemanticRoute` 键控；`ComputerUseSession` 改为查 registry 分发，
DCC typed 适配器（Unreal HTTP / Maya TCP）实现该 trait，全部逻辑在 upstream SDK 之外。

```text
Agent → ComputerUseSession → AdapterRegistry → BrowserAdapter → sdk.call_tool("browser_click")
                                             → UnrealTypedAdapter → HTTP Remote Control
                                             → MayaTypedAdapter   → commandPort TCP
```

## 方案 B：upstream 扩展点 `register_host_tools`

upstream SDK 预留了正式的进程内扩展口（均为 public API）：

- `DriverHostOptions.register_host_tools: Option<fn(&mut ToolRegistry)>`
- `cua_driver_core::tool::Tool` trait（object-safe，`async fn invoke`，
  含 `protected_resource_*` 保护资源钩子）
- `ToolRegistry::register(Box<dyn Tool>)`

DCC 工具（如 `maya_command`、`unreal_remote_call`）注册进 upstream ToolRegistry，
与 `browser_click`、`get_window_state` 同级分发，复用 upstream 的 session 生命周期、
authorization/consent、trusted-adapter 证据链。

```text
Agent → ComputerUseSession(安全壳) → sdk.call_tool("maya_command") → CuaDriver ToolRegistry
                                                                       └─ 我们注册的 MayaTool
```

## 逐项对比

| 维度 | A：外层 SurfaceAdapter | B：register_host_tools |
|---|---|---|
| 分发 | 不新增二进制（与 B 相同；companion 二进制问题两案均不解决，见下） | 同左 |
| upstream 耦合 | 只依赖已在用的 `cua-driver-sdk` 公开面（`call_tool` 等），升级 rev 风险最小 | 额外依赖 `cua-driver-core::tool`（内部 crate，publish=false，无 semver 承诺），升级 rev 可能破坏 Tool trait 签名 |
| 复用 upstream 机制 | 无：session 授权、consent、element_token、审计需在外层自行对齐或绕过 | 全部复用：注册的工具天然进入 upstream 分发/授权/审计管线 |
| 表达能力 | 完全自由：trait 可携带任意状态、任意签名 | 受 `fn` 指针限制：注册函数不能捕获闭包状态，工具可变状态需 `static`/`OnceLock` 自管 |
| 架构重复 | 在 SDK 外再造一层"工具注册 + 分发"，与 upstream ToolRegistry 职责重叠（当前"二次开发像重复造壳"感受的来源） | 无重复：外层 `ComputerUseSession` 回归纯安全壳（exact-window scope、observation fence、policy） |
| 消除现有硬编码 | 白名单/前缀/超时改由 AdapterCapabilities 声明，但仍是我们自己维护的平行元数据 | DCC 工具从"白名单转发"变为一等工具；浏览器白名单仍保留在壳层（这是刻意的安全设计，两案都不应移除） |
| 安全边界 | 外层 policy 全权把关；typed API 通道（HTTP/TCP）完全绕过 upstream 授权，需要自建审计 | typed API 通道同样由我们实现，但挂在 upstream `Tool::protected_resource_*` 钩子下，破坏性操作可接入其 consent 流程 |
| 测试 | 纯外层，mock 简单；CI 无新要求 | 需构造带自定义 registry 的 driver 实例；upstream 已有 `register_slow_host_tool` 测试先例可参照 |
| 工作量 | 新 crate + core 大改（runtime.rs 分发路径重写） | core 小改（driver_factory 传入注册函数 + 新增 tools 模块），无新 crate 或新 crate 仅放 Tool 实现 |
| hakari/CI | 新 crate 需 `cargo hakari generate` 更新 workspace-hack | 同左（若新增 crate）；不新增 crate 则无 |

## 关键事实澄清

1. **两案都不解决 companion 二进制分发**。`daemon`/`mcp`/`recording` 代理和
   macOS private worker 是独立问题：可后续将这三个子命令改为进程内实现
   （SDK 有 `EmbeddedCuaDriverHost`；recording 有 `ToolRegistry::register_recording_tools`）
   或直接砍掉，届时才可能去掉 `cua-driver(.exe)` 分发。该决策与 A/B 正交。
2. **"继承覆写"在 Rust 语境下即方案 B**：upstream 没有留出行为覆写点
   （如替换 `browser_click` 实现），`register_host_tools` 只能**新增**工具，
   不能覆盖同名工具；真要改 upstream 行为，唯一惯例是提 PR 后 pin 新 rev
   （已有先例：PR #2812）。
3. **`fn` 指针限制的实际影响**：DCC typed 连接（Maya TCP、Unreal HTTP client）
   的连接池需要全局状态。用 `OnceLock<Registry>` 可行但是隐式全局；
   方案 A 中同样的状态挂在 session 上，生命周期更清晰。
4. **`cua-driver-core` 是内部 crate**：方案 B 使 Cargo.toml 直接依赖它，
   每次升级 upstream rev 需要同时验证 `Tool` trait 是否变化。当前 pin 策略下
   风险可控（rev 锁死），但升级成本从"验证 SDK 面"扩大为"验证 SDK + core::tool 面"。

## 混合方案 C（备选）

- **进程内 DCC typed 工具走 B**：`maya_command`、`unreal_remote_call` 注册进
  upstream registry，获得统一分发与 consent 管线。
- **路由与语义仍在外层**：`SemanticRoute` → 工具名的映射、denied-word 豁免、
  外部 profile 加载放在 `dcc-cua-semantic-profiles` 增强中，不新建 adapter crate。
- **壳层职责不变**：浏览器白名单、observation fence、policy 留在
  `ComputerUseSession`，只做去重清理（合并 `action_arguments` 等），不做架构翻转。

C 的代价是同时承担 B 的 upstream 内部依赖风险，但避免 A 的平行注册表和 B 的
"路由也塞进 upstream"过度耦合。

## 建议

若接受对 `cua-driver-core::tool` 的 pin 依赖（当前 rev 锁定，风险可控），
**推荐 C**：它直接回应"为什么不直接使用 upstream 而要二次开发"——我们不再造壳，
二次开发只剩两块真正的增量：(1) DCC typed 工具本体，(2) 主机侧安全壳与语义 profile。
若希望与 upstream 内部 API 完全隔离、接受多维护一层注册表，则选 A。

## 决策（原始，已被推翻）

采用方案 C：DCC typed 工具经 `register_host_tools` 注册进 upstream ToolRegistry；
路由与语义增强留在 `dcc-cua-semantic-profiles`；`ComputerUseSession` 保持安全壳职责，
仅做去重清理，不新建外层 adapter crate。

## 实测修订（2026-08-06）

方案 C 实现后端到端实测暴露一个此前对比矩阵遗漏的硬约束：

**upstream 风险分类闸门对 host 工具 fail-closed。** 每次工具调用都经过
`cua_driver_core::authorization::authorize_tool_call_with_context`，其中
`classify_tool_call`/`advertised_risk_for` 是**硬编码的工具名 match**，
未知名字一律 `RiskClass::Unclassified` → 拒绝
（`tool 'maya_command' has no reviewed risk classification`）。
upstream 没有为 host 工具提供风险分类扩展点（`ToolDef` 的
`read_only`/`destructive` 注解不参与风险派生），任何 permission mode
（含 unrestricted + bypass）都无法放行。也就是说：`register_host_tools`
注册的工具能出现在 `tools` 列表，但**永远不可调用**——该扩展点在当前
pinned rev 下对第三方工具事实上不可用（upstream 自己的
`check_update_tool` 之所以可用，是因为 `check_for_update` 名字被硬编码进
R0 分类）。

单元测试直接调 `tool.invoke()` 绕过了授权边界，因此 CI 未暴露此问题；
经 `call_global_tool` 走完整分发链路的实测才发现。

**修订后的决策——按职责分工，不再让 CUA 承载 typed execution：**

- **DCC typed execution（Maya commandPort、Unreal Remote Control 等）
  归 DCC-MCP Gateway/adapter 所有**。移除 `dcc_tools` 模块、
  `register_host_tools` 注册与 `cua-driver-core` 依赖。
- **dcc-cua 只负责 exact-window UI 观察/动作与 UI fallback**：
  窗口发现、快照、语义/视觉动作、验证、control banner。
- `SemanticRoute::{MayaTypedApi,UnrealTypedApi}` 保留为**声明式路由意图**
  （`is_gateway_typed_api()`），供上层把请求路由给 Gateway；
  profile 本身不再映射到任何 CUA 工具名。
- 若未来 upstream 为 host 工具开放风险分类扩展点（或接受相关 PR），
  可重新评估方案 C。

本次保留的增量：安全壳去重（结构化 UIA fallback 触发、共享 action builder、
统一 denied-word 常量）与 semantic-profiles 增强（外部 profile 目录、
denied-word 豁免）不受影响，照常合入。
