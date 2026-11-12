# MCP connection lifecycle evidence

An MCP stdio bridge belongs to its client connection. It waits for input until
the client closes stdin or transport I/O fails. Several initialized bridges can
therefore outlive individual tool calls while remaining legitimate connections.
Task TTL and connection lifetime are different contracts. This change adds no
idle process timeout, process pool, reconnect, parent termination or cleanup scan.

## Separate the three termination observations

The previous connection record was published as `closed` before awaiting owned
task shutdown, and never recorded that shutdown's result. This left the saved
diagnostics unable to distinguish transport EOF from successful, failed or
interrupted task cleanup.

The additive `shutdown` object now reports:

| Field | Meaning |
| --- | --- |
| `cleanup_status: pending` | Transport ended; owned task cleanup has not returned. |
| `cleanup_status: completed` | All owned task cleanup returned successfully. |
| `cleanup_status: failed` | Cleanup returned at least one failed or unknown task receipt. |
| `cleanup_status: unknown` | No result is available, including older records or a dropped cleanup future. |
| `failed_tasks` | Number of unsuccessful cleanup receipts, when observed. |
| `cleanup_finished_at_unix_ms` | Time cleanup returned; absent when unknown/pending. |

Neither `closed` nor `completed` attests OS process exit or client-side child
reaping. `process_status` remains separately fenced by the complete OS birth
identity. Abrupt process termination cannot write a final cleanup acknowledgement.
Request parameters, error bodies, tokens, paths, window contents and audio are
not copied into this object. Task counts and associated Hosts retain their last
observation at transport disconnect, rather than being rewritten as proof that
unobserved native resources disappeared.

## Attribution and caller responsibility

The existing initialize allowlist preserves only sanitized `clientInfo.name`
and `clientInfo.version`, plus optional `_meta.dcc-cua.chat_id` and `task_id`.
Those identifiers are self-reported metadata, never authorization. Arbitrary
SDK extension fields, credentials, environment variables and private client
session files must not be used to invent attribution. If a client supplies no
chat identifier, diagnostics must continue to report it as unknown.

This repository implements that JSON-RPC/stdin loop directly with Tokio and
serde_json. It does not own the external client's MCP SDK or connection cache;
no client shutdown/reconnect behavior is inferred from SDK branding alone.

An exact PID plus full process birth can associate a bridge with its recorded
parent; it does not prove which chat currently holds the parent's pipe handle.
Client-side connection deduplication and release cannot be repaired by expiring
an otherwise valid idle server. The client must close its owned stdin and await
the exact child handle when discarding a transport. A request timeout alone does
not establish EOF or permit terminating another connection.

## Browser Provider is a separate source

The older provider restores a saved session pairing on background startup and
can call `connectNative` without a fresh toolbar click. Its native relay connects
to the default resident Host; the connection helper does not spawn that Host.
A browser-launched native relay and a client-launched `mcp-server` are distinct
roles even though both use the same executable. A Codex parent proves the
immediate creation source of that bridge, not the absence of all browser activity.

Current source clears the old pairing on startup, disconnects the prior native
port when pairing again, and clears rejected/disconnected pairings. The relay
discovers existing task-owned transports; its discovery timer is not an OS
process spawn loop. These source fixes do not prove that an installed older
provider/runtime has been upgraded or that its actual browser route works.
No extension, native registration, runtime installation or browser is changed by
this diagnostic increment. Native browser pairing, input/navigation and
disconnect cleanup require their own explicit acceptance evidence.

## Verification boundary

The transport seam uses generic async byte streams so EOF, malformed frames,
response disconnect, cancellation, retained connections after a client timeout,
and repeated teardown can be tested without starting a CUA process, native Host,
SDK or application. Existing real-child integration tests separately own and
reap their isolated children. In-memory tests establish transport and receipt
behavior; they do not attest OS child reaping or fix an external client's
reconnection policy.

`MissingWindow` from `IsWindow == 0` on a Host execution thread remains a separate
diagnostic problem. A different CLI process's Default-desktop metadata is not
evidence about that Host thread. This change does not restore desktop binding,
change privileges, retry UI or certify an unknown session's cleanup.
