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

## Remaining integration and native gates

The trusted broker/action scope and client permission presentation still need
wiring to the prepared exact target; the old video permission cannot authorize
audio. The existing producer/MP4 track writer must own the native adapter and
audio track through the same stop/finalize result. No capability may advertise
sound before this route is connected and qualified.

Portable tests establish contract and lifecycle behavior only. Generated-file
tests must encode/decode real audio in MP4 with timing/duration checks. Native
acceptance then uses two owned processes: the authorized sound/visual-marker
window and an unrelated owned sound source. It must show a real target audio
track, exclusion of the unrelated marker, bounded A/V offset, cancellation,
disconnect and failure cleanup. Record PID/fullbirth/two HWND identities before
any UI operation and keep explicit cleanup readbacks. Never sample private sound
or rerun a denied OS5 route. This fixture intent is authorized; actual persistent
MCP configuration/output-root changes, permission expansion and interruption of
active tasks/Hosts still need their exact objects and impact confirmed first.
