//! Recipe selection is an Agent Host bootstrap capability, never a tool-side bypass.
use super::*;
use crate::workflow::WorkflowManager;

pub(super) fn startup_policy() -> Result<WorkflowManager, std::io::Error> {
    let path = std::env::var_os("DCC_CUA_WORKFLOW_POLICY");
    let environment = std::env::var("DCC_CUA_ENVIRONMENT_ID").ok();
    let session = std::env::var("DCC_CUA_AGENT_HOST_SESSION_ID").ok();
    match (path, environment, session) {
        (None, None, None) => Ok(WorkflowManager::disabled()),
        (Some(path), Some(environment), Some(session)) => {
            WorkflowManager::from_policy_file(std::path::Path::new(&path), &environment, &session)
                .map_err(std::io::Error::other)
        }
        _ => Err(std::io::Error::other(
            "workflow bootstrap requires policy, environment and Agent Host session bindings",
        )),
    }
}

impl TaskAuthorizationServer {
    pub(super) async fn start_workflow_task(
        &mut self,
        mut arguments: Value,
    ) -> Result<Value, String> {
        let recipe = required_string(&arguments, "workflow_recipe_id")?.to_owned();
        arguments
            .as_object_mut()
            .ok_or("task input must be an object")?
            .remove("workflow_recipe_id");
        let request: PrepareTaskInput = serde_json::from_value(arguments.clone())
            .map_err(|_| "invalid owned workflow task request")?;
        if request.surface != TaskSurface::Window
            || request.target_process_id.is_some()
            || request.target_window_handle.is_some()
            || request.owned_browser_launch.is_some()
            || !request.allowed_browser_origins.is_empty()
            || request.allow_recording
            || request.allow_capture_preparation
            || request.allow_native_capture_popup_keyboard
            || !(1..=MAX_TTL_MINUTES).contains(&request.ttl_minutes)
        {
            return Err("owned workflow requires the window surface without caller targets, browser, recording or secondary capture permissions".into());
        }
        start_contract::validate_task_start(&request, true)?;
        self.ensure_task_capacity()?;
        self.workflows.gui_recipe_authorizes(
            &recipe,
            &request.allowed_methods,
            &request.allowed_actions,
        )?;
        let started = self.workflows.start(&recipe)?;
        let workflow_id = required_string(&started, "workflow_id")?.to_owned();
        let outcome = async {
            let (pid, hwnd) = self.derive_workflow_window(&workflow_id).await?;
            arguments["target_process_id"] = json!(pid);
            arguments["target_window_handle"] = json!(hwnd);
            let prepared = self.prepare_scoped_task(arguments, Some(&workflow_id))?;
            let task_id = required_string(&prepared, "task_id")?.to_owned();
            let proposal = self
                .proposals
                .get_mut(&task_id)
                .ok_or("workflow task was not retained")?;
            proposal.workflow_id = Some(workflow_id.clone());
            let mut started = match self.start_task(json!({"task_id":task_id})).await {
                Ok(started) => started,
                Err(error) => {
                    let _ = self.revoke_task(json!({"task_id":task_id})).await;
                    return Err(error);
                }
            };
            started["workflow"] = self.workflows.status(&workflow_id)?;
            started["workflow"]["environment_id"] = json!(self.workflows.environment_id());
            started["workflow"]["agent_host_session_id"] = json!(self.workflows.session_id());
            Ok::<_, String>(started)
        }
        .await;
        match outcome {
            Ok(started) => Ok(started),
            Err(error) => {
                let cleanup = self.workflows.cancel(&workflow_id)
                    .unwrap_or_else(|_| json!({"state":"outcome_unknown","reason":"workflow_startup_cleanup_unconfirmed"}));
                Err(format!("{error}; owned workflow cleanup: {cleanup}"))
            }
        }
    }

