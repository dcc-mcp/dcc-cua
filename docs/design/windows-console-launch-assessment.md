# Windows process roles and console-flash assessment

Status: source assessment; current installed-client attribution and native
acceptance remain unverified. No running provider, Host or browser was changed.

## Process roles

One `dcc-cua` executable serves several entry points. A process count alone does
not establish duplication or a leak.

| Role | Owner and lifetime | Responsibility |
| --- | --- | --- |
| MCP stdio bridge | The external client's connection, until stdin EOF or transport failure | JSON-RPC, request authorization and logical task routing |
| Host | The resident execution service | Native execution and task state on its multithreaded runtime |
| Browser native relay | The browser's explicitly paired native-messaging connection | Relay browser requests to the existing Host |

The MCP bridge uses a current-thread runtime; the Host uses a multithreaded
runtime. These are executable modes, not separate SDK products. The browser
extension currently clears stale pairing at startup and calls `connectNative`
after explicit toolbar pairing. A rejected handshake or disconnect clears that
pairing; it does not schedule an automatic reconnect loop. A replaced port is
disconnected. This describes the checked-in extension, not every previously
installed extension version.

Task TTL is not the client's connection lifetime. Transport EOF, completion of
owned-task cleanup, OS child exit and client-side child reaping are distinct
observations. Closing a valid idle bridge or adding a timer would alter its
contract without proving the cause of a console flash.

## Launch policy and its limits

The Host command uses `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` with null
stdin/stdout/stderr. The registry helper uses `CREATE_NO_WINDOW`, and the SDK
compatibility source preserves its bounded helper launch flags.

The main Windows artifact keeps the Console PE subsystem for CLI behavior.
Its MSVC manifest requests `consoleAllocationPolicy=detached`, supported on
Windows 11 build 26100 and later and Server 2025. Older Windows and GNU artifacts
also depend on the external caller using `CREATE_NO_WINDOW`. Source flags and a
manifest do not prove the actual linked artifact or the caller's creation flags.
See [the console policy ADR](../adr/0018-single-executable-console-policy.md).

Do not hide a console after creation, add a wrapper, expire independently owned
connections, or change plugin permissions as a speculative repair. Such changes
can obscure evidence or disrupt pipes without correcting the responsible launch.

## Evidence still needed

The reported intermittent flash has not been correlated with one exact process
birth, executable hash, parent launch and console ownership. Historical parent
and bridge receipts establish only their recorded observations; they cannot
attribute a new flash to a particular chat or current client pipe.

A future authorized observation should bind the event to the complete process
birth identity and parent, linked executable/manifest, caller creation policy,
and transport EOF/cleanup/exit/reaping records. Missing evidence remains unknown.
This assessment performs no global process or desktop scan and does not retry a
previously refused access path. Driver 0.38 adoption is not evidence that the
reported flash has been fixed.

The separately reported Max refusal is also preserved: a UIA timeout and
`visible_desktop_proof/target_outside_desktop` for bounds `[0,0,3841,2281]` do not
authorize clipping the proof, moving the window, or substituting whole-desktop
capture. Source maintenance does not clear that original owner's native gate.

## Source references

- `crates/dcc-cua-cli/src/host_lifecycle.rs`
- `crates/dcc-cua-cli/src/owned_process.rs`
- `crates/dcc-cua-cli/src/async_runtime.rs`
- `crates/dcc-cua-cli/windows.manifest`
- `browser-extension/chrome/entrypoints/background.ts`
- `browser-extension/chrome/tests/background-pairing.test.ts`
- [MCP connection lifecycle evidence](mcp-connection-lifecycle-evidence.md)
