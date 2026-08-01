# dcc-mcp-computer-use

Cross-platform Computer Use host and CLI for DCC-MCP, backed by the open-source
[CUA SDK](https://github.com/trycua/cua).

This is the standalone runtime that DCC-MCP Core can launch and keep alive for
a whole task. Its public protocol is owned by this repository and provides:

- exact PID/window/title scope; agent requests cannot widen the target;
- a fresh observation ID is required for every mutation;
- bounded text, key, drag, and coordinate input;
- fail-closed sensitive-window policy;
- explicit stop/resume lifecycle and structured errors;
- a visible CUA mouse-shaped cursor and `DCC UI Control · <app> · Esc to stop`
  marker.

The repository is a Cargo workspace with six responsibilities:

- `dcc-mcp-cua-core`: scoped Computer Use domain, safety policy, and
  CUA execution boundary;
- `dcc-mcp-cua-browser`: exact-window browser binding, tab snapshots, typed
  browser actions, and bounded file transfer;
- `dcc-mcp-cua-client`: reusable Core-side Host IPC client with request
  correlation and binary image attachments;
- `dcc-mcp-cua-host`: long-lived versioned IPC and request
  routing;
- `dcc-mcp-cua-shm`: cross-platform shared-memory image handoff;
- `dcc-mcp-cua-cli`: the thin CLI process that composes the workspace crates.

Application-specific adapters stay above this workspace. A browser adapter can
add tab/DOM/iframe/download capabilities through CDP or WebDriver while using
this host as its visual fallback. Unreal/Fab flows belong in the Unreal or
browser adapter and should combine typed Unreal APIs with scoped CUA; Fab
account, purchase, and download confirmation remain explicit user-approved
operations.

## CLI

```powershell
cargo run -p dcc-mcp-cua-cli -- list --app chrome.exe --on-screen
cargo run -p dcc-mcp-cua-cli -- apps
cargo run -p dcc-mcp-cua-cli -- tools
cargo run -p dcc-mcp-cua-cli -- call --tool check_permissions --json '{}'
cargo run -p dcc-mcp-cua-cli -- call --tool set_config --json-file payload.json
cargo run -p dcc-mcp-cua-cli -- desktop-snapshot --output desktop.png
cargo run -p dcc-mcp-cua-cli -- screen-size
cargo run -p dcc-mcp-cua-cli -- cursor-position
cargo run -p dcc-mcp-cua-cli -- desktop-act --action-json '{"action":"click","x":100,"y":100}'
cargo run -p dcc-mcp-cua-cli -- launch --name Calculator
cargo run -p dcc-mcp-cua-cli -- doctor
cargo run -p dcc-mcp-cua-cli -- snapshot --app chrome.exe --output screenshot.png
cargo run -p dcc-mcp-cua-cli -- act --app chrome.exe --action-json '{"action":"click","x":100,"y":100}'
cargo run -p dcc-mcp-cua-cli -- verify --app chrome.exe --expect-json '[{"window":{"exists":true}}]'
```

`list`, `apps`, `tools`, `desktop-snapshot`, `screen-size`, `cursor-position`, and
`doctor` are read-only. `snapshot` and `act` require one exact
target; if an application has multiple windows, pass `--pid` and
`--window-id` instead of relying on an app name.

`apps` uses CUA's cross-platform application inventory. `launch` accepts one
explicit selector (`--name`, `--bundle-id`, `--aumid`, `--path`, or
`--launch-path`), supports repeated `--url`/`--arg` values, and applies the
same sensitive-application deny policy as the host.

Clipboard access is session-scoped and grant-gated: `clipboard_read` does not
return text unless the caller asks for it, and `clipboard_write` accepts exactly
one bounded text, image path, or regular-file path. Recording is also
grant-gated through `recording_start`, `recording_stop`, and `recording_state`.

## Host IPC

Start one persistent host process instead of spawning a process per action:

```powershell
cargo run -p dcc-mcp-cua-cli -- host --stdio
cargo run -p dcc-mcp-cua-cli -- host
cargo run -p dcc-mcp-cua-cli -- host-call --method list_apps --json '{}'
```

`dcc-mcp-cua-client` is the direct embedding path for dcc-mcp-core. It opens
the per-session endpoint, performs `hello`, sends JSON requests, and returns
the following binary image frame without base64 decoding in the control path:

```rust,no_run
let mut host = dcc_mcp_cua_client::HostClient::connect_default("dcc-mcp-core").await?;
let response = host.request("list_windows", serde_json::json!({})).await?;
```

Long waits can use `HostClient::request_with_cancel`; it sends `cancel` on the
same connection and consumes both the cancellation acknowledgement and the
wait terminal response.

For large or frequent images, use `connect_default_with_transport(...,
SnapshotTransport::SharedMemory)` and open the returned descriptor with
`dcc_mcp_cua_shm::SharedImageReader`; pixels do not cross the control pipe.

On Windows, the default endpoint is the per-session named pipe
`\\.\pipe\dcc-mcp-cua-v1-session-<WindowsSessionId>`. On Unix, the default
endpoint is `$TMPDIR/dcc-mcp-cua-v1.sock`. The protocol uses an unsigned
big-endian `u32` length followed by one UTF-8 JSON request or response. A
client selects `snapshot_transport` as `binary_frame` (the default) or
`shared_memory` in `hello`; binary snapshots are followed by one additional
length-prefixed PNG frame, avoiding base64 pixel transfer and keeping the JSON
frame under 4 MiB. Requests may include a top-level `request_id` (1–128
characters); the host echoes it on the JSON response, including errors. The
handshake advertises the exact capabilities of this build.

Native tool results expose every returned image in `attachments`. The single
binary frame following the JSON response is the concatenation of those images;
each descriptor gives its `offset`, `length`, and `mime_type`.

The supported request surface is `hello`, `list_apps`, `list_tools`, `list_windows`, `launch_app`, `open_session`,
`get_window_state`, `change_window_state` (`restore`, `show`, `activate`), `snapshot`,
`accessibility_snapshot`, `verify_state`, `call_tool`, `call_global_tool`, `get_session_state`, `cursor_tool`, `escalate_session`, `find`, `wait_for`, `browser_snapshot`,
`browser_prepare`, `browser_navigate`, `browser_click`, `browser_type`, `browser_pointer`,
`browser_set_input_files`, `browser_download`, `browser_dialog`,
`clipboard_read`, `clipboard_write`, `recording_start`, `recording_stop`,
`recording_state`,
`desktop_snapshot`, `screen_size`, `cursor_position`, `open_desktop_session`,
`desktop_session_snapshot`, `execute_desktop_action`, `stop_desktop_session`,
`execute_action`, `resume_session`, `terminate_app`, and
`stop_session`; `cancel` is available while `wait_for` is active on the same
connection. Semantic actions use CUA `element_index` values from the latest
accessibility snapshot, and `set_text`/`set_value`/`set_checked` use CUA's
native semantic value path. Coordinate actions remain available for
custom-drawn surfaces.
`list_windows` supports optional `app`, `pid`, and `on_screen_only` filters;
these are applied by the native backend before the response crosses IPC.
`find` filters the current accessibility tree by text, role, or element index
and returns a fresh `accessibility_state_id`. `wait_for` is bounded to 30 seconds and supports `element_present`,
`text_contains`, `text_equals`, and `value_equals`. `launch_app` requires a non-empty `task_grant_id`, `dcc_type`, and
`allow_app_launch: true`; `terminate_app` requires the separate
`allow_app_terminate: true` grant and force-closes only the exact session target;
neither permission inherits from an open DCC window
session. Clipboard operations require `allow_clipboard_read` or
`allow_clipboard_write`; recording operations require `allow_recording: true`.
Browser mutations additionally require `allow_browser_input: true`.
`browser_prepare` is destructive and separately requires
`allow_browser_prepare: true`; it never changes a personal browser profile
implicitly and forwards CUA's explicit setup refusal/approval contract.
`browser_snapshot` first binds the exact native window, then snapshots a
specific CUA tab; `browser_click`, `browser_type`, and `browser_pointer` require
the latest browser `snapshot_id`, exact binding, and an explicit input route.
`browser_navigate`, `browser_set_input_files`, and `browser_download` invalidate
the tab snapshot. Upload uses `allow_browser_input`; download is a separate
destructive grant (`allow_browser_download`) and CUA's host approval evidence.
`browser_dialog` only resolves page-owned JavaScript dialogs and requires the
exact current `dialog_id` for accept/dismiss.
`desktop_snapshot` is a full-display visual discovery surface; it does not
widen an existing window session or grant desktop-wide mutation. For an
explicit desktop input scope, use `open_desktop_session`, then take a fresh
`desktop_session_snapshot` and call `execute_desktop_action`; raw input grant
and the exact desktop capability are required.
While `wait_for` is running, the same connection accepts `cancel` with the
exact session grant and window capability; the host returns both a cancellation
acknowledgement and the wait's cancelled terminal response. Other requests stay
ordered and are rejected until that wait completes.
`restore`, `show`, and `activate` are scoped through CUA's `bring_to_front`
operation; the returned window state is always revalidated against the exact
PID/HWND target.

`get_session_state` reads CUA's live capture policy. `escalate_session` is a
one-way window-to-desktop scope transition and requires the separate
`allow_session_escalation: true` grant plus one of CUA's bounded escalation
reasons; `resume_session` remains the explicit post-approval restart path.
`cursor_tool` exposes only `set_agent_cursor_enabled`,
`set_agent_cursor_motion`, `set_agent_cursor_theme`, and
`get_agent_cursor_state`; the session id is always injected by Host, so the
mouse-shaped marker cannot be redirected to another session.

Extension tools from the live CUA inventory are available through `call_tool`
only after `open_session` grants `allow_native_tool: true`. The host injects
the exact session PID/HWND/session values from the live SDK schema and rejects
reserved arguments. Click, keyboard, browser, clipboard, recording, and
session-lifecycle tools stay on their dedicated grant-gated routes so the
extension surface cannot bypass observation or approval fences.

The CLI `call` and `host-call` commands accept either `--json JSON` or
`--json-file PATH`; `host-call` reuses the persistent Host endpoint instead of
creating a new CUA driver for each request.
`--json-file -` reads UTF-8 JSON from stdin, keeping large payloads off the
process command line. Host clients use `call_global_tool` for the grant-gated
global CUA tools `check_permissions`, `health_report`, `get_config`,
`set_config`, `replay_trajectory`, and `install_ffmpeg`; window tools continue
through `call_tool` with an exact session capability.

Example host requests:

```json
{"method":"list_apps","params":{}}
{"method":"launch_app","params":{"grant":{"task_grant_id":"task-1","dcc_type":"unreal","allow_app_launch":true},"launch":{"name":"Calculator"}}}
{"method":"call_tool","params":{"session_id":"session-1","task_grant_id":"task-1","window_capability":"cua-window-...","tool":"debug_window_info","arguments":{}}}
{"method":"call_global_tool","params":{"grant":{"task_grant_id":"task-1","dcc_type":"desktop","allow_native_tool":true},"tool":"health_report","arguments":{}}}
```

## Build and test

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
```

The CUA SDK revision is pinned in `Cargo.toml` and `Cargo.lock`. Native desktop
permissions and an interactive session are still required for real capture and
input on Windows, macOS, and Linux.
