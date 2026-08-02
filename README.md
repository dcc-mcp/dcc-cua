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

This project wraps the CUA SDK for agent-friendly, bounded operations. The
`dcc-mcp-cua` CLI owns its own `update` command and exposes `daemon`, `mcp`, and
`recording render` as first-class entries. Those three entries reuse the
official `cua-driver` executable through `CUA_DRIVER_BIN` rather than copying
its daemon/MCP/render implementation. Use this CLI/Host for the DCC-MCP safety
envelope, exact window capability, fresh observations, grants, friendly actions,
and software-specific adapters.

## Development gates

Every Rust file is limited to 2000 lines. Unit tests live in a sibling
`src/tests.rs` module rather than production files and use `rstest`; the same
layout gate runs before formatting in CI:

```powershell
pwsh -NoProfile -File scripts/check-rust-layout.ps1
cargo fmt --all -- --check
cargo test --workspace --all-targets
```

## CLI

```powershell
cargo run -p dcc-mcp-cua-cli -- list --app chrome.exe --on-screen
cargo run -p dcc-mcp-cua-cli -- wait-window --app UE5Editor.exe --title "PCG Fab" --on-screen
cargo run -p dcc-mcp-cua-cli -- apps
cargo run -p dcc-mcp-cua-cli -- tools
cargo run -p dcc-mcp-cua-cli -- call --tool check_permissions --json '{}'
cargo run -p dcc-mcp-cua-cli -- call --tool set_config --json-file payload.json
cargo run -p dcc-mcp-cua-cli -- desktop-snapshot --output desktop.png
cargo run -p dcc-mcp-cua-cli -- screen-size
cargo run -p dcc-mcp-cua-cli -- cursor-position
cargo run -p dcc-mcp-cua-cli -- desktop-act --action-json '{"action":"click","x":100,"y":100}'
cargo run -p dcc-mcp-cua-cli -- act --app chrome.exe --action-json '{"action":"click","x":100,"y":100}' --output action.png
cargo run -p dcc-mcp-cua-cli -- launch --name Calculator
cargo run -p dcc-mcp-cua-cli -- doctor
cargo run -p dcc-mcp-cua-cli -- snapshot --app chrome.exe --output screenshot.png
cargo run -p dcc-mcp-cua-cli -- accessibility --app chrome.exe
cargo run -p dcc-mcp-cua-cli -- window-state --app chrome.exe
cargo run -p dcc-mcp-cua-cli -- activate --app chrome.exe
cargo run -p dcc-mcp-cua-cli -- click --app chrome.exe --x 100 --y 100
cargo run -p dcc-mcp-cua-cli -- click --app chrome.exe --element-index 12
cargo run -p dcc-mcp-cua-cli -- toggle --app chrome.exe --element-index 14
cargo run -p dcc-mcp-cua-cli -- set-value --app chrome.exe --element-index 15 --value Published
cargo run -p dcc-mcp-cua-cli -- drag --app chrome.exe --from-x 100 --from-y 100 --to-x 300 --to-y 200
cargo run -p dcc-mcp-cua-cli -- type --app chrome.exe --text "hello" --focused
cargo run -p dcc-mcp-cua-cli -- hotkey --app chrome.exe --key CTRL --key L
cargo run -p dcc-mcp-cua-cli -- act --app chrome.exe --action-json '{"action":"click","x":100,"y":100}'
cargo run -p dcc-mcp-cua-cli -- verify --app chrome.exe --expect-json '[{"window":{"exists":true}}]'
cargo run -p dcc-mcp-cua-cli -- update --check
```

`daemon` and `mcp` pass their remaining flags to the official `cua-driver`
binary. `recording render` can be invoked as
`dcc-mcp-cua recording render INPUT_DIR OUTPUT_MP4`; set `CUA_DRIVER_BIN` when
the upstream executable is not on `PATH`. `recording start|stop|status` keeps
the upstream daemon lifecycle, while this project's Host routes remain the
grant-gated DCC-MCP surface.

`list`, `apps`, `tools`, `desktop-snapshot`, `screen-size`, `cursor-position`, and
`doctor` are read-only. `snapshot` and `act` require one exact
target; if an application has multiple windows, pass `--pid` and
`--window-id` instead of relying on an app name.

`wait-window` polls CUA's native window inventory for an app, process ID,
window handle, or exact title. It is capped at 30 seconds and is intended for
launch/switch orchestration before a target-bound action.

