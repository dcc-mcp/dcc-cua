# Context-only executor diagnostics

`dcc-cua doctor --context-only` is a local, synchronous metadata command. It
returns before platform startup, asynchronous runtime construction, SDK discovery
or Host/session creation. It accepts that one flag only: no target selectors,
endpoint, route, duplicate flag, value, or combination with `--diagnostics-only`.

On Windows it queries only its own PID, full process birth, current TID, process
session ID and borrowed station/thread-desktop metadata. Birth is a decimal
`windows_filetime_100ns` string, preserving all 100 ns FILETIME ticks since the
Windows epoch without rounding through JSON floating point. It does not open
foreign processes, inspect tokens or read the Agent Host's configuration.

The metadata port cannot express input operations. Its Windows implementation
uses current-process identity APIs and borrowed-handle `GetUserObjectInformationW`
queries (`UOI_NAME`, `UOI_IO`). Borrowed handles are never closed or rebound.
It does not call `OpenInputDesktop`, `GetCursorPos`, `GetCursorInfo`,
`GetForegroundWindow`, desktop setters or a capture/input API. No cursor position,
foreground HWND, screenshot, recording, theme state or permission mutation is produced.

## Report interpretation

- `mode=context_only`, `read_only=true`, `diagnostic_report_complete=true`
  identify a completed diagnostic report, not a ready native runtime.
- `checks.execution_context.status=collected` means the requested metadata was
  returned. `partial` preserves missing metadata and error text; unavailable
  booleans remain null rather than false. Other platforms report `not_run` for
  the Windows context without constructing a platform runtime.
- `checks.execution_context.data.thread_desktop.receives_input` describes the
  borrowed desktop's `UOI_IO` result. Even true does not establish capture,
  recording, input permission or task authorization.
- `readiness_assessed=false`, `ready=false`, `input_ready=false` and
  `native_acceptance=not_run` prevent metadata collection from becoming a
  readiness or acceptance claim. No automatic retry hint is emitted.
- Input-desktop, cursor/input-surface, foreground, SDK, permissions, capture and
  recording checks explicitly remain `not_run` with reason
  `outside_context_only_scope`. The input desktop is not opened or compared.
- Session ID identifies the process's session; it does not claim the session is
  active/unlocked. There is no inference from a desktop name or error 5 to a
  particular sandbox, ACL, UAC state or launch policy.

`doctor --diagnostics-only` retains its existing behavior: no SDK/Host startup,
but Windows input-desktop/foreground/cursor probes do run. The normal `doctor`
is a broader native readiness probe. Neither is automatically invoked by this
mode, nor should either be retried after a refusal without its own authorized scope.

## Agent Host launch evidence remains separate

The command reports its own process/thread, not the context of another MCP
server, a prior exited doctor, an Agent Host parent or a target application.
`agent_host_binding.status=not_established` and `missing_evidence` explicitly
retain the integration gaps:

1. Actual Agent Host product/version and its documented supported launch mode.
2. Exact parent and launched-child PID plus full birth identity.
3. Executable hash and source revision of the launched runtime.
4. Actual startup desktop policy/fields and inherited handle policy.
5. Context of the thread that actually executes MCP/native work.
6. Original native refusal/security context, when investigating a prior failure.

The MCP task Host is in-process; a task's target PID/HWND does not switch that
executor's desktop. A transport handshake, user grant, installation or version
upgrade does not grant Win32 access. A separate local doctor is not proof of an
existing MCP server's environment. Retain original refusal receipts and list
missing evidence instead of borrowing a different caller's working desktop.

The integration owner must review the product's supported normal launch/binding
path before any functional recovery change. Reconnection alone is not a promised
repair. No automatic desktop/session switch, privilege change, ACL edit, proxy,
alternate executor or recording retry is part of this command. Any later native
acceptance needs a fresh exact target scope and an independently allocated desktop.
