# One executable for CLI and MCP

Status: proposed implementation; linked-artifact and client acceptance pending.

Ship only `dcc-cua`; CLI, MCP, native messaging and Host modes use the existing
shared runtime. The separate background binary duplicated the deployment and
self-update contract for a Windows process-creation concern.

Preserve the selected client's historical `dccCuaTaskAuthorization` registration
key; the runtime's initialize name remains independent. Do not copy permission
policy to a new key or assume this change diagnoses missing tool discovery.

Keep the Console PE subsystem so interactive shells retain CLI waiting semantics.
MSVC Windows builds embed `consoleAllocationPolicy=detached`, effective before
main on Windows 11 24H2 build 26100+ and Server 2025+. Older Windows and GNU
builds require the calling client to set CREATE_NO_WINDOW. Portable MCP manifests
do not express that flag, and support in every Codex/Agent Host launcher has not
been established. This is an explicit compatibility limit, not a universal
console-free-launch claim. See [Microsoft documentation](https://learn.microsoft.com/en-us/windows/console/console-allocation-policy).

Do not attach/hide a console at runtime, introduce a wrapper EXE, change task
authority, or merge independently owned connections. Preserve inherited pipes,
JSON-RPC stdout, stderr diagnostics, exit status, EOF and guardian supervision.
The updater retains its single-writer lock, validates one canonical replacement
and preserves recovery bytes if rollback cannot restore the executable. It never
deletes historical binaries or terminates clients.

Acceptance requires the actual linked Console PE and embedded RT_MANIFEST,
CLI/redirected-pipe behavior, MCP initialize/tools-list/EOF and guardian tests,
exact single-binary archive/install verification, and selected-client runtime
readback. A manifest assertion alone does not prove an observed absence of flash.
