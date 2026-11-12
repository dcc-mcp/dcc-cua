use super::*;
use rstest::rstest;

#[rstest]
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
