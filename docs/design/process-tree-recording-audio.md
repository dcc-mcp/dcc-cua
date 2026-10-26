# Bounded target-process playback audio

This draft exposes an independent `recording_audio` option on `start_task`,
adds the closed `playback_audio` action scope, and connects a Windows WASAPI
worker to the existing recorder and MP4 writer. Native acceptance remains
unqualified; no installed runtime or shipped recording route is changed.

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

Every recorder has a local revocation fence that also holds the task fence.
Stopping/failing one recording revokes only the local fence; task revocation
still rejects all local admissions. Another explicitly requested recording can
reuse a valid task without inheriting the previous recording's revoked state.
The existing encoder owns that audio controller and the existing MP4 writer.
The complete file (video, audio, headers and seek patches) has the grant's byte
ceiling. Packets are stereo PCM16 at 48 kHz, with a 4800-sample ceiling; silent
buffers are tested before dereferencing. Unknown target metadata, timestamp
error, flagged packet discontinuity or QPC rollback/overlap ends the source. Raw QPC is
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


### Virtual process-loopback packet clock

The Windows process-loopback virtual client was observed returning device position
zero for consecutive 480-frame packets while their first-sample QPC timestamps
advanced by exactly 100,000 units (10 ms). Treating that device counter as an
advancing frame index falsely rejected the second packet. Device position remains
in diagnostics; it is not the admission clock for this virtual source.

Packet ordering uses strictly increasing first-sample QPC timestamps and rejects
overlap beyond one PCM frame of quantization at48kHz. A forward QPC interval is
retained on the shared media epoch, not interpreted as a documented glitch by
itself. The existing mux preserves non-sampled intervals as explicitly generated
zero PCM rather than splicing subsequent audio earlier. Receipts distinguish
captured frames from generated timeline-gap frames and retain bounded examples
with complete aggregate counts; those intervals are not claimed as captured
silence or proof that no unflagged loss occurred. Later DATA_DISCONTINUITY, TIMESTAMP_ERROR, unknown flags,
invalid packet bounds and target/grant failures still terminate capture. Failure
receipts retain the prior packet clock, current packet metadata, API stage/HRESULT
and stream-stop result without retaining PCM or identifying unrelated devices.

Microsoft's [GetBuffer contract](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer)
defines the first-sample QPC output in 100 ns units and makes the device-position
output optional. The constant-zero behavior above is an observed qualification
result, not a claim that the generic API documents it for every source.

### Capture during owned native input

The control banner keeps a capability-local pool of exclusion epochs. A native
input epoch owns its exact renderer suppression and registered-cursor exclusion
through key release and final input postcheck. Its live capture source may borrow
those same guards without reacquiring the cross-process exclusive file gate.
Borrowing performs no presenter suppression or cursor mutation: all overlays must
already be hidden, and live publication revalidates that condition after readback.
A visible peer fails closed. The process/tree/window grant and physical input
fence remain unchanged, including the maximum20ms sleep interval between held-key checks.

An explicit snapshot may share only a capture epoch, whose ordinary global
exclusion guard remains alive. It cannot borrow an input epoch. The live producer
and explicit snapshot can share a capture epoch without waiting for each other's
readback. Pools come only from the same private banner capability; another task
or process still waits on the original cross-process gate and unchanged1s budget.
No request field can supply a pool or borrow another task's guards.

Closing an epoch owner stops new borrowing. Existing capture work retains the
actual guards until publication drains, then cursor/presenter restoration and
gate release occur once. Cancellation of the async input waiter does not end the
blocking input worker's guard. Real capture failure still pauses eligibility;
a video pause with audio still ends that recording prefix rather than silently
splicing out wall time. A finalized prefix is not complete gameplay acceptance,
and interrupted audio/video tail differences must remain visible in evidence.

### WGC source time and publication cost

WGC video uses the compositor's actual `SystemRelativeTime`, mapped to the
process monotonic clock through a QPC sample bracket of at most 2 ms. The raw
source QPC, publication QPC, publication UTC time and full calibration bracket
travel with each frame into its recorded sidecar and first-frame acknowledgement.
The mapping uncertainty is at most half the bracket plus 100 ns quantization;
UTC is a diagnostic projection, not the admission clock. Freshness and startup
use the earliest possible source time within that uncertainty bound; an
estimated midpoint cannot admit pixels that may predate the request. Publication after
validation cannot refresh old pixels. Missing/future/unreliably calibrated source
time pauses eligibility; repeated or backwards source QPC cannot become a new
sample. Startup still requires source capture time at or after its request.
Verified-visible and prepared routes do not invent a compositor clock.

Each admitted WGC frame also carries disjoint wall-clock phase costs in
microseconds: the initial recording-authorization precheck, exclusion acquisition, backend capture (including source wait,
readback and recovery), surrounding source validation, recording-authorization
admission and final publication validation. Exclusion acquisition includes its
presenter work; admission includes both parent and recording-local fences.
These are phase durations, not CPU utilization or isolated mutex wait times.
The precheck precedes the existing capture-cycle metric; owner handoffs,
scheduling between cycles, clock mapping and status serialization are outside
those phases. Live status exposes
the latest phase record; immutable sidecars retain the full admitted history.
All identity, visibility, authorization, cancellation and key-release checks keep
their existing order and protections. A shared source epoch alone does not prove
content-level A/V sync or matching end boundaries; retain real gaps, codec
priming and track-duration differences for acceptance.