    async fn derive_workflow_window(&mut self, workflow_id: &str) -> Result<(u32, u64), String> {
        let driver =
            ComputerUseDriver::create().map_err(|_| "workflow discovery provider unavailable")?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let pid = self.workflows.process_id(workflow_id)?;
            let rows =
                tokio::time::timeout_at(deadline, driver.list_windows_filtered(Some(pid), true))
                    .await
                    .map_err(|_| "owned workflow window discovery timed out")?
                    .map_err(|_| "owned workflow window discovery unavailable")?;
            if let Some(hwnd) = unique_owned_window(&rows, pid)? {
                // Recheck the retained process handle after native enumeration.
                if self.workflows.process_id(workflow_id)? == pid {
                    return Ok((pid, hwnd));
                }
                return Err("owned workflow process identity changed".into());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(
                    "owned workflow did not expose one exact window before the deadline".into(),
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    pub(super) fn stop_task_workflow(&mut self, arguments: &Value) -> Option<Value> {
        let id = arguments.get("task_id")?.as_str()?;
        let workflow = self.proposals.get(id)?.workflow_id.as_deref()?;
        Some(self.workflows.cancel(workflow).unwrap_or_else(|_| {
            json!({
                "state":"outcome_unknown", "reason":"workflow_cleanup_not_authorized_or_unconfirmed"
            })
        }))
    }

    pub(super) async fn workflow_call(&mut self, arguments: Value) -> Result<Value, String> {
        let id = required_string(&arguments, "task_id")?.to_owned();
        let method = required_string(&arguments, "method")?;
        if !arguments
            .get("params")
            .is_some_and(|params| params.as_object().is_some_and(|params| params.is_empty()))
        {
            return Err("workflow lifecycle methods accept only empty params".into());
        }
        let proposal = self
            .proposals
            .get(&id)
            .ok_or("workflow task not found on this connection")?;
        if proposal.revoked
            || proposal.user_action_required.is_some()
            || proposal.registration.expires_at_unix_ms <= unix_time_millis()
        {
            return Err("workflow task is stopped or expired".into());
        }
        let workflow = proposal
            .workflow_id
            .clone()
            .ok_or("task does not own a workflow")?;
        match method {
            "workflow_status" => self.workflows.status(&workflow),
            "workflow_cancel" => {
                let result = self.workflows.cancel(&workflow)?;
                self.revoke_task(json!({"task_id":id})).await?;
                Ok(result)
            }
            "workflow_rebind" => {
                let (pid, hwnd) = self.derive_workflow_window(&workflow).await?;
                let proposal = self
                    .proposals
                    .get_mut(&id)
                    .ok_or("workflow task not found")?;
                if proposal.registration.target
                    == (TrustedTaskAuthorizationTarget::ExactWindow {
                        process_id: pid,
                        window_handle: hwnd,
                    })
                {
                    return Ok(proposal_payload(&id, proposal, "started"));
                }
                // Close the old exact-window lease before issuing a new capability.
                proposal.workflow_id = None;
                let stopped = self.revoke_task(json!({"task_id":id})).await;
                self.proposals
                    .get_mut(&id)
                    .ok_or("workflow task not found")?
                    .workflow_id = Some(workflow.clone());
                let stopped = stopped?;
                if stopped["status"] != "stopped" {
                    return Err("previous workflow window cleanup is unconfirmed".into());
                }
                self.workflows.authorizes_process(&workflow, pid)?;
                let deadline = self.workflows.authorization_deadline(&workflow)?;
                let proposal = self
                    .proposals
                    .get_mut(&id)
                    .ok_or("workflow task not found")?;
                proposal.registration.target = TrustedTaskAuthorizationTarget::ExactWindow {
                    process_id: pid,
                    window_handle: hwnd,
                };
                proposal.receipt = Some(
                    self.authority
                        .issuer
                        .register_with_monotonic_deadline(proposal.registration.clone(), deadline)
                        .map_err(|_| "workflow target lease could not be issued")?,
                );
                proposal.revoked = false;
                proposal.cleanup = None;
                proposal.session_open_attempted = false;
                let mut result = self.start_task(json!({"task_id":id})).await?;
                result["fresh_observation_required"] = json!(true);
                result["old_observations_revoked"] = json!(true);
                Ok(result)
            }
            _ => Err("unknown owned workflow lifecycle method".into()),
        }
    }
}

impl TaskAuthorizationServer {
    pub(super) async fn validate_workflow_process(&mut self, task_id: &str) -> Result<(), String> {
        let Some(proposal) = self.proposals.get(task_id) else {
            return Ok(());
        };
        let Some(workflow) = proposal.workflow_id.clone() else {
            return Ok(());
        };
        let expected_pid = match proposal.registration.target {
            TrustedTaskAuthorizationTarget::ExactWindow { process_id, .. } => process_id,
            _ => return Err("owned workflow lost its exact process binding".into()),
        };
        match self.workflows.process_id(&workflow) {
            Ok(pid) if pid == expected_pid => Ok(()),
            _ => {
                let cleanup = self.revoke_task(json!({"task_id":task_id})).await;
                Err(format!(
                    "owned workflow process is no longer authorized; no input dispatched; cleanup: {}",
                    cleanup.unwrap_or_else(|_| json!({"status":"cleanup_unknown"}))
                ))
            }
        }
    }
}

fn unique_owned_window(rows: &[Value], pid: u32) -> Result<Option<u64>, String> {
    let windows = rows
        .iter()
        .filter(|row| row["pid"].as_u64() == Some(u64::from(pid)))
        .filter_map(|row| row["window_id"].as_u64().filter(|id| *id != 0))
        .collect::<std::collections::BTreeSet<_>>();
    match windows.len() {
        0 => Ok(None),
        1 => Ok(windows.first().copied()),
        _ => Err("owned workflow window identity is ambiguous".into()),
    }
}

#[cfg(test)]
mod tests;

pub(super) fn extend_tool_schema(tools: &mut [Value]) {
    let schema = &mut tools[0]["inputSchema"];
    schema["properties"]["workflow_recipe_id"] = json!({
        "type":"string", "minLength":1, "maxLength":128,
        "description":"Select an immutable recipe supplied by the trusted Agent Host at MCP startup. No executable, arguments, path, environment or permission mode can be supplied by this tool. Windows only."
    });
    schema["oneOf"] = json!([
        {"required":["target_process_id","target_window_handle"],"not":{"anyOf":[{"required":["owned_browser_launch"]},{"required":["workflow_recipe_id"]}]}},
        {"required":["owned_browser_launch"],"not":{"anyOf":[{"required":["target_process_id"]},{"required":["target_window_handle"]},{"required":["workflow_recipe_id"]}]}},
        {"required":["workflow_recipe_id"],"properties":{"surface":{"const":"window"}},"not":{"anyOf":[{"required":["target_process_id"]},{"required":["target_window_handle"]},{"required":["owned_browser_launch"]}]}}
    ]);
    schema["allOf"][0]["then"]
        .as_object_mut()
        .expect("schema object")
        .remove("required");
    schema["allOf"][0]["then"]["anyOf"] = json!([
        {"required":["target_process_id","target_window_handle"]}, {"required":["workflow_recipe_id"]}
    ]);
    let description = tools[0]["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    tools[0]["description"] = json!(format!(
        "{description} Owned install/uninstall workflows select only a trusted startup recipe. task_status verifies the owned process exit code and independent final artifact state; successful input alone is never completion."
    ));
    let description = tools[3]["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    tools[3]["description"] = json!(format!(
        "{description} Owned recipe tasks additionally support workflow_status, workflow_cancel and workflow_rebind with empty params. Rebind derives one unique window of the same retained live process, revokes the old target lease and requires fresh observation. require_user_action with a closed reason (two_factor, captcha, new_legal_terms, system_permission, credential_unavailable) pauses all automatic input and requires a fresh task authorization to resume."
    ));
}

impl TaskAuthorizationServer {
    pub(super) fn require_user_action(&mut self, arguments: Value) -> Result<Value, String> {
        let id = required_string(&arguments, "task_id")?;
        let params = arguments
            .get("params")
            .and_then(Value::as_object)
            .ok_or("human boundary requires reason params")?;
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .ok_or("human boundary requires a typed reason")?;
        if params.len() != 1
            || ![
                "two_factor",
                "captcha",
                "new_legal_terms",
                "system_permission",
                "credential_unavailable",
            ]
            .contains(&reason)
        {
            return Err("unknown human boundary reason or fields".into());
        }
        let proposal = self
            .proposals
            .get_mut(id)
            .ok_or("task not found on this connection")?;
        if proposal.revoked || proposal.registration.expires_at_unix_ms <= unix_time_millis() {
            return Err("task is stopped or expired".into());
        }
        let receipt = proposal
            .receipt
            .as_ref()
            .ok_or("task runtime lease was not issued")?;
        self.authority
            .issuer
            .revoke(&receipt.authorization_id)
            .map_err(|_| "task pause revocation failed")?;
        proposal.user_action_required = Some(reason.to_owned());
        let mut result = proposal_payload(id, proposal, "needs_user_action");
        result["reason"] = json!(reason);
        result["automatic_input"] = json!(false);
        result["fresh_authorization_required"] = json!(true);
        Ok(result)
    }
}
