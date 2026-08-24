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
fn browser_snapshot_origin_is_exact_and_tab_bound() {
    let snapshot = json!({
        "structuredContent": {
            "tabs": [
                {"tab_id": "tab-1", "url": "https://chromewebstore.google.com/detail/item?draft=1"},
                {"tab_id": "tab-2", "url": "https://example.test/other"}
            ]
        }
    });
    assert_eq!(
        browser_snapshot_origin(&snapshot, "tab-1").as_deref(),
        Some("https://chromewebstore.google.com")
    );
    assert_eq!(
        browser_snapshot_origin(&snapshot, "tab-2").as_deref(),
        Some("https://example.test")
    );
    assert!(browser_snapshot_origin(&snapshot, "missing").is_none());
    assert!(
        browser_snapshot_origin(
            &json!({"structuredContent": {"origin": "https://example.test/path"}}),
            "tab-1"
        )
        .is_none()
    );
}

#[rstest]
fn browser_snapshot_origin_supports_semantic_page_url() {
    let snapshot = json!({
        "structuredContent": {
            "target_id": "target-1",
            "tab_id": "tab-1",
            "page": {
                "url": "https://addons.mozilla.org/en-US/developers/"
            },
            "snapshot": {
                "id": "p1",
                "format": "semantic_v2"
            }
        }
    });

    assert_eq!(
        browser_snapshot_origin(&snapshot, "tab-1").as_deref(),
        Some("https://addons.mozilla.org")
    );
    assert!(browser_snapshot_origin(&snapshot, "tab-2").is_none());
    for invalid_url in ["file:///private/data", "not a URL"] {
        let mut invalid = snapshot.clone();
        invalid["structuredContent"]["page"]["url"] = json!(invalid_url);
        assert!(browser_snapshot_origin(&invalid, "tab-1").is_none());
    }
}

#[rstest]
fn ancestor_scope_request_requires_the_strict_semantic_contract() {
    let valid: BrowserSnapshotRequest = serde_json::from_value(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_format": "semantic_v2",
        "scope_ref": "p1:7",
        "scope_ancestor_role": "RoW",
        "query": "View release options"
    }))
    .unwrap();
    assert!(validate_ancestor_scope_request(&valid).is_ok());
    let args = browser_snapshot_args(&valid).unwrap();
    assert_eq!(args["scope_ref"], "p1:7");
    assert_eq!(args["scope_ancestor_role"], "RoW");
    assert_eq!(args["query"], "View release options");

    let legacy: BrowserSnapshotRequest = serde_json::from_value(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_format": "semantic_v2"
    }))
    .unwrap();
    assert!(legacy.scope_ancestor_role.is_none());
    assert!(validate_ancestor_scope_request(&legacy).is_ok());

    for invalid_value in [Value::Null, json!(7), json!(false)] {
        assert!(
            serde_json::from_value::<BrowserSnapshotRequest>(json!({
                "target_id": "target-1",
                "tab_id": "tab-1",
                "snapshot_format": "semantic_v2",
                "scope_ref": "p1:7",
                "scope_ancestor_role": invalid_value,
                "query": "View release options"
            }))
            .is_err()
        );
    }

    for invalid_request in [
        json!({
            "snapshot_format": "dom_refs_v1",
            "scope_ref": "p1:7",
            "scope_ancestor_role": "row",
            "query": "View release options"
        }),
        json!({
            "snapshot_format": "semantic_v2",
            "scope_ref": "p1:7",
            "scope_ancestor_role": "row",
            "query": "View release options"
        }),
        json!({
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot_format": "semantic_v2",
            "scope_ancestor_role": "row",
            "query": "View release options"
        }),
        json!({
            "snapshot_format": "semantic_v2",
            "scope_ref": "p1:7",
            "scope_ancestor_role": "row",
            "query": "View release options",
            "continuation": "opaque"
        }),
        json!({
            "snapshot_format": "semantic_v2",
            "scope_ref": "p1:7",
            "scope_ancestor_role": " row",
            "query": "View release options"
        }),
    ] {
        let request: BrowserSnapshotRequest = serde_json::from_value(invalid_request).unwrap();
        assert!(validate_ancestor_scope_request(&request).is_err());
    }
}

