#[cfg(not(windows))]
use super::action_result::validated_action_effect;
use super::*;

impl ComputerUseSession {
    /// Request a polite close for the exact Windows PID/HWND target.
    /// This never terminates the owning process.
    pub async fn close_window(&mut self) -> ComputerUseResult<Value> {
        self.ensure_active()?;
        if self.scope.process_id.is_none() || self.scope.window_handle.is_none() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidTarget,
                "close requires an exact process_id and window_handle grant binding",
            ));
        }
        #[cfg(windows)]
        {
            let target = self.require_observed_target_available().await?;
            self.invalidate_action_observations();
            dcc_cua_platform_windows::post_close_window(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                || Ok(()),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("close the exact Windows target", error)
            })?;
            Ok(json!({
                "success": true,
                "effect": "confirmed",
                "target": {"process_id": target.pid, "window_handle": target.window_id},
                "cua": {"path": "windows_exact_post_wm_close"},
                "process_terminated": false,
                "fresh_observation_required": true,
            }))
        }
        #[cfg(not(windows))]
        Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "exact polite window close is currently available only on Windows",
        ))
    }

    /// Set and independently revalidate the exact target window frame through CUA.
    pub async fn set_window_frame(
        &mut self,
        request: &ComputerUseWindowFrameRequest,
    ) -> ComputerUseResult<Value> {
        request.validate()?;
        self.ensure_active()?;
        #[cfg(windows)]
        {
            self.require_observed_window_activation_available()?;
            let target = self.require_observed_target_available().await?;
            let requested = [
                request.x.round() as i32,
                request.y.round() as i32,
                request.width.round() as i32,
                request.height.round() as i32,
            ];
            self.invalidate_action_observations();
            let applied = dcc_cua_platform_windows::set_window_frame(
                dcc_cua_platform_windows::UiaTarget {
                    process_id: target.pid,
                    window_handle: target.window_id,
                },
                requested,
                || windows_platform_window_activation_gate("set_window_frame"),
            )
            .map_err(|error| {
                map_windows_window_mutation_error("set the exact Windows target frame", error)
            })?;
            let target = self.require_observed_target_available().await?;
            self.target = Some(target.clone());
            Ok(json!({
                "success": true,
                "effect": "confirmed",
                "requested_frame": request,
                "applied_frame": applied,
                "target": target,
                "cua": {"path": "windows_exact_set_window_pos"},
                "text": "Set and verified the exact Windows PID/HWND frame.",
                "degraded": false,
            }))
        }

        #[cfg(not(windows))]
        {
            let target = self.preflight_mutating_bound_tool().await?;

            // The backend may mutate before a timeout or partial result is observed.
            self.invalidate_action_observations();
            let result = self
                .call_bound_tool_without_refresh(
                    "set_window_frame",
                    json!({
                        "pid": target.pid,
                        "window_id": target.window_id,
                        "x": request.x,
                        "y": request.y,
                        "width": request.width,
                        "height": request.height,
                    }),
                )
                .await;
            let result = self.finish_observation_sensitive_attempt(result)?;
            let effect = validated_action_effect(&result, "set_window_frame")?;
            let result = native_tool_result(result)?;
            let target = self.require_observed_target_available().await?;
            self.require_observed_input_available()?;
            self.target = Some(target.clone());
            let success = effect == "confirmed";

            Ok(json!({
                "success": success,
                "effect": effect,
                "requested_frame": request,
                "target": target,
                "cua": result.value,
                "text": result.text,
                "degraded": result.degraded,
            }))
        }
    }
}
