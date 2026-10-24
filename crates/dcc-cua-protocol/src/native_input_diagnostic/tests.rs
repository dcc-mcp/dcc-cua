use super::*;
use rstest::rstest;

#[rstest]
#[case("authorizes_input", serde_json::json!(true))]
#[case("window_title", serde_json::json!("untrusted content"))]
fn diagnostic_rejects_authority_and_unbounded_content(
    #[case] key: &str,
    #[case] value: serde_json::Value,
) {
    let mut sample = serde_json::json!({
        "schema_version":1,"authorizes_input":false,"target_process_id":42,
        "target_window_handle":99,"point":null,"unobscured":false,
        "point_proof_complete":false,"point_visible":null,"hit_window_handle":null,
        "hit_process_id":null,"hit_root_window_handle":null,"hit_is_target_or_child":null,
        "redacted_rectangles":[],"redaction_count":0,"redactions_complete":false
    });
    assert!(serde_json::from_value::<NativeInputFenceDiagnostic>(sample.clone()).is_ok());
    sample[key] = value;
    assert!(serde_json::from_value::<NativeInputFenceDiagnostic>(sample).is_err());
}