#[rstest]
fn ancestor_scope_response_requires_exact_anchor_evidence() {
    let expected = AncestorScopeExpectation::Initial {
        requested_ref: "p1:7".to_owned(),
        role: "row".to_owned(),
        target_id: "target-1".to_owned(),
        tab_id: "tab-1".to_owned(),
    };
    let valid = json!({
        "structuredContent": {
            "status": "ok",
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot": {
                "id": "p2",
                "format": "semantic_v2",
                "scope": "ancestor_subtree",
                "complete": true,
                "scope_anchor": {
                    "requested_ref": "p1:7",
                    "role": "row",
                    "name": "Release 0.19.89",
                    "frame": "main",
                    "distance": 1
                }
            }
        }
    });
    assert!(
        validate_ancestor_scope_response(&valid, Some(&expected))
            .unwrap()
            .is_some()
    );

    for path in [
        "target_id",
        "tab_id",
        "format",
        "snapshot_id",
        "scope",
        "requested_ref",
        "role",
        "frame",
        "distance",
    ] {
        let mut invalid = valid.clone();
        match path {
            "target_id" => invalid["structuredContent"]["target_id"] = json!("target-2"),
            "tab_id" => invalid["structuredContent"]["tab_id"] = json!("tab-2"),
            "format" => invalid["structuredContent"]["snapshot"]["format"] = json!("dom_refs_v1"),
            "snapshot_id" => invalid["structuredContent"]["snapshot_id"] = json!("p9"),
            "scope" => invalid["structuredContent"]["snapshot"]["scope"] = json!("subtree"),
            "requested_ref" => {
                invalid["structuredContent"]["snapshot"]["scope_anchor"]["requested_ref"] =
                    json!("p1:8")
            }
            "role" => {
                invalid["structuredContent"]["snapshot"]["scope_anchor"]["role"] = json!("table")
            }
            "frame" => {
                invalid["structuredContent"]["snapshot"]["scope_anchor"]["frame"] = json!("unknown")
            }
            "distance" => {
                invalid["structuredContent"]["snapshot"]["scope_anchor"]["distance"] = json!(0)
            }
            _ => unreachable!(),
        }
        let error = validate_ancestor_scope_response(&invalid, Some(&expected)).unwrap_err();
        assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    }

    let mut missing_anchor = valid;
    missing_anchor["structuredContent"]["snapshot"]["scope_anchor"] = Value::Null;
    assert!(validate_ancestor_scope_response(&missing_anchor, Some(&expected)).is_err());

    let refusal = json!({
        "structuredContent": {
            "status": "refused",
            "refusal": {
                "code": "browser_scope_unavailable",
                "message": "nearest ancestor is unavailable"
            }
        }
    });
    assert!(browser_result_is_structured_refusal(&refusal).unwrap());
    assert!(validate_ancestor_scope_response(&refusal, Some(&expected)).is_err());
}

