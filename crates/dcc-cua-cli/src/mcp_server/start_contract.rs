//! Pure task contract checks shared by attachment and owned workflow admission.
use super::*;

pub(super) fn validate_task_start(
    input: &PrepareTaskInput,
    host_derived_target: bool,
) -> Result<Vec<String>, String> {
    if input.application_label.is_empty()
        || input.application_label != input.application_label.trim()
        || input.application_label.chars().count() > dcc_cua_host::MAX_APPLICATION_LABEL_CHARS
        || input.application_label.chars().any(char::is_control)
    {
        return Err(
            "application_label must be bounded printable text without surrounding whitespace"
                .into(),
        );
    }
    let actions = input
        .allowed_actions
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    if actions.len() != input.allowed_actions.len() || actions.len() > 32 {
        return Err("allowed actions must be unique and bounded".into());
    }
    if !(1..=MAX_TTL_MINUTES).contains(&input.ttl_minutes) {
        return Err(format!(
            "ttl_minutes must be between 1 and {MAX_TTL_MINUTES}"
        ));
    }
    validate_allowed_methods(input.surface, &input.allowed_methods)?;
    validate_native_lifecycle_methods(&input.allowed_methods)?;
    capture_popup::validate_start(input)?;
    let preparation_requested = input
        .allowed_methods
        .iter()
        .any(|method| method.starts_with("capture_preparation_"));
    let preparation_action = input
        .allowed_actions
        .iter()
        .any(TrustedTaskActionScope::is_capture_preparation);
    if input.allow_capture_preparation != preparation_requested
        || input.allow_capture_preparation != preparation_action
        || (input.allow_capture_preparation
            && (input.surface != TaskSurface::Window
                || input.observation_mode != TaskObservationMode::PixelsOnly
                || ![
                    "get_window_state",
                    "capture_preparation_begin",
                    "capture_preparation_state",
                    "capture_preparation_stop",
                ]
                .iter()
                .all(|required| {
                    input
                        .allowed_methods
                        .iter()
                        .any(|method| method == required)
                })))
    {
        return Err("capture preparation requires explicit allow_capture_preparation=true, pixels_only exact window, get_window_state, complete begin/state/stop and its closed action scope".into());
    }
    let requested_recording = input
        .allowed_methods
        .iter()
        .any(|method| method.starts_with("recording_"));
    if input.allow_recording != requested_recording
        || (input.allow_recording
            && (input.observation_mode != TaskObservationMode::PixelsOnly
                || input.surface != TaskSurface::Window
                || !["recording_start", "recording_state", "recording_stop"]
                    .iter()
                    .all(|required| {
                        input
                            .allowed_methods
                            .iter()
                            .any(|method| method == required)
                    })))
    {
        return Err(
            "recording methods require explicit allow_recording=true on a pixels_only window task"
                .into(),
        );
    }
    let audio_action = input
        .allowed_actions
        .iter()
        .any(TrustedTaskActionScope::is_recording_audio);
    if input.recording_audio.is_some() != audio_action
        || (audio_action
            && (!input.allow_recording
                || input.observation_mode != TaskObservationMode::PixelsOnly
                || input.surface != TaskSurface::Window))
    {
        return Err("audio requires an independent playback_audio action on a pixels_only recording window task".into());
    }
    if let Some(audio) = &input.recording_audio {
        audio.validate()?;
    }
    if input.observation_mode == TaskObservationMode::PixelsOnly {
        if input.surface != TaskSurface::Window
            || (!host_derived_target && !matches!(input.target_process_id, Some(pid) if pid != 0))
            || (!host_derived_target
                && !matches!(input.target_window_handle, Some(hwnd) if hwnd != 0))
            || input.owned_browser_launch.is_some()
            || !input.allowed_browser_origins.is_empty()
        {
            return Err("pixels_only requires the window surface with an exact PID/HWND".into());
        }
        if input
            .allowed_methods
            .iter()
            .any(|method| !input.observation_mode.permits_method(method))
            || input.allowed_actions.iter().any(|action| {
                !action.is_window_minimize()
                    && !action.is_window_frame()
                    && !action.is_native_menu_dismiss()
                    && !action.is_capture_preparation()
                    && !action.is_recording_audio()
                    && !action.is_pixels_input()
            })
        {
            return Err(
                "pixels_only permits only closed native window_state or covered raw_input scopes, without semantic/browser methods or secrets"
                    .into(),
            );
        }
        if input
            .allowed_methods
            .iter()
            .any(|method| method == "execute_action")
            && !input
                .allowed_actions
                .iter()
                .any(TrustedTaskActionScope::is_pixels_input)
        {
            return Err(
                "pixels_only execute_action requires an actual supported raw_input action scope"
                    .into(),
            );
        }
    }
    if input
        .allowed_actions
        .iter()
        .any(TrustedTaskActionScope::is_native_menu_dismiss)
        && (input.observation_mode != TaskObservationMode::PixelsOnly
            || input.surface != TaskSurface::Window
            || !["get_window_state", "change_window_state"]
                .iter()
                .all(|required| {
                    input
                        .allowed_methods
                        .iter()
                        .any(|method| method == required)
                }))
    {
        return Err("dismiss_menu requires explicit pixels_only exact window, get_window_state, change_window_state and its independent window_state action scope".into());
    }
    if input
        .allowed_methods
        .iter()
        .any(|method| method == "set_window_frame")
        && (input.observation_mode != TaskObservationMode::PixelsOnly
            || !input
                .allowed_actions
                .iter()
                .any(TrustedTaskActionScope::is_window_frame)
            || !input
                .allowed_methods
                .iter()
                .any(|method| method == "get_window_state"))
    {
        return Err("set_window_frame requires explicit pixels_only, get_window_state, and its closed window_state action scope".into());
    }
    if input
        .allowed_methods
        .iter()
        .any(|method| method == "minimize_window")
        && !input
            .allowed_actions
            .iter()
            .any(TrustedTaskActionScope::is_window_minimize)
    {
        return Err(
            "minimize_window requires the closed window_state/minimize_window action scope".into(),
        );
    }
    let allowed_browser_origins = input
        .allowed_browser_origins
        .iter()
        .cloned()
        .chain(
            input
                .allowed_actions
                .iter()
                .filter_map(|action| action.browser_origin.clone()),
        )
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(allowed_browser_origins)
}

