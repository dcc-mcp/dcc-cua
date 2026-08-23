use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmedActionEvidenceRefresh {
    NotRequired,
    AccessibilityObservation,
    FreshSnapshotRequired,
}

pub(crate) fn confirmed_action_evidence_refresh(
    action: &HostAction,
    refresh_due: bool,
) -> ConfirmedActionEvidenceRefresh {
    if !refresh_due {
        return ConfirmedActionEvidenceRefresh::NotRequired;
    }
    let accessibility_only_raw_text = action.input_kind == "raw_input"
        && matches!(action.action.as_str(), "type" | "type_chars")
        && action.element_index.is_none()
        && action.element_token.is_none()
        && action.x.is_none()
        && action.y.is_none()
        && action.path.is_empty();
    if accessibility_only_raw_text {
        ConfirmedActionEvidenceRefresh::AccessibilityObservation
    } else {
        ConfirmedActionEvidenceRefresh::FreshSnapshotRequired
    }
}

pub(crate) fn rebind_confirmed_action_evidence(
    action: &HostAction,
    confirmed_action_value: &Value,
    expected_target: ConfirmationWindowIdentity,
    previous: &dcc_cua_core::ComputerUseObservation,
    refreshed: &dcc_cua_core::ComputerUseObservation,
    refreshed_accessibility_root: &Value,
) -> ComputerUseResult<String> {
    let same_exact_target = previous.process_id == expected_target.process_id
        && previous.window_handle == expected_target.window_handle
        && refreshed.process_id == expected_target.process_id
        && refreshed.window_handle == expected_target.window_handle;
    let same_geometry = previous.session_id == refreshed.session_id
        && previous.source_rect == refreshed.source_rect
        && previous.width == refreshed.width
        && previous.height == refreshed.height;
    let authorization_category = action.authorization_category(Some(refreshed_accessibility_root));
    let mut refreshed_action_value = serde_json::to_value(action).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            format!("could not revalidate confirmed action identity: {error}"),
        )
    })?;
    refreshed_action_value["authorization_category"] =
        Value::String(authorization_category.clone());
    let same_action_scope = refreshed_action_value == *confirmed_action_value
        && action.safety_tier(Some(refreshed_accessibility_root))
            == HostActionSafetyTier::ActionConfirmation
        && matches!(authorization_category.as_str(), "raw_input" | "credential");
    if !same_exact_target || !same_geometry || !same_action_scope {
        return Err(cached_target_pre_dispatch_refusal(
            ComputerUseErrorCode::StaleObservation,
            "the exact target or approved action scope changed while waiting for confirmation",
        ));
    }
    Ok(refreshed.observation_id.clone())
}