#[rstest]
fn ancestor_scope_continuation_inherits_the_exact_anchor() {
    let anchor = json!({
        "requested_ref": "p1:7",
        "role": "row",
        "name": "Release 0.19.89",
        "frame": "main",
        "distance": 1
    });
    let pending = PendingAncestorContinuation {
        token: "bc-1".into(),
        target_id: "target-1".into(),
        tab_id: "tab-1".into(),
        snapshot_id: "p2".into(),
        anchor: anchor.clone(),
    };
    let session = BrowserSession {
        pending_ancestor_continuation: Some(pending.clone()),
        ..BrowserSession::default()
    };
    let request: BrowserSnapshotRequest = serde_json::from_value(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_format": "semantic_v2",
        "continuation": "bc-1"
    }))
    .unwrap();
    let expectation = session
        .ancestor_scope_expectation(&request)
        .unwrap()
        .expect("tracked ancestor continuation");
    let continued = json!({
        "structuredContent": {
            "status": "ok",
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot": {
                "id": "p2",
                "format": "semantic_v2",
                "scope": "continuation",
                "complete": false,
                "continuation": "bc-2",
                "scope_anchor": anchor
            }
        }
    });
    let validated = validate_ancestor_scope_response(&continued, Some(&expectation))
        .unwrap()
        .expect("validated ancestor continuation");
    assert_eq!(validated.snapshot_id, "p2");
    assert_eq!(validated.continuation.as_deref(), Some("bc-2"));
    assert_eq!(validated.anchor, pending.anchor);

    let mut changed_anchor = continued.clone();
    changed_anchor["structuredContent"]["snapshot"]["scope_anchor"]["name"] =
        json!("Release 0.19.88");
    assert!(validate_ancestor_scope_response(&changed_anchor, Some(&expectation)).is_err());

    let mut reused_token = continued.clone();
    reused_token["structuredContent"]["snapshot"]["continuation"] = json!("bc-1");
    assert!(validate_ancestor_scope_response(&reused_token, Some(&expectation)).is_err());

    let wrong_target: BrowserSnapshotRequest = serde_json::from_value(json!({
        "target_id": "target-2",
        "tab_id": "tab-1",
        "snapshot_format": "semantic_v2",
        "continuation": "bc-1"
    }))
    .unwrap();
    let mismatch = session
        .ancestor_scope_expectation(&wrong_target)
        .unwrap_err();
    assert_eq!(mismatch.code, ComputerUseErrorCode::StaleObservation);

    for invalid in [
        json!({
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot_format": "dom_refs_v1",
            "continuation": "bc-1"
        }),
        json!({
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot_format": "semantic_v2",
            "scope_ref": "p1:7",
            "continuation": "bc-1"
        }),
        json!({
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot_format": "semantic_v2",
            "query": "View release options",
            "continuation": "bc-1"
        }),
    ] {
        let invalid: BrowserSnapshotRequest = serde_json::from_value(invalid).unwrap();
        assert!(session.ancestor_scope_expectation(&invalid).is_err());
    }

    let legacy_direct_scope = json!({
        "structuredContent": {
            "status": "ok",
            "snapshot": {
                "id": "p3",
                "scope": "subtree",
                "scope_anchor": {
                    "requested_ref": "p2:9",
                    "role": "list",
                    "name": "Releases",
                    "frame": "main",
                    "distance": 0
                }
            }
        }
    });
    assert!(
        validate_ancestor_scope_response(&legacy_direct_scope, None)
            .unwrap()
            .is_none()
    );

    let untracked_session = BrowserSession::default();
    let untracked = untracked_session
        .ancestor_scope_expectation(&request)
        .unwrap()
        .expect("tab continuation without local ancestor state");
    let stale = validate_ancestor_scope_response(&continued, Some(&untracked)).unwrap_err();
    assert_eq!(stale.code, ComputerUseErrorCode::StaleObservation);

    let legacy_continued_scope = json!({
        "structuredContent": {
            "status": "ok",
            "target_id": "target-1",
            "tab_id": "tab-1",
            "snapshot": {
                "id": "p3",
                "format": "semantic_v2",
                "scope": "continuation",
                "scope_anchor": {
                    "requested_ref": "p2:9",
                    "role": "list",
                    "name": "Releases",
                    "frame": "main",
                    "distance": 0
                }
            }
        }
    });
    assert!(
        validate_ancestor_scope_response(&legacy_continued_scope, Some(&untracked))
            .unwrap()
            .is_none()
    );

    let mut explicit_null_anchor = legacy_continued_scope.clone();
    explicit_null_anchor["structuredContent"]["snapshot"]["scope_anchor"] = Value::Null;
    assert!(
        validate_ancestor_scope_response(&explicit_null_anchor, Some(&untracked))
            .unwrap()
            .is_none()
    );

    let mut missing_anchor = legacy_continued_scope.clone();
    missing_anchor["structuredContent"]["snapshot"]
        .as_object_mut()
        .unwrap()
        .remove("scope_anchor");
    assert!(validate_ancestor_scope_response(&missing_anchor, Some(&untracked)).is_err());

    let mut wrong_scope = legacy_continued_scope;
    wrong_scope["structuredContent"]["snapshot"]["scope"] = json!("ancestor_subtree");
    assert!(validate_ancestor_scope_response(&wrong_scope, Some(&untracked)).is_err());
}

#[rstest]
fn geometry_changes_invalidate_only_the_browser_snapshot() {
    let mut session = BrowserSession {
        target_id: Some("target-1".into()),
        mutation_allowed: true,
        latest_snapshot_id: Some("snapshot-1".into()),
        latest_tab_id: Some("tab-1".into()),
        latest_origin: Some("https://example.test".into()),
        pending_ancestor_continuation: Some(PendingAncestorContinuation {
            token: "bc-1".into(),
            target_id: "target-1".into(),
            tab_id: "tab-1".into(),
            snapshot_id: "snapshot-1".into(),
            anchor: json!({"requested_ref": "p0:1"}),
        }),
    };
    session.invalidate_snapshot();
    assert_eq!(session.target_id.as_deref(), Some("target-1"));
    assert!(session.mutation_allowed);
    assert!(session.latest_snapshot_id.is_none());
    assert!(session.latest_tab_id.is_none());
    assert!(session.latest_origin.is_none());
    assert!(session.pending_ancestor_continuation.is_none());
}

