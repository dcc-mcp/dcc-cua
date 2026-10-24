# Desktop connections and readiness

An installed binary, a working MCP handshake and permission to control a desktop
are separate checks. DCC-CUA must execute in, or connect through a supported
interface to, the authorized desktop session. A client-side installation cannot
grant access to a display, bypass a sandbox restriction or inherit another
tool's desktop permissions.

## Current connection modes

| Entry point | What it connects to | What success establishes |
| --- | --- | --- |
| Local commands such as `dcc-cua doctor` | The runtime in the current process environment | The reported native checks, not access to a different desktop |
| `--endpoint PATH` / `HostClient::connect` | DCC Host local IPC: a Unix socket or Windows named pipe | Transport and Host handshake; desktop readiness is a separate check |
| `--spawn BINARY` / `HostProcess::spawn` | An owned `host --stdio` child with inherited pipes | Child ownership and Host handshake; the child still needs desktop access |
| `dcc-cua mcp-server` / packaged background entry | The MCP client's owned stdio connection; task runtimes live in that process | Protocol initialization and the advertised task tools, not a completed native task |

`--endpoint` is not a URL option. Hierarchical scheme URLs (`scheme://...` and
`scheme:/...`, including `file:/...`) are rejected before transport opening or
handshake. Drive paths and relative Unix filenames containing a colon retain
their existing OS path validation; this check is not a general URI parser.
The error deliberately omits the
supplied value because URLs can contain credentials. Pass the intended local IPC
path; do not replace a failed local socket with a network URL or change its
permissions to get past a refusal.

The lower-level `HostClient::from_stream` API accepts an already-connected stream
for embeddings and tests. It does not implement endpoint discovery, remote
authentication or a remote desktop permission grant. An embedding that supplies
its own transport is responsible for its trust and lifecycle contracts.

## Diagnose the correct layer

1. **Package:** verify the release archive, install manifest and every declared
   payload file. A working primary binary alone does not verify the companion
   background entry, assets or skills.
2. **Connection:** `dcc-cua connections` reads bounded local MCP connection
   records without starting a Host or touching UI. An initialized connection and
   a tool list establish protocol availability only. See the
   [connection diagnostics contract](design/mcp-connection-diagnostics.md).
3. **Executor metadata:** `dcc-cua doctor --context-only` reads current-process
   identity and borrowed Windows station/thread-desktop metadata. It does not
   open the input desktop, query cursor/foreground state, start SDK/Host/UIA or
   attest readiness. It does not inspect an existing MCP process. See the
   [context-only contract and missing launch evidence](context-only-diagnostics.md).
4. **Native environment:** inspect the doctor report's health, permissions and
   route readiness only within the authorized probe scope. Normal `doctor`
   constructs/probes a runtime. `doctor --diagnostics-only` skips SDK startup
   but still queries the input desktop, foreground and cursor surface on Windows;
   it is not a substitute for `--context-only` after an input-surface refusal.
5. **Task acceptance:** perform an authorized, bounded screenshot/input/readback
   task on the exact target. Record cancellation and cleanup separately. Static
   inventory, compilation and an idle stdio shutdown are not substitutes.

On Linux, a display variable is only configuration, not proof that the process
can reach the display or inject input. AT-SPI also needs the appropriate desktop
session bus. On Windows, a service/SSH process and an interactive desktop can be
different sessions. On macOS, desktop consent belongs to the relevant executable
identity. Do not infer native readiness from a platform label or an unrelated
tool that can see a desktop. The current non-Windows
`interactive_desktop_platform_managed` doctor subcheck delegates readiness to
the SDK; its booleans are not independent native observations. Read the combined
route result and SDK health/permission evidence.

If a sandbox rejects local socket access or supplies no desktop interface, stop
at that boundary. A supported environment must expose the authorized connection.
Installing more client dependencies, reading internal tokens, forwarding a
privileged tool through a hidden proxy or changing OS security settings is not
a connection repair.

## Remote support: existing building blocks and limits

The pinned SDK includes remote envelope machinery, but DCC's
`from_test_remote_channel` constructor is feature-gated test support. Its presence
does not create a production remote endpoint. The pinned SDK's
[MCP envelope carrier document](https://github.com/loonghao/cua/blob/3d2bcf50089ea18aa5bedab328b1afb3f710ea00/libs/cua-driver/docs/mcp-envelope-carrier.md)
describes a candidate, not a released guest/Fleet endpoint. Do not remove the test
gate or enable an unauthenticated listener as a deployment shortcut.

Upstream also documents two distinct deployment approaches:

- [Run the driver on the desktop-owning machine and carry MCP over SSH](https://cua.ai/docs/cua-driver/guides/vms-and-remote).
  The remote interactive session still needs its native permissions. This is an
  upstream deployment pattern, not support for an `ssh://` DCC endpoint.
- [Attach to an existing authenticated cua-spacesd service](https://cua.ai/docs/cua-sdk/guides/spacesd).
  This needs an explicitly supplied, authorized service address and credential.
  The [current service](https://github.com/trycua/cua/blob/main/libs/cua-spacesd/README.md)
  is not wire-compatible with the former computer-server. Its existence does not
  imply compatibility with DCC's pinned SDK or access to a platform's private
  desktop service.

Adding a production DCC remote provider requires an explicitly supported and
versioned upstream carrier, authenticated target identity, capability negotiation,
connection-generation fencing, bounded requests/images, cancellation and joined
cleanup. Disconnect/reconnect must revoke old grants and observations without
replaying mutations. Transport negotiation must distinguish unsupported protocol,
authentication failure, unavailable desktop and denied native permissions.
Deploying a new service or creating persistent access is a separate authorization
decision, not a side effect of connection diagnosis.

Acceptance must exercise the final provider on a real authorized desktop. The
[read-only Linux X11 candidate](https://github.com/dcc-mcp/dcc-cua/pull/320) has a
narrow local-pixels contract; it does not establish input support, remote access,
Wayland support or native acceptance on other platforms.
