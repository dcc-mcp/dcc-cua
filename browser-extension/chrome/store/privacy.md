# Extension data practices — owner review draft

This describes the extension source in this repository. It is not an approved
privacy policy or a certification about the complete deployed runtime/client.
The project owner must verify the release's data path, approve a final policy
and provide an accessible policy URL before a store submission.

## Data used for the requested function

After the user selects the extension action in a tab, the extension records a
pairing nonce, browser tab/window identifiers, origin and document identifier
in browser session storage. Pairing is cleared on navigation, tab closure or
an explicit unpair command. Session storage can survive an extension worker
restart; it is not a permanent browsing-history database.

For an authorized task, the bundled tab bridge can return bounded page metadata
and semantic elements, including the current URL, title, labels, roles, state
and geometry. URLs and page labels may themselves contain personal information.
The extension also handles scoped interaction requests and their results.
Snapshot naming avoids using form-control values as fallback labels, and
password input is restricted; these controls are not a promise that arbitrary
website content contains no sensitive information.

## Recipients and retention

The extension sends requests/results through the browser's Native Messaging
channel to the installed local DCC-CUA host. It does not contain a direct
analytics, advertising or remote-upload service. The host and the connected
agent client are separate components: they may retain or transmit information
under their own configuration and policies, including to a remote model
service. Do not describe the complete workflow as local-only without verifying
those components.

The extension keeps pairing metadata in session storage and processes page
results in memory. This statement does not specify retention by the native
host, logs, browser, operating system, connected client or remote service.
Confirm and disclose those practices for the actual distributed integration.

## User controls and support

Only pair a tab whose information may be shared with the intended client.
Navigate or close the tab to clear pairing, revoke the task through its trusted
host integration, or disable/uninstall the extension to stop using it. Removing
the extension does not delete copies already received by other components.

Report extension issues through the
[project issue tracker](https://github.com/dcc-mcp/dcc-cua/issues). Do not include
passwords, tokens, personal page contents or unredacted diagnostics in a public
issue.