`accessibility` reads the current bounded semantic tree without screenshot
pixels. `window-state` and `activate` operate on the same exact target scope.
Long-lived recording remains on the persistent Host session so its start/stop
lifecycle is not lost when a one-shot CLI process exits.

The friendly selector forms also include toggle, drag, set-text, and set-value.
Click, toggle, scroll, press, hotkey, type, and value actions can target the
latest semantic tree with --element-index or --element-token; coordinate
actions remain available for custom-rendered surfaces.

For common controls, `click`, `double-click`, `right-click`, `move`, `scroll`,
`press`, `hotkey`, and `type` build the same fenced CUA actions without manual
JSON. `type` requires `--focused` or an explicit `--element-index`/
`--element-token`; `hotkey` accepts repeated `--key` values.

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
cargo run -p dcc-mcp-cua-cli -- host-batch --json '[{"method":"list_apps","params":{}},{"method":"screen_size","params":{}}]'
cargo run -p dcc-mcp-cua-cli -- host-call --spawn target/debug/dcc-mcp-cua --method list_apps --json '{}'

# Keep one Host connection open and process one JSON request per input line.
cargo run -p dcc-mcp-cua-cli -- host-jsonl --spawn target/debug/dcc-mcp-cua --output-dir artifacts
# Batch only stateless discovery lines within a short window.
cargo run -p dcc-mcp-cua-cli -- host-jsonl --parallel-discovery --spawn target/debug/dcc-mcp-cua
```

`dcc-mcp-cua-client` is the direct embedding path for dcc-mcp-core. It opens
the per-session endpoint, performs `hello`, sends JSON requests, and returns
the following binary image frame without base64 decoding in the control path:

```rust,no_run
let mut host = dcc_mcp_cua_client::HostClient::connect_default("dcc-mcp-core").await?;
let response = host.request("list_windows", serde_json::json!({})).await?;
```

When Core owns the Host lifecycle, use `HostProcess::spawn` to start the CLI
with `host --stdio`, reuse the same negotiated `HostClient`, and call
`shutdown` when the task ends. This keeps process supervision out of Core's
request code while preserving the same protocol as endpoint connections.

```rust,no_run
let mut host = dcc_mcp_cua_client::HostProcess::spawn(
    "dcc-mcp-cua",
    "dcc-mcp-core",
    dcc_mcp_cua_client::SnapshotTransport::SharedMemory,
).await?;
let response = host.client_mut().request("list_apps", serde_json::json!({})).await?;
let _status = host.shutdown().await?;
```

Long waits can use `HostClient::request_with_cancel`; it sends `cancel` on the
same connection and consumes both the cancellation acknowledgement and the
wait terminal response.

Read-only discovery and observation calls can use `HostClient::request_batch`
to write several requests before one flush; responses are returned in request
order. Core callers that need task/turn tracing can use
`HostClient::request_batch_with_ids`, which preserves caller-owned IDs through
the same pipelined write. The Host dispatches stateless discovery calls
(`list_apps`, `list_tools`, `list_windows`, `screen_size`, and
`cursor_position`) concurrently; exact-window and desktop session state stays
serialized. The client matches responses by `request_id`, so completion order
does not change the caller's result order. Mutating methods stay on `request`
so side effects remain explicit. The handshake advertises this as
`pipelined_read_requests` and `parallel_discovery_requests`.

The `host-batch` CLI accepts `{request_id?, method, params}` objects and uses
one persistent Host connection; supplied IDs are echoed, while omitted IDs
receive a deterministic `host-batch-N` ID. If a response contains image
pixels, pass `--output-dir DIR`; metadata-only batches can omit it. `host-call` and
`host-batch` accept `--spawn BINARY` for a one-shot stdio-managed Host when a
supervisor does not already own an endpoint.

`host-jsonl` is the streaming CLI bridge for dcc-mcp-core and scripts. It keeps
one negotiated Host session open, reads one `{request_id?, method, params}`
object per line from stdin, and writes one response object per line to stdout.
When present, `request_id` is preserved end to end for Core task/turn tracing.
Binary image attachments are written to `--output-dir`; `shared_memory` keeps
image pixels out of the Host control pipe.

`--parallel-discovery` batches contiguous `list_apps`, `list_tools`,
`list_windows`, `screen_size`, and `cursor_position` requests for up to 5 ms or
32 lines, preserves input order and `request_id`, and leaves stateful,
visual, browser, and mutating requests serialized.

```text
{"request_id":"core-task-42","method":"list_apps","params":{}}
{"request_id":"core-task-43","method":"list_windows","params":{"on_screen_only":true}}
```

For large or frequent images, use `connect_default_with_transport(...,
SnapshotTransport::SharedMemory)` and open the returned descriptor with
`dcc_mcp_cua_shm::SharedImageReader`; window/desktop snapshots, verification
screenshots, and browser responses containing one image keep pixels out of the
control pipe. Native extension results and browser responses containing multiple images use one
bounded binary attachment frame with offset descriptors.

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

The supported request surface is `hello`, `list_apps`, `list_tools`, `list_windows`, `wait_for_window`, `launch_app`, `open_session`,
`get_window_state`, `change_window_state` (`activate`), `snapshot`,
`accessibility_snapshot`, `verify_state`, `call_tool`, `call_global_tool`, `get_session_state`, `cursor_tool`, `escalate_session`, `find`, `wait_for`, `browser_snapshot`,
`browser_prepare`, `browser_navigate`, `browser_click`, `browser_type`, `browser_pointer`,
`browser_set_input_files`, `browser_download`, `browser_dialog`,
`clipboard_read`, `clipboard_write`, `recording_start`, `recording_stop`,
`recording_state`,
`desktop_snapshot`, `screen_size`, `cursor_position`, `open_desktop_session`,
`desktop_session_snapshot`, `execute_desktop_action`, `stop_desktop_session`,
`zoom`,
`execute_action`, `resume_session`, `terminate_app`, and
`stop_session`; `cancel` is available while `wait_for` is active and
`cancel_window_wait` while `wait_for_window` is active on the same connection.
Semantic actions use CUA `element_index` values from the latest
accessibility snapshot, and `set_text`/`set_value`/`set_checked` use CUA's
native semantic value path. Coordinate actions remain available for
custom-drawn surfaces. For applications that miss fast keystrokes, use
`action: "type_chars"` with `delay_ms` (0..1000); it requires an
`element_index`/`element_token`, or the explicit `type_chars_only: true` when
the target field is already focused. This maps to CUA's cross-platform
`type_text_chars` input path and does not accept screen coordinates.
`list_windows` supports optional `app`, `pid`, and `on_screen_only` filters;
these are applied by the native backend before the response crosses IPC.
`wait_for_window` accepts `query` with `app`, `process_id`, `window_handle`,
`window_title`, and `on_screen_only`; its `timeout_ms` is capped at 30 seconds.
When a request ID is supplied, cancel it on the same connection with
`cancel_window_wait` and `{"wait_id":"<request_id>"}`. The Rust Client's
`wait_for_window_with_cancel` helper sends that route automatically.
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
For an isolated browser, mint the one-use approval interactively with the
upstream `cua-driver browser-approve --pid PID --profile-mode isolated_new`
command, then pass its token to `browser_prepare`; this CLI never mints a
browser approval on behalf of an agent. Existing-profile attachment requires
a trusted Core authorization host created through
`ComputerUseDriver::create_with_authorization_host`; the default runtime and
Host process keep refusing it.
`browser_snapshot` first binds the exact native window, then snapshots a
specific CUA tab; `browser_click`, `browser_type`, and `browser_pointer` require
the latest browser `snapshot_id`, exact binding, and an explicit input route.
`browser_navigate`, `browser_set_input_files`, and `browser_download` invalidate
the tab snapshot. Upload uses `allow_browser_input`; download is a separate
destructive grant (`allow_browser_download`) and CUA's host approval evidence.
`browser_dialog` only resolves page-owned JavaScript dialogs and requires the
exact current `dialog_id` for accept/dismiss.

For native windows whose UIA/AX provider is unavailable (for example a
game-engine editor or custom-rendered DCC surface), `snapshot` first attempts
the semantic window capture. After an explicit `escalate_session` approval,
the same exact-window session may use a CUA desktop visual frame cropped to
the validated PID/HWND bounds. The result is marked
`capture_backend: "cua-driver-sdk-desktop-crop"` and
`accessibility_available: false`; coordinate actions remain observation-bound,
while semantic element actions correctly remain unavailable.
`desktop_snapshot` is a full-display visual discovery surface; it does not
widen an existing window session or grant desktop-wide mutation. For an
explicit desktop input scope, use `open_desktop_session`, then take a fresh
`desktop_session_snapshot` and call `execute_desktop_action`; raw input grant
and the exact desktop capability are required.
Action responses preserve the CUA SDK's structured result, text, degraded
status, and image attachments. Host transports image bytes as the negotiated
binary attachment, so Core does not need to decode base64 on its control path.
While `wait_for` is running, the same connection accepts `cancel` with the
exact session grant and window capability; the host returns both a cancellation
acknowledgement and the wait's cancelled terminal response. Other requests stay
ordered and are rejected until that wait completes.
`activate` is scoped through CUA's `bring_to_front` operation; the returned
window state is always revalidated against the exact PID/HWND target.

`zoom` is a typed, read-only crop of the latest window `snapshot`. It requires
the matching `observation_id`, keeps the exact PID/HWND binding, limits the
requested width to 500 pixels, and returns the JPEG through the negotiated
binary or shared-memory image transport. This is useful for dense DCC/UE/Fab
panels where the full observation is resized.

Window actions accept CUA's `element_token` as an alternative to
`element_index`; when both are supplied the token wins. They also accept
`delivery_mode: "background" | "foreground"`. The default is `background`,
which preserves the user's foreground and lets CUA select its accessibility or
synthetic-event route. Use `foreground` only after CUA reports that the
background route is unavailable.

`get_session_state` reads CUA's live capture policy. `escalate_session` is a
one-way window-to-desktop scope transition and requires the separate
`allow_session_escalation: true` grant plus one of CUA's bounded escalation
reasons; `resume_session` remains the explicit post-approval restart path.
`cursor_tool` exposes `move_cursor`, `set_agent_cursor_enabled`,
`set_agent_cursor_motion`, `set_agent_cursor_theme`, and
`get_agent_cursor_state`; `move_cursor` is forced to `scope: "window"`, and the
session id is always injected by Host, so the mouse-shaped marker cannot be
redirected to another session or move the real system pointer.

Extension tools from the live CUA inventory are available through `call_tool`
only after `open_session` grants `allow_native_tool: true`. The host injects
the exact session PID/HWND/session values from the live SDK schema and rejects
reserved arguments. Click, keyboard, browser, clipboard, recording, and
session-lifecycle tools stay on their dedicated grant-gated routes so the
extension surface cannot bypass observation or approval fences.

The CLI `call` and `host-call` commands accept either `--json JSON` or
`--json-file PATH`; `host-call` reuses the persistent Host endpoint instead of
creating a new CUA driver for each request. `host-call --output FILE` consumes
both binary-frame images and negotiated shared-memory image descriptors.
`--json-file -` reads UTF-8 JSON from stdin, keeping large payloads off the
process command line. Host clients use `call_global_tool` for the grant-gated
global CUA tools `check_permissions`, `health_report`, `get_accessibility_tree`, `get_config`,
`set_config`, `replay_trajectory`, and `install_ffmpeg`; window tools continue
through `call_tool` with an exact session capability.

Example host requests:

```json
{"method":"list_apps","params":{}}
{"method":"launch_app","params":{"grant":{"task_grant_id":"task-1","dcc_type":"unreal","allow_app_launch":true},"launch":{"name":"Calculator"}}}
{"method":"call_tool","params":{"session_id":"session-1","task_grant_id":"task-1","window_capability":"cua-window-...","tool":"debug_window_info","arguments":{}}}
{"method":"zoom","params":{"session_id":"session-1","task_grant_id":"task-1","window_capability":"cua-window-...","request":{"observation_id":"session-1-obs-1","x1":120,"y1":80,"x2":420,"y2":220}}}
{"method":"call_global_tool","params":{"grant":{"task_grant_id":"task-1","dcc_type":"desktop","allow_native_tool":true},"tool":"health_report","arguments":{}}}
```

## Build and test

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
```

## CI/CD and release

CI checks layout, formatting, workspace tests, the locked release build, and a
CLI help smoke test on Windows, Linux, and macOS. The release workflow packages
the `dcc-mcp-cua` binary as platform archives and attaches them to the GitHub
release.

The release gate is intentionally closed while the product is still being
completed. Do not set the repository variable `DCC_MCP_CUA_RELEASE_READY=true`
until the full computer-control goal is accepted. The initial manifest starts
at `0.0.0`, so the first release-please release will be `0.1.0`; after that
release-please owns the manifest version and conventional-commit changelog.

The workflow uses the official `cargo-workspace` release-please plugin so all
workspace crates and `Cargo.lock` stay aligned. It does not publish crates;
`publish = false` remains intentional.

The CUA SDK revision is pinned in `Cargo.toml` and `Cargo.lock`. Native desktop
permissions and an interactive session are still required for real capture and
input on Windows, macOS, and Linux.
