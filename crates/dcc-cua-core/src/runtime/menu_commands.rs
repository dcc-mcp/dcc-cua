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
        let target = self.revalidate_target().await?;

        // Menu invocation may mutate before an unverifiable result is returned.
        self.observation = None;
        let result = self
            .call_bound_tool(
                "invoke_menu",
                json!({
                    "pid": target.pid,
                    "window_id": target.window_id,
                    "path": &request.path,
                }),
            )
            .await?;
        let effect = validated_action_effect(&result, "invoke_menu")?;
        let result = native_tool_result(result)?;
        let target = self.revalidate_target().await?;
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
