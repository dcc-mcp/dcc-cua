//! Caller chooses only ceilings; the issuer derives native identity and source.
use super::*;
use dcc_cua_protocol::recording_audio::{
    MAX_PROCESS_AUDIO_DURATION_MS, MAX_PROCESS_AUDIO_OUTPUT_BYTES, ProcessAudioGrant,
    ProcessAudioTarget, RecordingAudioMode,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AudioRequest {
    mode: RecordingAudioMode,
    max_duration_ms: u64,
    max_output_bytes: u64,
}
impl AudioRequest {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.mode != RecordingAudioMode::TargetProcessTree
            || self.max_duration_ms == 0
            || self.max_duration_ms > MAX_PROCESS_AUDIO_DURATION_MS
            || self.max_output_bytes == 0
            || self.max_output_bytes > MAX_PROCESS_AUDIO_OUTPUT_BYTES
        {
            return Err(
                "audio mode or ceilings are outside the closed process-tree contract".into(),
            );
        }
        Ok(())
    }
    pub(super) fn grant(&self, target: ProcessAudioTarget) -> ProcessAudioGrant {
        ProcessAudioGrant {
            target,
            max_duration_ms: self.max_duration_ms,
            max_output_bytes: self.max_output_bytes,
        }
    }
}

pub(super) fn resolve(
    target: &TrustedTaskAuthorizationTarget,
) -> Result<ProcessAudioTarget, String> {
    #[cfg(windows)]
    if let TrustedTaskAuthorizationTarget::ExactWindow {
        process_id,
        window_handle,
    } = target
    {
        return dcc_cua_platform_windows::process_audio::observe_process_audio_target(
            *process_id,
            *window_handle,
        )
        .map_err(|_| "audio target instance metadata is unavailable; no capture attempted".into());
    }
    let _ = target;
    Err("process-tree audio requires an exact Windows window".into())
}

pub(super) fn scope_schema() -> Value {
    json!({
        "title":"Target process-tree playback audio", "type":"object","additionalProperties":false,
        "required":["action","input_kind","secret_input","authorization_category"],
        "properties":{"action":{"const":"recording_audio"},"input_kind":{"const":"playback_audio"},"secret_input":{"const":false},"authorization_category":{"const":"recording_audio"},"browser_origin":{"type":"null"}}
    })
}
pub(super) fn request_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,"required":["mode","max_duration_ms","max_output_bytes"],
        "properties":{"mode":{"const":"target_process_tree"},"max_duration_ms":{"type":"integer","minimum":1,"maximum":MAX_PROCESS_AUDIO_DURATION_MS},"max_output_bytes":{"type":"integer","minimum":1,"maximum":MAX_PROCESS_AUDIO_OUTPUT_BYTES}}
    })
}

pub(super) fn configure_schema(tool: &mut Value) {
    tool["inputSchema"]["properties"]["recording_audio"] = request_schema();
    tool["inputSchema"]["allOf"].as_array_mut().expect("task conditions").push(json!({
        "if":{"anyOf":[{"required":["recording_audio"]},{"required":["allowed_actions"],"properties":{"allowed_actions":{"contains":scope_schema()}}}]},
        "then":{"required":["recording_audio","allow_recording","observation_mode","target_process_id","target_window_handle"],
            "properties":{"allow_recording":{"const":true},"observation_mode":{"const":"pixels_only"},"surface":{"const":"window"},"allowed_actions":{"contains":scope_schema()}}}
    }));
}

pub(super) fn bind(params: &mut Value, grant: Option<&ProcessAudioGrant>) -> Result<(), String> {
    let expected = if grant.is_some() {
        RecordingAudioMode::TargetProcessTree
    } else {
        RecordingAudioMode::None
    };
    if let Some(value) = params.get("request").and_then(|r| r.get("recording_audio")) {
        let requested: RecordingAudioMode = serde_json::from_value(value.clone())
            .map_err(|_| "unsupported recording audio mode")?;
        if requested != expected {
            return Err("audio mode cannot override the immutable independent task grant".into());
        }
    }
    if grant.is_some() {
        params["request"]["recording_audio"] = json!(expected);
    }
    Ok(())
}

pub(super) fn bind_recording_request(
    params: &mut Value,
    directory: Option<&str>,
) -> Result<(), String> {
    let directory =
        directory.ok_or_else(|| "native recording output was not authorized".to_owned())?;
    validate_task_method_params("recording_start", params)?;
    if params
        .get("request")
        .and_then(|request| request.get("output_dir"))
        .is_some_and(|value| value.as_str() != Some(directory))
    {
        return Err(
            "recording output directory does not match the immutable task authorization".into(),
        );
    }
    let audio = params
        .get("request")
        .and_then(|r| r.get("recording_audio"))
        .cloned();
    params["request"] = json!({"output_dir":directory,"record_video":true});
    if let Some(audio) = audio {
        params["request"]["recording_audio"] = audio;
    }
    Ok(())
}

/// Only the unique empty directory just created by this preparation may be
/// removed. Never recursively delete contents if ownership/state has changed.
pub(super) fn with_owned_directory_cleanup_on_error<T>(
    directory: Option<&str>,
    register: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    register().map_err(|mut error| {
        if let Some(directory) = directory
            && let Err(cleanup) = std::fs::remove_dir(directory)
        {
            error.push_str(&format!(
                "; empty owned recording directory cleanup failed: {cleanup}"
            ));
        }
        error
    })
}

#[cfg(test)]
mod tests;
