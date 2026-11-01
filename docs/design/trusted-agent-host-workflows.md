# Trusted Agent Host workflows

DCC-CUA follows ADR 0028: the connected Agent Host decides user authorization through its own sandbox and tool policy. The packaged MCP server does not add action popups. A task request is not a permission-mode switch: no `bypass`, grant receipt, executable, environment, or policy field can turn on a capability.

The existing four-tool lifecycle remains `start_task`, `dcc_cua_task_call`, `task_status`, `stop_task`. Each task has one exact process/window and independent method/action/origin scopes, expiration, fresh observation checks, and revocation. Ordinary native actions in authorized tasks are checked against the action scope as well as the method scope. Ordinary login applications can be observed only when a constructor-owned lease explicitly includes the corresponding account-access or credential action; OS login, security applications, command interpreters, UAC and secure desktops remain outside that capability.

## Bootstrap

Owned installation and normal uninstallation are opt-in Windows capabilities. The trusted Agent Host supplies all three environment variables when it starts a fresh `dcc-cua mcp-server` process:

- `DCC_CUA_WORKFLOW_POLICY`: an operator-owned local policy file.
- `DCC_CUA_ENVIRONMENT_ID`: the identity of this sandbox or local machine scope.
- `DCC_CUA_AGENT_HOST_SESSION_ID`: the trusted parent session identity.

The policy is read once before accepting MCP calls. Its environment/session must exactly match bootstrap, and its deadline must be in the future. Use distinct host-supplied bindings for a cloud sandbox and a user's workstation. Do not derive these values from a webpage, model-generated task, installer content, or other untrusted input. Protect the bootstrap file and environment with the same boundary as the Agent Host process. There is no model-visible tool for installing or replacing the policy. No configuration preserves the existing behavior; partial or invalid configuration fails startup.

Policy version 1 has this shape (paths, timestamp and hashes are placeholders; the host must supply a future deadline no more than 24 hours from startup):

```json
{
  "version": 1,
  "environment_id": "disposable-sandbox",
  "session_id": "trusted-parent-session",
  "expires_at_unix_ms": 1800000000000,
  "recipes": [{
    "id": "install-test-app",
    "kind": "install",
    "executable": "C:\\isolated-fixture\\setup.exe",
    "executable_sha256": "<64 lowercase hex characters>",
    "arguments": ["--test-root", "C:\\isolated-fixture\\artifacts"],
    "working_directory": "C:\\isolated-fixture\\artifacts",
    "artifact_root": "C:\\isolated-fixture\\artifacts",
    "result": {"relative_path": "installed/app.bin", "present": true, "sha256": "<64 lowercase hex characters>"},
    "allowed_exit_codes": [0],
    "timeout_ms": 60000,
    "allow_cancel": true,
    "gui": true,
    "allowed_methods": ["snapshot", "get_window_state", "change_window_state", "execute_action"],
    "allowed_actions": [{"action":"click", "input_kind":"semantic", "secret_input":false, "authorization_category":"task_grant"}]
  }]
}
```

`uninstall` recipes use an explicitly authorized normal uninstaller executable and require `result.present=false`. They are consumed once per connection. Uninstall never means killing an application or deleting arbitrary software directories. Version 1 does not discover or execute registry uninstall strings. The launcher rejects command interpreters/system executables, inherited elevation, reparse/device paths and unsupported platforms. It pins the approved executable and directory identity, checks its SHA-256 before launch, supplies fixed arguments and a minimal child environment, retains its native process handle, and imposes a bounded total lifetime.

## Run and verify

Call `start_task` with the usual surface, allowed methods/actions and optional observation mode, plus `workflow_recipe_id`. Omit PID/HWND and `owned_browser_launch`. The requested scopes must be subsets of the immutable recipe. The complete observation/action contract, available task capacity, and GUI recipe flag are checked before launching any executable. DCC-CUA derives a unique onscreen window from the owned live process and opens a normal exact-window task. Report the returned provider/runtime/PID/HWND before the first observation or input.

