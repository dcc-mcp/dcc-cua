use rstest::rstest;

use super::*;

#[rstest]
fn browser_urls_are_scheme_and_length_bounded() {
    assert!(validate_url("https://example.com").is_ok());
    assert!(validate_url("file:///secret").is_err());
    assert!(validate_url(&format!("https://{}", "a".repeat(MAX_URL_CHARS))).is_err());
}

#[rstest]
fn browser_routes_keep_dom_event_explicit() {
    assert_eq!(validate_route(None).unwrap(), "trusted");
    assert_eq!(validate_route(Some("dom_event")).unwrap(), "dom_event");
    assert!(validate_route(Some("fallback")).is_err());
}

#[rstest]
fn browser_snapshot_id_supports_both_cua_formats() {
    assert_eq!(
        browser_snapshot_id(&json!({"structuredContent":{"snapshot_id":"p1"}})),
        Some("p1".into())
    );
    assert_eq!(
        browser_snapshot_id(&json!({"structuredContent":{"snapshot":{"id":"p2"}}})),
        Some("p2".into())
    );
}

#[rstest]
fn geometry_changes_invalidate_only_the_browser_snapshot() {
    let mut session = BrowserSession {
        target_id: Some("target-1".into()),
        mutation_allowed: true,
        latest_snapshot_id: Some("snapshot-1".into()),
        latest_tab_id: Some("tab-1".into()),
    };
    session.invalidate_snapshot();
    assert_eq!(session.target_id.as_deref(), Some("target-1"));
    assert!(session.mutation_allowed);
    assert!(session.latest_snapshot_id.is_none());
    assert!(session.latest_tab_id.is_none());
}

#[rstest]
fn native_attempt_preclear_preserves_binding_and_fences_snapshot() {
    let mut session = BrowserSession {
        target_id: Some("target-1".into()),
        mutation_allowed: true,
        latest_snapshot_id: Some("snapshot-1".into()),
        latest_tab_id: Some("tab-1".into()),
    };

    session.begin_snapshot_sensitive_native_attempt();

    assert_eq!(session.target_id.as_deref(), Some("target-1"));
    assert!(session.mutation_allowed);
    assert!(session.latest_snapshot_id.is_none());
    assert!(session.latest_tab_id.is_none());
}

#[rstest]
fn browser_file_paths_require_direct_files_and_direct_directory() {
    assert!(validate_upload_paths(&[]).is_err());
    assert!(validate_upload_paths(&["relative.txt".into()]).is_err());
    assert!(validate_destination_root("relative").is_err());
    assert!(
        validate_dialog_request(&BrowserDialogRequest {
            target_id: "target".into(),
            tab_id: "tab".into(),
            action: "accept".into(),
            dialog_id: None,
            prompt_text: None,
            delivery_mode: None,
        })
        .is_err()
    );
}

#[rstest]
fn browser_result_extracts_every_image_from_json() {
    let result = BrowserResult::from_value(json!({
        "content": [
            {"type":"image", "mimeType":"image/png", "data":"aGVsbG8="},
            {"type":"image", "mimeType":"image/jpeg", "data":"d29ybGQ="}
        ]
    }))
    .unwrap();

    assert_eq!(result.images.len(), 2);
    assert_eq!(result.images[0].data, b"hello");
    assert_eq!(result.images[1].mime_type, "image/jpeg");
    assert_eq!(result.value["content"][0]["data"], Value::Null);
    assert_eq!(result.value["content"][1]["data"], Value::Null);
}
