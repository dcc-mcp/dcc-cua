# Paired-tab navigation

## Batches

1. Add a single navigation operation for an already paired, authorized tab.
   Reuse task authorization, native PID/HWND binding, the extension relay and
   DOM snapshot identity. This change adds no browser permission or SDK backend.
2. Separately review discovery of tabs explicitly selected by the user. Existing
   provider status is task scoped; this change does not enumerate browser tabs,
   read other pages, or introduce persistent site permission.
3. Keep native AX and other native browser operations in the official SDK where
   supported. An extension DOM snapshot is not an AX tree. Do not duplicate a
   native backend to make the capability lists look identical.

## Authorization and request

Use one persistent MCP connection and an existing exact native browser target.
The task must grant `browser_extension_call`, its exact browser origin, and this
independent action scope:

```json
{
  "action": "browser_extension_navigate",
  "input_kind": "browser",
  "secret_input": false,
  "authorization_category": "external_effect",
  "browser_origin": "https://example.test"
}
```

A click, typing, observation-only, credential or another-origin grant does not
authorize navigation. Take a fresh task-owned extension snapshot, then call:

```json
{
  "task_id": "RETURNED_TASK_ID",
  "method": "browser_extension_call",
  "params": {
    "provider_id": "RETURNED_PROVIDER_ID",
    "expected_origin": "https://example.test",
    "method": "navigate",
    "params": {
      "snapshot_id": "LATEST_SNAPSHOT_ID",
      "url": "https://example.test/next"
    },
    "timeout_ms": 10000
  }
}
```

The URL must be a bounded absolute HTTP(S) URL without username/password and
must remain within the paired origin. URL hashes, rather than URL text, enter
the action-confirmation value. The Host generates the deadline; callers cannot
override it. The provider must advertise `tab_navigation_v1`. Older extensions
fail with `navigation_unsupported`, without a navigation command being queued.

## Lifecycle and readable result

The Host checks the exact window, foreground input readiness, task lease,
origin and current observation, then consumes the observation. The extension
rechecks the active tab/window, pairing nonce and deadline. Its content bridge
validates the current document URL and snapshot, then clears document identity,
nonce, snapshot and refs. The background removes its pairing before calling
`tabs.update` for that single tab. Fragment and same-URL navigations follow the
same revocation rule. The Host removes only that provider after the admitted
attempt; a subsequent explicit pairing gets a new provider identity. Known
pre-dispatch refusals retain the provider. Once document evidence was consumed,
`navigation_pairing_required` requires re-pairing even if no navigation was issued.

The result reports dispatch, not successful loading or destination acceptance:

```json
{
  "phase": "dispatched",
  "navigation_requested": true,
  "completion_known": false,
  "effect": "unverified",
  "pairing_required": true,
  "fresh_observation_required": true
}
```

The caller must explicitly pair the destination tab again, verify task/provider/
native-window/origin binding, and take a fresh snapshot. Redirects do not grant
another origin. No task, old ref or credential authority migrates automatically.

Before browser dispatch, cancellation, expiration, focus loss, replaced pairing
or stale document evidence refuses the operation. After dispatch, browser
failure, cancellation, disconnect or expiration can leave the outcome unknown;
`navigation_outcome_unknown` must never trigger an automatic retry. A cancelled
request cannot undo an admitted browser operation. Late old-port events cannot
reassign the task or clear a new explicit pairing. There is no automatic retry,
background tab activation or navigation-completion claim.

## Validation gates

Host-free regressions cover strict protocol/schema bounds, provider capability,
Host-owned deadlines, URL origin/credential refusal, independent action leases
and revocation, one-shot document/ref invalidation, duplicate requests,
cancellation/timeout before and after admission, focus loss, disconnect and old
pairing events. The existing SDK is used by the runtime; fake channels and
in-memory browser APIs do not establish live browser readiness.

Live acceptance requires a supported interactive executor, fresh native PID/full
birth/HWND binding, the project's own extension and a compatible candidate Host,
an explicitly selected tab, and a read-only baseline snapshot. On a dedicated
approved test page, navigation must invalidate old evidence, require re-pairing,
and produce a fresh readback only in the authorized origin. Preserve failure and
cleanup receipts. Do not navigate an active user workflow merely to test this
feature. Do not use a generic browser/CUA provider as acceptance evidence.

Older components without the navigation capability remain unsupported for this
operation. This patch does not install or update them. OS error 5 or absent legal
exact-window binding blocks live acceptance; no desktop/session/identity/ACL
switch is a repair.