Use normal snapshot/action calls for installer controls and progress. After the installer replaces its window, call `dcc_cua_task_call` with `method="workflow_rebind"` and `params={}`. Only one unique window of the same retained live process is accepted. The old lease is revoked and its session cleaned up before a new exact-window capability is issued; all prior observations become stale. Issuance and rebind retain the original minimum of the run and policy monotonic deadlines; neither a new window nor wall-clock rollback extends input or live-capture authority. Ambiguity, another process, or unconfirmed cleanup fails closed. Only direct EXE installers/uninstallers are covered. MSI launchers, separate processes, process trees, elevation brokers, and registry discovery require additional contracts and are unsupported here.

`task_status.workflow` and `workflow_status` report `state`, `process_id`, `process_exited`, `exit_code`, `result_verified` and a fixed reason code. Completion requires both an allowed process exit code and independently verified presence/absence (and optional digest) under the trusted artifact root. Verification pins each existing ordinary parent directory without write/delete sharing. A missing parent gives an immediate absence result without reopening a deeper path; a junction or other reparse parent is refused. `running` and `verifying` are pending states. `succeeded`, `failed`, `timed_out`, `cancelled`, `expired` and `outcome_unknown` distinguish outcomes. A deadline, lost observation or cancellation request never implies success or rollback. Reports always state `rollback_verified=false`; a recovery starts a newly authorized task after checking the actual result.

`workflow_cancel` or `stop_task` revokes input and requests termination only when `allow_cancel` authorizes cleanup of the owned process. Poll status for confirmed exit. If cancellation is not allowed or cannot be confirmed, report `outcome_unknown` and retain the process handle for observation/reaping. Deadlines are enforced independently of status polling. Connection cleanup reports remaining uncertainty. There is no automatic reinstall, rollback, retry, escalation, or broader target search.

## Login and human boundaries

Reuse an existing authenticated session when authorized. When credentials are needed, use the constructor-owned `HostSecretVault` and opaque `secret_handle` interface from ADR 0022. Credentials are resolved only at final dispatch, never passed as tool plaintext, logged, or included in workflow recipes/results. The policy file is not a credential store. Existing keyring failures remain typed and fail closed.

Verify login through fresh destination state and the application's actual authenticated session. A form submission or input acknowledgement is insufficient. The workflow layer does not infer successful authentication from arbitrary page text. When the agent/host encounters 2FA, CAPTCHA, new legal terms, system permission, or unavailable credentials, call `dcc_cua_task_call` with `method="require_user_action"` and a closed `params.reason`: `two_factor`, `captcha`, `new_legal_terms`, `system_permission`, or `credential_unavailable`. The server revokes the lease, reports `needs_user_action`, and rejects every further automatic input/rebind/start attempt on that task. It cannot be resumed by a task-supplied flag; stop it and obtain fresh Agent Host authorization and observations after the user handles the boundary. This checkpoint reduces authority and does not grant permission from page content.

## Validation

Unit/contract tests cover missing/invalid bootstrap, expired/revoked leases, method/action scope, cross-session replay, prompt-supplied switches, exact target identity, lifecycle ambiguity and fixed human-boundary statuses. Owned-process tests use a disposable subprocess and real temporary artifacts; these are process E2E, not GUI E2E.

`scripts/test_owned_workflow_e2e.py --binary <candidate-dcc-cua>` builds a disposable WinForms fixture and drives the candidate MCP → Host → Core → native provider path. Installation/uninstallation affect only its marked temporary root. It verifies progress, same-process window replacement, process exit, final artifacts, failure, timeout and cancellation. The login acceptance fixture uses a loopback-only server and a test constructor-owned vault; report its executed route and any skipped browser tests separately. Cross-platform compilation and actual provider acceptance remain required before release; a Draft PR is not a runtime deployment.
