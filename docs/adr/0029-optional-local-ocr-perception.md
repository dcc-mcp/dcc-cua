# ADR 0029: Optional local OCR perception for observed windows

## Status

Proposed

## Context

DCC-CUA already treats the captured window image as evidence, but its
structured observation sources are not uniformly available across games,
desktop applications, and remote-rendered surfaces. Accessibility trees,
application APIs, and semantic snapshots remain the preferred sources. They
are unavailable or incomplete for many game and software interfaces.

Consumers need an opt-in way to turn one already-authorized screenshot into
structured text regions. The result must support inspection and bounded action
planning without making OCR an authority to widen a task, select a different
window, or bypass fresh-observation and target-verification requirements.

## Decision

Introduce optional, local OCR perception as a capability of a fresh DCC-CUA
observation. It is disabled by default and is requested per observation, not
persistently for a task or host.

The initial reference implementation targets Windows, macOS, and Linux through
a Rust-owned provider boundary. It downloads a version-pinned CPU inference
runtime and model bundle only after an explicit OCR request. The main package
does not embed a model.

The default bundle is a multilingual, UI-oriented text detector and recognizer
exported to ONNX. A PP-OCR server-grade bundle is the acceptance baseline for
Chinese and English UI text. Implementations may use a smaller bundle when the
caller selects it, but must identify its immutable model manifest in the
response. GPU runtimes and generative vision-language models are out of scope
for the initial provider.

The observation request adds an optional, closed configuration:

```json
{
  "ocr": {
    "enabled": true,
    "bundle": "ui-accurate-v1",
    "languages": ["zh-Hans", "en"],
    "install_if_missing": true
  }
}
```

`bundle`, `languages`, and `install_if_missing` are allowlisted values. Unknown
fields fail closed. Model installation is a download only: it never starts an
automation task, emits input, or changes an application window.

### Internal deployment overrides

Internal pipelines may stage approved bundles without depending on a
user-profile cache. The provider supports these process-start environment
variables:

- `DCC_CUA_OCR_BUNDLE_DIR`: absolute directory containing one or more
  preinstalled, versioned bundles.
- `DCC_CUA_OCR_CACHE_DIR`: absolute writable directory used for the normal
  verified download cache when `DCC_CUA_OCR_BUNDLE_DIR` does not provide the
  requested bundle.

`DCC_CUA_OCR_BUNDLE_DIR` has precedence for lookup; the normal cache remains
the fallback only when installation was explicitly requested. A matching local
bundle must contain its signed release manifest and every artifact must match
the manifest's bundle ID, version, platform, architecture, and SHA-256.
Missing, relative, inaccessible, duplicated, or mismatched directories fail
with a typed OCR-unavailable result. They do not silently fall back to a
different bundle or download a replacement. The runtime canonicalizes the
path before use, rejects traversal outside the configured root, and reports
only the bundle identity and source (`environment`, `cache`, or `download`) in
MCP responses; it never exposes an internal filesystem path.

The environment selects storage only. It cannot select an arbitrary model,
disable verification, alter the caller's closed `bundle` allowlist, enable
OCR, or widen any task/action scope. This keeps hermetic pipeline deployment
compatible with the same supply-chain and automation safety contract as a
user-initiated download.

An OCR-augmented observation returns the normal image identity plus ordered
regions. Each region carries its recognized text, normalized and pixel bounds,
confidence, reading order, language when available, and the exact observation
ID from which it was derived. No region may be reused after its source
observation is stale.

```json
{
  "observation_id": "opaque-observation-id",
  "ocr": {
    "provider": "local-onnx",
    "bundle": {"id": "ui-accurate-v1", "version": "1.0.0", "sha256": "..."},
    "regions": [
      {
        "text": "Confirm",
        "confidence": 0.98,
        "bounds_px": {"x": 842, "y": 716, "width": 98, "height": 34},
        "bounds_normalized": {"x": 0.66, "y": 0.75, "width": 0.08, "height": 0.04},
        "reading_order": 17
      }
    ]
  }
}
```

OCR is a perception hint, not an input primitive. A later action that refers
to an OCR region must be resolved against a fresh observation of the same
PID/HWND, use the existing allowed-method and risk checks, and reject missing,
ambiguous, disabled, or low-confidence candidates. Existing confirmation,
payment, account, CAPTCHA/2FA, stop, expiry, and revocation boundaries are
unchanged.

## Generic UI understanding

The initial capability recognizes text only. It does not claim that every text
box is clickable. Future optional UI-element detectors may return separate
button, icon-button, input, tab, and dialog candidates. They must expose a
distinct provider and confidence result, and pairing a text region with a
control must remain an explicitly verifiable inference.

Model training is cross-application rather than per game. Generic UI datasets
label visual categories such as buttons, input fields, tabs, dialog controls,
and icon buttons. Per-application behavior is stored as a small profile with
text aliases, DPI assumptions, and safety rules; it is not a separately
downloaded model. Low-confidence or user-corrected examples can enter an
opt-in, redacted evaluation corpus for periodic generic-model updates.

## Distribution, privacy, and resource limits

- Bundles are downloaded from an allowlisted release manifest over TLS,
  verified with a pinned SHA-256, atomically installed into a versioned local
  cache, and may be removed by an explicit cache-management command.
- The default accurate bundle should budget roughly 170 MB for models and no
  more than 230 MB including its CPU inference runtime and cache metadata.
  Smaller bundles remain available for constrained machines.
- Screenshots and recognized text remain local. They are never uploaded by the
  OCR provider, and telemetry must be off by default.
- Installation has bounded download size, timeout, disk-space checks,
  single-flight locking, cancellation, rollback, and actionable offline error
  reporting. A corrupt or unverified bundle is never loaded.
- OCR work is cancellable, bounded by image dimensions and elapsed time, and
  cannot block task stop or session cleanup.

## Consequences

- Users gain a provider-neutral structured fallback for UI surfaces without
  accessible semantics.
- The default DCC-CUA binary stays small and existing observation latency and
  privacy behavior are unchanged unless OCR is explicitly requested.
- OCR confidence must be presented as uncertainty. It cannot substitute for
  application APIs, accessibility semantics, target identity, or fresh visual
  verification.
- The provider boundary permits future system OCR, pure-Rust inference, or
  specialized model bundles without changing the public observation contract.

## Validation before implementation

The implementation PR must include:

1. Contract tests for disabled-by-default behavior, closed options, immutable
   bundle identity, coordinate mapping under DPI scaling, reading order, and
   source-observation binding.
2. Downloader tests for manifest/signature verification, checksum mismatch,
   interrupted download, concurrent installation, offline mode, disk limits,
   cancellation, and atomic rollback.
3. Deployment-override tests for precedence, absolute/canonical path handling,
   platform and architecture mismatch, traversal/symlink escape, redacted
   errors, inaccessible roots, and a fully offline preinstalled bundle.
4. Golden screenshot tests covering Chinese and English desktop UI, game HUD
   text, small fonts, scaled windows, overlap, and intentionally ambiguous
   labels. Results must report precision, recall, and coordinate error rather
   than only text-match rate.
5. Safety tests proving stale OCR regions, wrong PID/HWND, low-confidence or
   multiple matches, task expiry, and stop/revocation cannot produce input.
6. Exact-head native CI plus real Windows host acceptance with a packaged
   bundle. Local tests and successful installation are not evidence of a
   correct action on a live target.
