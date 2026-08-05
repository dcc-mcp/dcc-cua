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
        let target = self.revalidate_target().await?;

        // The backend may mutate before a timeout or partial result is observed.
        self.observation = None;
        let result = self
            .call_bound_tool(
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
            .await?;
        let effect = validated_action_effect(&result, "set_window_frame")?;
        let result = native_tool_result(result)?;
        let target = self.revalidate_target().await?;
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
