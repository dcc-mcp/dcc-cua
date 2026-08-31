# Listing copy — draft

Review this copy against the released runtime and client before using it in a
store. It is not a claim that a store submission or a working client integration
already exists.

## Name

DCC-CUA Browser Provider

## Short description

Explicitly pair one browser tab with the project-owned DCC-CUA runtime.

## Description

DCC-CUA Browser Provider connects one explicitly selected desktop browser tab to a
compatible local DCC-CUA native runtime. It supports bounded semantic page
observations and scoped interactions for an authorized task. Click the
extension action in the intended HTTP or HTTPS tab to pair it. Navigating or
closing that tab clears its pairing.

This extension is a companion to DCC-CUA, not a standalone automation service.
The native host and a connected Agent Host with its own sandbox and permission
policy are required. Pairing a tab alone does not bypass those permissions or
start a task. Unsupported clients must stop rather than creating their own
control path.

The extension requests no blanket host permission. It communicates with the
local native host using Native Messaging. Website information returned to the
runtime may subsequently be processed by the client you connect; review the
data practices of that client as well as this extension. Avoid pairing tabs
containing information you do not intend to share with your chosen client.

## Single purpose and permissions

Connect an explicitly paired browser tab to the local DCC-CUA runtime for a
bounded, authorized browser task.

| Permission | Purpose |
| --- | --- |
| `activeTab` | Access the tab deliberately selected through the extension action |
| `scripting` | Inject the bundled tab bridge into that selected tab |
| `nativeMessaging` | Exchange bounded requests and results with the installed local DCC-CUA host |
| `storage` | Keep pairing identity in browser session storage across extension worker restarts |

The extension bundles its executable JavaScript and does not download remote
code. Firefox declares browsing activity, website content and website activity
for its primary function; local transmission must not be described as no data
access. Platform data certifications remain an explicit owner decision.

## Reviewer preparation

Provide the exact released runtime version, supported client integration and
native-host installation instructions. Demonstrate pairing, a bounded action
on a non-sensitive test page, fresh post-action verification, navigation
invalidation and revocation. Do not give reviewers a fabricated grant or a
generic automation fallback. Resolve [#237](https://github.com/dcc-mcp/dcc-cua/issues/237)
before claiming a supported client workflow.

Support: [project issue tracker](https://github.com/dcc-mcp/dcc-cua/issues).
License: MIT. Privacy: [review draft](privacy.md), not yet a published policy.