#[rstest]
fn bind_refusal_keeps_the_existing_browser_error_contract() {
    let mut session = BrowserSession::default();
    let error = session
        .store_binding(&json!({
            "structuredContent": {
                "status": "refused",
                "refusal": {
                    "code": "browser_wrong_target_refused",
                    "message": "the exact browser target was not proven"
                }
            }
        }))
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::BrowserRefused);
}

#[rstest]
fn native_attempt_preclear_preserves_binding_and_fences_snapshot() {
    let mut session = BrowserSession {
        target_id: Some("target-1".into()),
        mutation_allowed: true,
        latest_snapshot_id: Some("snapshot-1".into()),
        latest_tab_id: Some("tab-1".into()),
        latest_origin: Some("https://example.test".into()),
        pending_ancestor_continuation: None,
    };

    session.begin_snapshot_sensitive_native_attempt();

    assert_eq!(session.target_id.as_deref(), Some("target-1"));
    assert!(session.mutation_allowed);
    assert!(session.latest_snapshot_id.is_none());
    assert!(session.latest_tab_id.is_none());
    assert!(session.latest_origin.is_none());
}

#[rstest]
fn dialog_inspection_preclear_rejects_the_old_snapshot_for_an_immediate_click() {
    let mut session = BrowserSession {
        target_id: Some("target-1".into()),
        mutation_allowed: true,
        latest_snapshot_id: Some("p1".into()),
        latest_tab_id: Some("tab-1".into()),
        latest_origin: Some("https://example.test".into()),
        pending_ancestor_continuation: None,
    };

    session.begin_dialog_attempt();

    let error = session
        .require_mutation_target("target-1", "tab-1", "p1")
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
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

#[rstest]
fn browser_result_publishes_only_a_successful_snapshot() {
    let snapshot = BrowserResult::from_value(json!({
        "structuredContent": {
            "status": "ok",
            "mode": "snapshot",
            "snapshot": {"id": "p1"}
        }
    }))
    .unwrap();
    assert!(snapshot.publishes_snapshot_evidence());

    let refusal = BrowserResult::from_value(json!({
        "structuredContent": {
            "status": "refused",
            "refusal": {"code": "browser_scope_unavailable"}
        }
    }))
    .unwrap();
    assert!(!refusal.publishes_snapshot_evidence());
}

#[rstest]
fn browser_type_requires_exactly_one_text_source() {
    let both = serde_json::from_value::<BrowserTypeRequest>(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_id": "p1",
        "ref": "p1:2",
        "text": "plaintext",
        "secret_handle": "firefox.amo-secret"
    }))
    .unwrap();
    assert!(both.resolve(None).is_err());

    let neither = serde_json::from_value::<BrowserTypeRequest>(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_id": "p1",
        "ref": "p1:2"
    }))
    .unwrap();
    assert!(neither.resolve(None).is_err());
}

#[rstest]
fn browser_type_secret_handle_is_resolved_outside_the_browser_contract() {
    let request = serde_json::from_value::<BrowserTypeRequest>(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_id": "p1",
        "ref": "p1:2",
        "secret_handle": "firefox.amo-secret",
        "replace": true
    }))
    .unwrap();
    assert_eq!(request.secret_handle(), Some("firefox.amo-secret"));
    assert!(request.clone().resolve(None).is_err());

    let resolved = request.resolve(Some("resolved-only-at-dispatch")).unwrap();
    assert_eq!(resolved.text, "resolved-only-at-dispatch");
    assert!(!format!("{resolved:?}").contains("resolved-only-at-dispatch"));
}

#[rstest]
fn browser_type_validation_rejects_unbound_evidence_before_secret_resolution() {
    let request = serde_json::from_value::<BrowserTypeRequest>(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "snapshot_id": "p1",
        "ref": "p1:2",
        "secret_handle": "firefox.amo-secret"
    }))
    .unwrap();
    assert!(
        BrowserSession::default()
            .validate_type_request(&request)
            .is_err()
    );
}
