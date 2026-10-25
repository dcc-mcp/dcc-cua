use super::*;
use rstest::rstest;

fn grant() -> ProcessAudioGrant {
    ProcessAudioGrant {
        target: ProcessAudioTarget {
            process_id: 42,
            process_creation_time_100ns: 123,
            window_handle: 43,
            window_thread_id: 44,
            window_class_hash: 45,
        },
        max_duration_ms: 1000,
        max_output_bytes: 1_000_000,
    }
}

#[rstest]
#[case("microphone")]
#[case("desktop_mix")]
#[case("exclude_process_tree")]
fn caller_cannot_select_another_audio_source(#[case] mode: &str) {
    assert!(
        serde_json::from_value::<AudioRequest>(
            json!({"mode":mode,"max_duration_ms":1000,"max_output_bytes":1000})
        )
        .is_err()
    );
}

#[rstest]
#[case("process_id")]
#[case("device_id")]
#[case("output_dir")]
fn caller_cannot_add_audio_identity_device_or_path(#[case] key: &str) {
    let mut input =
        json!({"mode":"target_process_tree","max_duration_ms":1000,"max_output_bytes":1000});
    input[key] = json!(42);
    assert!(serde_json::from_value::<AudioRequest>(input).is_err());
}

#[rstest]
fn audio_binding_derives_mode_and_refuses_upgrade_or_downgrade() {
    let mut empty = json!({});
    bind(&mut empty, Some(&grant())).unwrap();
    assert_eq!(empty["request"]["recording_audio"], "target_process_tree");
    assert!(bind(&mut empty, None).is_err());
    assert!(
        bind(
            &mut json!({"request":{"recording_audio":"none"}}),
            Some(&grant())
        )
        .is_err()
    );
    let mut silent = json!({});
    bind(&mut silent, None).unwrap();
    assert_eq!(silent, json!({}));
}

#[rstest]
fn audio_scope_schema_preserves_every_closed_property() {
    assert_eq!(
        scope_schema()["properties"],
        json!({"action":{"const":"recording_audio"},"input_kind":{"const":"playback_audio"},"secret_input":{"const":false},"authorization_category":{"const":"recording_audio"},"browser_origin":{"type":"null"}})
    );
    assert_eq!(scope_schema()["additionalProperties"], false);
    let action:TrustedTaskActionScope=serde_json::from_value(json!({"action":"recording_audio","input_kind":"playback_audio","secret_input":false,"authorization_category":"recording_audio"})).unwrap();
    assert!(action.is_recording_audio());
    assert!(!action.is_pixels_input());
}

#[rstest]
fn audio_ceilings_and_target_are_derived_without_native_calls() {
    let request: AudioRequest = serde_json::from_value(
        json!({"mode":"target_process_tree","max_duration_ms":1000,"max_output_bytes":1000}),
    )
    .unwrap();
    request.validate().unwrap();
    assert_eq!(request.grant(grant().target).target, grant().target);
    let invalid: AudioRequest = serde_json::from_value(
        json!({"mode":"target_process_tree","max_duration_ms":300001,"max_output_bytes":1000}),
    )
    .unwrap();
    assert!(invalid.validate().is_err());
}
