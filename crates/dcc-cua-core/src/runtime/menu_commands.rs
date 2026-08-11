use super::action_result::validated_action_effect;
use super::*;

impl ComputerUseSession {
    /// Invoke one exact native application-menu path without pixel targeting.
    pub async fn invoke_menu(
        &mut self,
        request: &ComputerUseMenuRequest,
    ) -> ComputerUseResult<Value> {
        request.validate()?;
        self.ensure_active()?;
        let target = self.preflight_mutating_bound_tool().await?;

        // Menu invocation may mutate before an unverifiable result is returned.
        self.invalidate_action_observations();
        let result = self
            .call_bound_tool_without_refresh(
                "invoke_menu",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "path": &request.path,
                }),
            )
            .await;
        let result = self.finish_observation_sensitive_attempt(result)?;
        let effect = validated_action_effect(&result, "invoke_menu")?;
        let result = native_tool_result(result)?;
        let target = self.require_observed_target_available().await?;
        self.require_observed_input_available()?;
        self.target = Some(target.clone());
        let success = matches!(effect.as_str(), "confirmed" | "unverifiable");

        Ok(json!({
            "success": success,
            "effect": effect,
            "verification_required": effect != "confirmed",
            "observation_required": true,
            "path": request.path,
            "target": target,
            "cua": result.value,
            "text": result.text,
            "degraded": result.degraded,
        }))
    }
}
