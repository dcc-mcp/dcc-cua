use serde_json::{Map, Value, json};

use super::ComputerUseDriver;
use crate::contracts::{
    ComputerUseError, ComputerUseErrorCode, ComputerUseLaunchRequest, ComputerUseResult,
    MAX_LAUNCH_ARGUMENTS, MAX_LAUNCH_URLS,
};

impl ComputerUseDriver {
    /// Launch one explicitly selected application through CUA's platform backend.
    pub async fn launch_app(&self, request: &ComputerUseLaunchRequest) -> ComputerUseResult<Value> {
        self.call_tool_value("launch_app", launch_arguments(request, None)?)
            .await
    }

    /// Launch an application under the CUA runtime session that will own its lifecycle.
    pub async fn launch_app_for_session(
        &self,
        request: &ComputerUseLaunchRequest,
        session_id: &str,
    ) -> ComputerUseResult<Value> {
        if session_id.trim().is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "launch session_id must not be empty",
            ));
        }
        let arguments = launch_arguments(request, Some(session_id))?;
        self.call_tool_value(
            "start_session",
            json!({"session": session_id, "capture_scope": "window"}),
        )
        .await?;
        match self.call_tool_value("launch_app", arguments).await {
            Ok(result) => Ok(result),
            Err(error) => {
                let _ = self.end_launch_session(session_id).await;
                Err(error)
            }
        }
    }

    /// End a launch-only runtime session that was not promoted to a window session.
    pub async fn end_launch_session(&self, session_id: &str) -> ComputerUseResult<Value> {
        if session_id.trim().is_empty() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "launch session_id must not be empty",
            ));
        }
        self.call_tool_value("end_session", json!({"session": session_id}))
            .await
    }
}

pub(crate) fn launch_arguments(
    request: &ComputerUseLaunchRequest,
    session_id: Option<&str>,
) -> ComputerUseResult<Value> {
    validate_launch_request(request)?;
    let mut arguments = serde_json::to_value(request)
        .map_err(|error| {
            ComputerUseError::new(ComputerUseErrorCode::InvalidAction, error.to_string())
        })?
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if let Some(session_id) = session_id {
        arguments.insert("session".into(), json!(session_id));
    }
    Ok(Value::Object(arguments))
}

pub(crate) fn validate_launch_request(request: &ComputerUseLaunchRequest) -> ComputerUseResult<()> {
    let selectors = [
        request.name.as_deref(),
        request.bundle_id.as_deref(),
        request.aumid.as_deref(),
        request.path.as_deref(),
        request.launch_path.as_deref(),
    ];
    let selector_count = selectors
        .iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .count();
    if selector_count > 1 || (selector_count == 0 && request.urls.is_empty()) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "launch requires one application selector or at least one URL",
        ));
    }
    let selected = selectors
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if crate::policy::DENIED_SURFACE_MARKERS
        .iter()
        .chain(&["bash"])
        .any(|marker| selected.contains(marker))
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidTarget,
            "system, terminal, authentication, password, and security applications are not allowed",
        ));
    }
    if request.urls.len() > MAX_LAUNCH_URLS
        || request.additional_arguments.len() > MAX_LAUNCH_ARGUMENTS
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "launch contains too many URLs or arguments",
        ));
    }
    if request.urls.iter().any(|url| {
        let url = url.trim().to_ascii_lowercase();
        url.len() > 4096
            || !matches!(
                url.as_str(),
                value if value.starts_with("https://")
                    || value.starts_with("http://")
                    || value.starts_with("com.epicgames.launcher://")
            )
    }) {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "launch URLs must use http, https, or the Epic Games Launcher protocol",
        ));
    }
    Ok(())
}