impl TaskAuthorizationServer {
    pub(super) fn ensure_task_capacity(&mut self) -> Result<(), String> {
        let now = unix_time_millis();
        self.proposals.retain(|_, proposal| {
            proposal.session.is_some()
                || proposal
                    .host_task
                    .as_ref()
                    .is_some_and(OwnedHostTask::is_pending)
                || proposal.registration.expires_at_unix_ms > now
        });
        if self.proposals.len() >= MAX_PENDING_TASKS {
            return Err("too many live tasks".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case::bounded_audio(true, true, 1000, true)]
    #[case::missing_action(false, true, 1000, false)]
    #[case::missing_audio(true, false, 1000, false)]
    #[case::zero_duration(true, true, 0, false)]
    #[case::excess_duration(true, true, 300_001, false)]
    #[case::video_only(false, false, 1000, true)]
    fn shared_recording_contract_preserves_independent_audio_scope(
        #[case] grant_action: bool,
        #[case] request_audio: bool,
        #[case] duration: u64,
        #[case] accepted: bool,
    ) {
        let mut request = json!({
            "application_label": "pure recording contract fixture",
            "surface": "window",
            "observation_mode": "pixels_only",
            "target_process_id": 42,
            "target_window_handle": 43,
            "ttl_minutes": 1,
            "allow_recording": true,
            "allowed_methods": ["recording_start", "recording_state", "recording_stop"],
            "allowed_actions": []
        });
        if grant_action {
            request["allowed_actions"] = json!([{
                "action": "recording_audio",
                "input_kind": "playback_audio",
                "secret_input": false,
                "authorization_category": "recording_audio"
            }]);
        }
        if request_audio {
            request["recording_audio"] = json!({
                "mode": "target_process_tree",
                "max_duration_ms": duration,
                "max_output_bytes": 1000
            });
        }
        let input = serde_json::from_value(request).unwrap();
        assert_eq!(validate_task_start(&input, false).is_ok(), accepted);
    }
}
