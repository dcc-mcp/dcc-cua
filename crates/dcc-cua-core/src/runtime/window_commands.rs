#[cfg(not(windows))]
use super::action_result::validated_action_effect;
use super::*;

impl ComputerUseSession {
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
            return Ok(json!({
                "success": true,
                "effect": "confirmed",
                "requested_frame": request,
                "applied_frame": applied,
                "target": target,
                "cua": {"path": "windows_exact_set_window_pos"},
                "text": "Set and verified the exact Windows PID/HWND frame.",
                "degraded": false,
            }));
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
