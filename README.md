# dcc-mcp-computer-use

Cross-platform Computer Use host and CLI for DCC-MCP, backed by the open-source
[CUA SDK](https://github.com/trycua/cua).

This is the standalone runtime that Core can launch and keep alive for a whole
task. It preserves the Core Computer Use contract:

- exact PID/window/title scope; agent requests cannot widen the target;
- a fresh observation ID is required for every mutation;
- bounded text, key, drag, and coordinate input;
- fail-closed sensitive-window policy;
- explicit stop/resume lifecycle and structured errors;
- a visible CUA mouse-shaped cursor and `DCC UI Control · <app> · Esc to stop`
  marker.

The repository is a Cargo workspace with four responsibilities:

- `dcc-mcp-cua-core`: scoped Computer Use domain, safety policy, and
  CUA execution boundary;
- `dcc-mcp-cua-browser`: exact-window browser binding, tab snapshots, typed
  browser actions, and bounded file transfer;
- `dcc-mcp-cua-host`: long-lived Core-compatible IPC and request
  routing;
- `dcc-mcp-cua-cli`: the thin CLI process that composes the workspace crates.

Application-specific adapters stay above this workspace. A browser adapter can
add tab/DOM/iframe/download capabilities through CDP or WebDriver while using
this host as its visual fallback. Unreal/Fab flows belong in the Unreal or
browser adapter and should combine typed Unreal APIs with scoped CUA; Fab
account, purchase, and download confirmation remain explicit user-approved
operations.

## CLI

```powershell
cargo run -p dcc-mcp-cua-cli -- list --app chrome.exe
cargo run -p dcc-mcp-cua-cli -- apps
cargo run -p dcc-mcp-cua-cli -- launch --name Calculator
cargo run -p dcc-mcp-cua-cli -- doctor
cargo run -p dcc-mcp-cua-cli -- snapshot --app chrome.exe --output screenshot.png
cargo run -p dcc-mcp-cua-cli -- act --app chrome.exe --action-json '{"action":"click","x":100,"y":100}'
```

`list` and `doctor` are read-only. `snapshot` and `act` require one exact
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

## Core host IPC

Start one persistent host process instead of spawning a process per action:

```powershell
cargo run -p dcc-mcp-cua-cli -- host --stdio
cargo run -p dcc-mcp-cua-cli -- host --endpoint '\\.\pipe\dcc-mcp-computer-use-v1'
```

On Unix, the default endpoint is
`$TMPDIR/dcc-mcp-computer-use-v1.sock`. The host uses Core's version-3 framing:
an unsigned big-endian `u32` length followed by one UTF-8 JSON request or
response. A `snapshot` response is followed by one additional length-prefixed
binary PNG frame, avoiding base64 pixel transfer and keeping the JSON frame
under 4 MiB. Requests may include a top-level `request_id` (1–128 characters);
the host echoes it on the JSON response, including errors, so Core can safely
correlate long-lived IPC calls. The handshake advertises the exact capabilities
of this build.

The supported Core request surface is `hello`, `list_apps`, `launch_app`, `open_session`,
`get_window_state`, `change_window_state` (`activate`), `snapshot`,
`accessibility_snapshot`, `find`, `wait_for`, `browser_snapshot`,
`browser_prepare`, `browser_navigate`, `browser_click`, `browser_type`, `browser_pointer`,
`browser_set_input_files`, `browser_download`, `browser_dialog`,
`clipboard_read`, `clipboard_write`, `recording_start`, `recording_stop`,
`recording_state`,
`execute_action`, `resume_session`, `terminate_app`, and
`stop_session`. Semantic actions use CUA `element_index` values from the latest
accessibility snapshot; `set_text`/`set_value` use CUA's native semantic value
path, while coordinate actions remain available for custom-drawn surfaces.
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
Shared-memory descriptors and typed system operations remain explicitly
unsupported until their cross-platform contracts are implemented.

Example host requests:

```json
{"method":"list_apps","params":{}}
{"method":"launch_app","params":{"grant":{"task_grant_id":"task-1","dcc_type":"unreal","allow_app_launch":true},"launch":{"name":"Calculator"}}}
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
