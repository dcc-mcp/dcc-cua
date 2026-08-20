use super::*;

pub(super) const BROWSER_STATE_CALL_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn browser_tool_timeout(
    name: &str,
    _arguments: &serde_json::Map<String, Value>,
) -> Duration {
    if name == "browser_prepare" || name == "get_browser_state" {
        // Existing-profile binding may re-prove the endpoint and spend up to
        // 32 seconds on the one bounded, consent-aware reconnect. Semantic
        // snapshots also perform several individually bounded CDP proofs.
        // Keep their read-only outer timeout above those contracts instead of
        // cancelling the driver while it still owns a live evidence request.
        BROWSER_STATE_CALL_TIMEOUT
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
        self.finish_observed_tool_attempt(&context, result)
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
                    cua_driver_sdk::worker::ActionCompletion::NotStarted => "not_started",
                    cua_driver_sdk::worker::ActionCompletion::Completed => "completed",
                    cua_driver_sdk::worker::ActionCompletion::Unknown => "unknown",
                };
                Err(ComputerUseError::new(
                    ComputerUseErrorCode::BackendUnavailable,
                    format!(
                        "{context}: {reason}; phase=evidence_dispatch; evidence_completion={completion}; action_attempted=false; local_session_invalidated=false; session_remains_active=true; automatic_input=false; blind_retry=false"
                    ),
                ))
            }
            Ok(Err(error)) => Err(map_driver_error(context, error)),
            Err(error) => Err(ComputerUseError::new(
                error.code,
                format!(
                    "{}; phase=evidence_dispatch; action_attempted=false; local_session_invalidated=false; session_remains_active=true; automatic_input=false; blind_retry=false",
                    error.message
                ),
            )),
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
