use super::*;

pub(super) const BROWSER_TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn browser_tool_timeout(
    name: &str,
    _arguments: &serde_json::Map<String, Value>,
) -> Duration {
    if name.starts_with("browser_") || name == "get_browser_state" {
        // Existing-profile binding may re-prove the endpoint and spend up to
        // 32 seconds on the one bounded, consent-aware reconnect. Semantic
        // snapshots also perform several individually bounded CDP proofs.
        // Browser mutations revalidate the same evidence before dispatch.
        // Keep the outer timeout above those contracts instead of cancelling
        // the driver during a live typed browser request. Completion semantics
        // remain unchanged and no timeout is retried automatically.
        BROWSER_TOOL_CALL_TIMEOUT
    } else {
        INPUT_CALL_TIMEOUT
    }
}

impl ComputerUseSession {
    /// Call one of CUA's typed browser tools within this exact native window.
    ///
    /// The allow-list is deliberate: browser adapters must not turn the Core
    /// host into an arbitrary CUA command proxy. CUA still owns browser target,
    /// tab, ref, origin, and input-trust validation.
    pub async fn call_browser_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        validate_native_tool_request(name, &arguments)?;
        let route = browser_tool_route(name, &arguments).ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                format!("browser tool {name:?} is not exposed by this host"),
            )
        })?;
        self.ensure_active()?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "browser tool arguments must be a JSON object",
            )
        })?;
        match route {
            BrowserToolDisposition::ReadOnlyEvidence => {
                self.refresh_upstream_session_before_observation_if_needed()
                    .await?;
            }
            BrowserToolDisposition::PotentialMutation => {
                self.require_current_upstream_session_for_evidence()?;
            }
        }
        let _banner_activity = self.begin_banner_activity(banner_activity_for_bound_tool(name));
        let target = self.require_observed_target_available().await?;
        if browser_tool_requires_input(name, &arguments) {
            self.require_observed_input_available()?;
        }
        let publishes_browser_evidence =
            route == BrowserToolDisposition::ReadOnlyEvidence || name == "browser_prepare";
        object.insert("session".into(), json!(self.session_id));
        if name == "browser_prepare"
            || (name == "get_browser_state" && !object.contains_key("target_id"))
        {
            object.insert("pid".into(), json!(target.pid));
            object.insert("window_id".into(), json!(target.window_id));
        }
        let timeout = browser_tool_timeout(name, &object);
        if publishes_browser_evidence {
            self.require_observed_exact_window_observation_available()?;
        }
        let result = self
            .dispatch_browser_tool(route, name, Value::Object(object), timeout)
            .await?;
        if publishes_browser_evidence {
            self.require_observed_exact_window_observation_available()?;
            self.require_observed_target_available().await?;
        }
        let value = serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {name} returned invalid JSON: {error}"),
            )
        })?;
        self.set_banner_activity(BannerActivity::Ready);
        Ok(value)
    }

    pub(super) async fn dispatch_browser_tool(
        &mut self,
        route: BrowserToolDisposition,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> ComputerUseResult<cua_driver_sdk::ToolResult> {
        let context = format!("call CUA {name}");
        let result = await_input_call(
            self.driver
                .driver
                .call_tool(name.to_owned(), arguments.to_string()),
            timeout,
            &context,
        )
        .await;
        let result = match route {
            BrowserToolDisposition::ReadOnlyEvidence => {
                self.finish_read_only_evidence_dispatch_result(&context, result)
            }
            BrowserToolDisposition::PotentialMutation => {
                self.finish_typed_dispatch_result(&context, result).await
            }
        };
        let result = self.finish_observation_sensitive_attempt(result)?;
        if name == "browser_navigate" && result.is_error {
            return Ok(result);
        }
        self.finish_observed_tool_attempt(&context, Ok(result))
    }

    pub(super) fn finish_read_only_evidence_dispatch_result<T>(
        &self,
        context: &str,
        result: ComputerUseResult<Result<T, cua_driver_sdk::DriverError>>,
    ) -> ComputerUseResult<T> {
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(cua_driver_sdk::DriverError::ActionInterrupted { completion, reason })) => {
                let completion = match completion {
                    cua_driver_sdk::worker::ActionCompletion::NotStarted
                    | cua_driver_sdk::worker::ActionCompletion::Completed => {
                        ComputerUseCompletionState::Known
                    }
                    cua_driver_sdk::worker::ActionCompletion::Unknown => {
                        ComputerUseCompletionState::Unknown
                    }
                };
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    format!("{context}: {reason}"),
                )
                .with_details(ComputerUseErrorDetails {
                    phase: Some(ComputerUseErrorPhase::EvidenceDispatch),
                    action_attempted: Some(false),
                    input_sent: Some(ComputerUseInputState::NotSent),
                    completion: Some(completion),
                    local_session_invalidated: Some(false),
                    session_remains_active: Some(true),
                    automatic_input: Some(false),
                    blind_retry: Some(false),
                    ..Default::default()
                }))
            }
            Ok(Err(error)) => Err(map_driver_error(context, error)),
            Err(error) => {
                let timed_out = error.details.as_ref().and_then(|details| details.timed_out);
                Err(
                    ComputerUseError::new(error.code, error.message).with_details(
                        ComputerUseErrorDetails {
                            timed_out,
                            phase: Some(ComputerUseErrorPhase::EvidenceDispatch),
                            action_attempted: Some(false),
                            input_sent: Some(ComputerUseInputState::NotSent),
                            local_session_invalidated: Some(false),
                            session_remains_active: Some(true),
                            automatic_input: Some(false),
                            blind_retry: Some(false),
                            ..Default::default()
                        },
                    ),
                )
            }
        }
    }

    /// Call the one browser destructive tool through CUA's trusted adapter
    /// ingress. The approval evidence is created here, never accepted from
    /// caller JSON.
    pub async fn call_browser_download_tool(
        &mut self,
        arguments: Value,
    ) -> ComputerUseResult<Value> {
        validate_native_tool_request("browser_download", &arguments)?;
        self.ensure_active()?;
        self.require_current_upstream_session_for_evidence()?;
        self.require_observed_target_available().await?;
        self.require_observed_input_available()?;
        let mut object = arguments.as_object().cloned().ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "browser download arguments must be a JSON object",
            )
        })?;
        object.insert("session".into(), json!(self.session_id));
        object.insert(
            "_cua_browser_download_mcp_host_approved".into(),
            Value::Bool(true),
        );
        let context = "call CUA browser_download";
        let result = await_input_call(
            self.driver
                .driver
                .call_tool_from_trusted_adapter("browser_download", Value::Object(object)),
            INPUT_CALL_TIMEOUT,
            context,
        )
        .await;
        let result = self.finish_typed_dispatch_result(context, result).await;
        let result = self.finish_observed_tool_attempt(context, result)?;
        serde_json::from_str(&result.raw_json).map_err(|error| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA browser_download returned invalid JSON: {error}"),
            )
        })
    }
}
