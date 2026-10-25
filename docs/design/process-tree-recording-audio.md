# Bounded target-process playback audio

This draft first adds the shared grant contract and a recorder-side admission/
cleanup owner. It does not yet expose audio through MCP, install a native source,
or claim that the shipped video-only recording route has acquired sound.

`RecordingAudioMode` defaults to `none`; the only positive mode is
`target_process_tree`. It accepts no caller PID/device, microphone, desktop mix,
or parent-process expansion. The independent trusted lease must bind PID, process
creation time, HWND, window thread and class identity. Parsing a grant DTO alone
does not authorize capture. An issuer must obtain that identity from the existing
authorized exact video window, never from caller audio parameters. Its immutable
duration and combined recording-output ceiling are bounded before activation.
The existing recording owner's immutable output directory remains authoritative.

Process loopback requires Windows build 20348 or later, per Microsoft's
`AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` contract. Unknown and older builds refuse;
there is no microphone, device-loopback or desktop fallback.

The portable `ProcessAudioSession` uses the existing `RecordingAuthorization`
admission fence. Source preparation must not stream. Its effective binding is
checked before `start`; every packet checks the exact native identity, bounded
stereo PCM packet size, monotonic timestamps, duration and remaining output
budget. The video media epoch is supplied explicitly. The mux port must enforce
the remaining combined output budget before any write, including overhead.
It is the existing recorder's track/finalization owner, not another screen
recording stack. Capture discontinuity, invalid identity, writer failure,
revocation and budget exhaustion are terminal. Stop revokes admission, stops the
source and then finalizes the mux exactly once. Failed startup also retains the
cleanup receipt; failure to stop/finalize remains `cleanup_pending`, without a
capture restart.

## Dependency decision

The official `cua-media-codec` package at
`62d1dc3779c66655b8669d85dc5d63b7035d683a` declares `FSL-1.1-MIT`, and its own
LICENSE contains competing-use restrictions and a future MIT conversion. It
cannot be treated as an immediately MIT-licensed dependency. No FSL source or
restricted dependency is copied/added by this draft, and this work does not
accept those terms on the operator's behalf.

The selected compatible capture candidate is `wasapi 0.26.0`: MIT, Rust 1.85,
with an application-loopback activation API. Only its exact-target include-tree
path may be adapted. Its process-enumeration example and desktop capture paths
are not the product route. A thin adapter must retain a native process handle,
revalidate birth/window relation, handle silent buffers and discontinuity flags,
use the QPC timestamp, and bound startup/cancellation/stop. Backend qualification
is still required before it can be enabled.

For the existing MP4 mux, AAC is the compatible track type; the official media
component's Opus encoder is not a drop-in MP4 audio track here. `rusty_aac 1.1.0`
(Apache-2.0, Rust 1.85) is a candidate for isolated generated-file encoding/
decoding tests; its buffering and codec delay must be bounded and measured before
adoption. These are version/file-specific findings, not a claim that the Driver
SDK already exposes audio.

Primary sources:

- https://github.com/trycua/cua/blob/62d1dc3779c66655b8669d85dc5d63b7035d683a/libs/cua/crates/cua-media-codec/LICENSE
- https://docs.rs/wasapi/0.26.0/wasapi/struct.AudioClient.html
- https://docs.rs/crate/rusty_aac/1.1.0
- https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ns-audioclientactivationparams-audioclient_process_loopback_params

## Connected route and remaining native gates

The independent closed action is `recording_audio` / `playback_audio` /
`secret_input=false` / `authorization_category=recording_audio`, without a
browser origin. `start_task` additionally requires `allow_recording=true`,
`observation_mode=pixels_only`, an exact window, the complete recording
start/state/stop methods and:

```json
"recording_audio": {
  "mode": "target_process_tree",
  "max_duration_ms": 20000,
  "max_output_bytes": 33554432
}
```

The operator supplies the pre-created `DCC_CUA_RECORDING_OUTPUT_ROOT` when
starting a new task-local MCP process. The task caller cannot choose a root,
audio PID, device or microphone. The issuer reads only the specified window's
PID/birth/thread/class identity, copies that immutable grant through the broker
and Host lease, and derives `recording_start.request.recording_audio`. Silent
video grants retain their existing default; either direction of mode override
is refused. Alternate platforms refuse process audio.

The Windows adapter uses one hidden helper of the same executable, with private
bounded framed pipes, immutable grant, QPC deadline and a kill-on-close job.
The parent retains its actual `Child` process handle and birth. This isolation
is necessary because the selected WASAPI library's asynchronous activation can
wait without a cancellation bound. Stop never searches numeric PIDs: it requests
normal stream shutdown, then terminates only that owned child if necessary,
and reads its exit and reader join. Forced or unknown cleanup cannot publish a
successful audio recording. No global device/source or desktop mix is selected.

The existing encoder owns that audio controller and the existing MP4 writer.
The complete file (video, audio, headers and seek patches) has the grant's byte
ceiling. Packets are stereo PCM16 at 48 kHz, with a 4800-sample ceiling; silent
buffers are tested before dereferencing. Unknown target metadata, timestamp
error, packet discontinuity or device-position gap ends the source. Raw QPC is
mapped to the actual first encoded video frame's monotonic epoch, without
speed changes. Audio mode requests 30 fps for a new live video source; actual
source fps remains reported. A video pause ends combined capture rather than
resuming on a different media clock. Bounded PCM is AAC-encoded into track 2
of the existing writer after source stop; source errors preserve the partial
and do not become a successful silent file. Content-free worker evidence and
codec priming are retained separately in the recording manifest.

Portable/generated-file tests establish contract, mux and lifecycle behavior
only. The independent generated-file decode showed about 20 ms additional onset
from AAC priming (1024 samples at 48 kHz), not a measured native A/V offset.
No trim, pad, stretch or marker redefinition may hide that delay.

Native acceptance still requires two newly owned processes: the authorized
sound/visual-marker window and an unrelated owned sound source. Before starting,
record PID/fullbirth/HWND and cleanup ownership, define raw decoded audio onset
minus decoded video marker PTS and a separate clock/codec error budget. Verify
target-tone presence, unrelated-tone exclusion, natural speed, normal stop,
cancel/disconnect/fault and exact owned-resource exit. A valid source/container
is not proof of isolation or synchronization. No private audio, microphone,
OS5 retry, parent-tree expansion, active Host interruption or permissions change
is authorized by this implementation. Persistent/shared MCP changes remain a
separate operator action. This PR remains draft and is not merged or released.
