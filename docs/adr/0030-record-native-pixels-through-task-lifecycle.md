# ADR 0030: Record native pixels through the task lifecycle

## Status

Implemented for source validation. Native decoded-video and target-marker
acceptance remain required; a build or portable encoder test does not establish
capture attribution, geometry, or color accuracy on a live desktop.

## Public contract

The Windows `pixels_only` task can explicitly grant these existing Host methods:

| Method | Request | Ownership |
| --- | --- | --- |
| `live_observation_start` | Optional `request` with `fps` 1–30 and `max_dimension` 256–4096 | Starts or reuses the session's single native worker/watch |
| `live_observation_state` | Empty parameters | Reads producer state and provenance |
| `live_observation_stop` | Empty parameters | Stops that session's producer; an attached recorder receives its truthful terminal state |
| `recording_start` | Optional `request`; `record_video` must be true | Attaches the existing `ShowcaseRecorder` to that producer |
| `recording_state` | Empty parameters | Reads local video/source state, without an upstream trajectory |
| `recording_stop` | Empty parameters | Awaits encoder and sidecar finalization; stops the producer only if recording created it |

A lifecycle start requires its state and stop methods in the same immutable
task scope. Recording additionally requires `allow_recording=true` and all three
recording methods. No input action scope is required for observation or video;
recording permission does not authorize physical input, semantic selectors,
element tokens, clipboard access, or browser operations.

The server operator preconfigures an ordinary, precreated absolute directory in
`DCC_CUA_RECORDING_OUTPUT_ROOT`. Each recording task gets a fresh child directory.
The resulting grant and trusted lease retain the exact directory. MCP callers
cannot configure the root or choose another destination. A supplied
`request.output_dir` must equal the authorized task directory; the server fills
it when omitted. Traversal, alternate streams, UNC/device paths, symlinks, and
reparse ancestors are rejected. The operator must retain ownership of this root;
these checks do not claim an atomic filesystem-handle sandbox against concurrent
changes by another local process.

Example task request (the returned binding must be reported before observation):

```json
{
  "application_label": "Owned native window",
  "surface": "window",
  "observation_mode": "pixels_only",
  "target_process_id": 1234,
  "target_window_handle": 5678,
  "allowed_methods": ["recording_start", "recording_state", "recording_stop"],
  "allowed_actions": [],
  "allow_recording": true
}
```

Then `dcc_cua_task_call` can call `recording_start` with `params: {}`. Recording
uses video only: `trajectory_available=false`, `trajectory=null`, and no
upstream `VisualOnly`, accessibility tree, or recording keepalive is created.
The task's ordinary idle timeout remains in force; recordings do not extend the
lease or session lifetime. Clients can make granted state calls while active.

## Fresh source and first encoded frame

Start requires a frame captured after the request and after the current stream,
action, and pause fences. Returning an older latest frame refuses startup; it
does not assert that a fresh frame was awaited successfully. The encoder's
acknowledgement contains the actual first encoded sequence, capture time,
dimensions, and typed native provenance. Core validates that receipt against the
same target, stream, native instance, pause fence, and post-ready native state.
A frame prepared before subscription cannot stand in for a different encoded
first frame.

The producer reuses exact native capture and publication guards on every frame.
Identity replacement is terminal; minimized or occluded targets pause/refuse
publication. Pause fences survive conflated watch updates. Resume requires a
fresh frame and creates a new encoded segment; paused time is not filled with a
stale frame. The sidecar retains per-sample source sequence, geometry, provenance,
and segment identity. Unavailable WGC timing remains unavailable. Pixels are not
color-corrected or assigned a guessed origin by the recording lifecycle.

## Stop and failures

`recording_stop` drains producer and encoder, retaining both failures if both
occur. Encoder/sidecar errors retain partial paths and never become a successful
finalized outcome. `recording_state` reports local paused, degraded, failed, and
terminal-source evidence even if the target has disappeared.

`stop_task` revokes the input lease first, awaits the exact Host session's
cleanup, and preserves recording cleanup issues. The acknowledgement must name
the same logical session and explicitly report inactive/nonpending cleanup.
Missing, malformed, mismatched, or disconnected acknowledgements are
`cleanup_unknown`; a completed cleanup with failures is `cleanup_failed`.
The Host cleanup request has a 60-second transport deadline. A timeout consumes
the connection and remains unknown; it is not silently retried or declared a
successful finalization. Repeated stops return the retained result. MCP EOF and
transport errors also await owned task cleanup before process exit.

Terminal input/transport invalidation revokes session capability and cached input
evidence before awaiting the same local recorder/source drain. It does not probe
a lost target or an upstream recording owner. Encoder failures retain actual
partial paths and a `recording_stop` cleanup issue. A source worker that cannot
acknowledge shutdown retains `live_observation_stop` with `cleanup_pending=true`.
Cancellation during either drain also leaves its pending flag set. Consuming a
local owner or making a second stop is not proof that cleanup completed; repeated
stops retain these failures or unknown outcomes. A standalone recording stop
preserves a borrowed live source, while terminal session invalidation stops all
sources owned by that session.
Restarting either session mode after pending or failed cleanup refuses before
native or driver probes and requires a new session object. A cleanly completed
session retains its previous reuse contract.

The `session_stopped` acknowledgement adds optional typed `recording_video` and
`live_observation` outcomes when those components existed. Video reports actual
`active`, `finalized`, optional `path`, `manifest_path`, `current_partial`,
`segment_paths`, optional `capture_sidecar` (path, SHA-256, record/frame counts,
finalized), and optional typed `error_code`. Source reports `active`,
`cleanup_complete`, `cleanup_pending`, and the known `stream_id`. Absent components
are omitted. These summaries come from recorder/source outcomes; they do not
infer successful files from path names or expose caller-controlled arbitrary
JSON. The authorized directory remains fixed by the task grant and lease.

## Validation boundaries

Pure tests cover immutable output authorization, wrong target and directory,
revocation, video-only requests, framed cleanup acknowledgements, stale first
frames, actual first encoded sample/sidecar correspondence, pause/terminal
projection, and preservation of encoder failures. Native acceptance must still
decode real output and independently attribute its markers to the exact owned
window, including pause/resume and same-executable ambiguity cases.
