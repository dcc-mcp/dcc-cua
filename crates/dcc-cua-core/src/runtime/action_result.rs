use crate::contracts::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult};

pub(super) fn validated_action_effect(
    result: &cua_driver_sdk::ToolResult,
    operation: &str,
) -> ComputerUseResult<String> {
    result
        .action
        .as_ref()
        .and_then(|action| serde_json::to_value(action.effect).ok())
        .and_then(|effect| effect.as_str().map(str::to_owned))
        .ok_or_else(|| {
            ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                format!("CUA {operation} omitted its validated action effect"),
            )
        })
}
