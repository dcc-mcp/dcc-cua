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
