# ADR 0008: Separate platform input and showcase encoding from Core

## Status

Accepted

## Context

`dcc-cua-core` is the auditable safety shell that binds an exact target,
evaluates policy, gates mutations, and records delivery evidence. It had also
accumulated direct Win32 `SendInput` construction, upstream Windows adapter
calls, held-key cleanup, OpenH264 encoding, and segmented MP4 publication.
Those implementation details created multiple injection paths inside the
policy boundary and linked a native codec into the safety-critical crate.

## Decision

Keep policy and action sequencing in `dcc-cua-core`, but move operating-system
execution into the workspace platform adapter:

- `dcc-cua-platform-windows` owns raw mouse and keyboard packets, cursor
  probing, exact-foreground held-key execution, cleanup, and wrappers around
  the pinned upstream Windows adapter;
- Core receives typed counts, snapshots, and errors and remains responsible
  for target gates, retry semantics, and delivery classification; and
- held-key cleanup attempts every owned key release before reporting the
  aggregate failure.

Add `dcc-cua-showcase` as the recording boundary. It consumes a bounded watch
channel containing shared BGRA frames and typed pause or terminal reasons,
then owns OpenH264, MP4 segmentation, manifests, finalization, and recorder
lifecycle. Core projects live-observation state into that channel without
copying the frame buffer and maps the recorder's typed error at its API edge.

Automated boundary tests reject direct upstream Windows, OpenH264, or MP4
dependencies in Core and reject raw Win32 input APIs in its runtime modules.

## Consequences

- Every workspace-owned Windows-specific input path crosses one platform
  adapter; the platform-neutral SDK remains Core's typed execution contract.
- Unsafe Win32 packet construction and native codec code are outside the Core
  policy boundary and can be audited and tested independently.
- Core retains platform-neutral sequencing and exact-target safety semantics.
- Frame handoff is reference-counted, avoiding a new full-frame copy at the
  extraction boundary.

## Alternatives considered

- Keeping low-level helpers in Core behind private modules was rejected
  because dependency ownership, unsafe code, and cleanup semantics would still
  live in the policy crate.
- Moving only the OpenH264 calls while leaving recorder lifecycle in Core was
  rejected because MP4 finalization and pause/resume segmentation form one
  cohesive media responsibility.
