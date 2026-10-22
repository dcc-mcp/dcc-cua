use rstest::rstest;

use super::*;

mod prepare_recovery;

#[rstest]
fn browser_urls_are_scheme_and_length_bounded() {
    assert!(validate_url("https://example.com").is_ok());
    assert!(validate_url("file:///secret").is_err());
    assert!(validate_url(&format!("https://{}", "a".repeat(MAX_URL_CHARS))).is_err());
}

#[rstest]
fn browser_navigation_requires_an_explicit_bounded_delivery_mode() {
    let foreground: BrowserNavigateRequest = serde_json::from_value(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "url": "https://example.test/next",
        "delivery_mode": "foreground"
    }))
    .unwrap();
    assert_eq!(foreground.delivery_mode.as_deref(), Some("foreground"));
    assert!(validate_navigate_request(&foreground).is_ok());

    let background: BrowserNavigateRequest = serde_json::from_value(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "url": "https://example.test/next",
        "delivery_mode": "background"
    }))
    .unwrap();
    assert!(validate_navigate_request(&background).is_ok());

    let invalid: BrowserNavigateRequest = serde_json::from_value(json!({
        "target_id": "target-1",
        "tab_id": "tab-1",
        "url": "https://example.test/next",
        "delivery_mode": "auto"
    }))
    .unwrap();
    assert!(validate_navigate_request(&invalid).is_err());
}

#[rstest]
fn foreground_navigation_readback_is_exact_and_visible() {
    let verified = json!({
        "structuredContent": {
            "status": "ok",
            "target_id": "target-1",
            "tab_id": "tab-1",
            "delivery_mode": "foreground",
            "activated": true,
            "current_url": "https://example.test/current",
            "title": "Current title",
            "heading": "Current heading",
            "visibility_state": "visible",
            "ready_state": "complete"
        }
    });
    assert!(validate_foreground_navigation_readback(&verified, "target-1", "tab-1").is_ok());

    for invalid in [
        json!({"structuredContent": {"status": "ok", "target_id": "target-drift", "tab_id": "tab-1", "delivery_mode": "foreground", "activated": true, "current_url": "https://example.test/current", "title": "Current title", "heading": "Current heading", "visibility_state": "visible", "ready_state": "complete"}}),
        json!({"structuredContent": {"status": "ok", "target_id": "target-1", "tab_id": "tab-drift", "delivery_mode": "foreground", "activated": true, "current_url": "https://example.test/current", "title": "Current title", "heading": "Current heading", "visibility_state": "visible", "ready_state": "complete"}}),
        json!({"structuredContent": {"status": "ok", "target_id": "target-1", "tab_id": "tab-1", "delivery_mode": "foreground", "activated": true, "current_url": "https://example.test/current", "title": "Current title", "heading": "Current heading", "visibility_state": "hidden", "ready_state": "complete"}}),
        json!({"structuredContent": {"status": "ok", "target_id": "target-1", "tab_id": "tab-1", "delivery_mode": "foreground", "activated": true, "current_url": "https://example.test/current", "title": "Current title", "heading": "Current heading", "visibility_state": "visible", "ready_state": "loading"}}),
    ] {
        assert!(validate_foreground_navigation_readback(&invalid, "target-1", "tab-1").is_err());
    }
}

#[rstest]
fn navigation_partial_failure_receipt_is_preserved_and_strictly_bound() {
    let receipt = json!({
        "structuredContent": {
            "status": "error",
            "target_id": "target-1",
            "tab_id": "tab-1",
            "url": "https://example.test/next",
            "delivery_mode": "foreground",
            "dispatched": true,
            "activated": false,
            "activation_state": "failed",
            "readback_state": "not_started",
            "refs_invalidated": true,
            "error_code": "target_activation_failed",
            "error": "foreground target activation failed"
        }
    });
    assert!(
        !navigation_response_is_success(
            &receipt,
            "target-1",
            "tab-1",
            "https://example.test/next",
            "foreground"
        )
        .unwrap()
    );

    for invalid in [
        json!({"structuredContent": {"status": "error", "target_id": "target-drift", "tab_id": "tab-1", "url": "https://example.test/next", "delivery_mode": "foreground", "dispatched": true, "activation_state": "failed", "readback_state": "not_started", "error_code": "target_activation_failed", "error": "foreground target activation failed"}}),
        json!({"structuredContent": {"status": "error", "target_id": "target-1", "tab_id": "tab-1", "url": "https://example.test/next", "delivery_mode": "foreground", "dispatched": true, "activation_state": "succeeded", "readback_state": "succeeded", "error_code": "", "error": ""}}),
        json!({"structuredContent": {"status": "error", "target_id": "target-1", "tab_id": "tab-1", "url": "https://example.test/next", "delivery_mode": "foreground", "dispatched": false, "activated": true, "activation_state": "succeeded", "readback_state": "not_started", "refs_invalidated": false, "error_code": "navigation_dispatch_failed", "error": "browser navigation dispatch failed"}}),
        json!({"structuredContent": {"status": "error", "target_id": "target-1", "tab_id": "tab-1", "url": "https://example.test/next", "delivery_mode": "foreground", "dispatched": true, "activated": false, "activation_state": "failed", "readback_state": "timeout", "refs_invalidated": true, "error_code": "target_activation_failed", "error": "foreground target activation failed"}}),
    ] {
        assert!(
            navigation_response_is_success(
                &invalid,
                "target-1",
                "tab-1",
                "https://example.test/next",
                "foreground"
            )
            .is_err()
        );
    }
}

#[rstest]
fn navigation_receipt_requires_the_exact_requested_url_for_success_and_failure() {
    for receipt in [
        json!({"structuredContent": {
            "status": "error", "target_id": "target-1", "tab_id": "tab-1",
            "url": "https://example.test/other", "delivery_mode": "foreground",
            "dispatched": true, "activated": false, "activation_state": "failed",
            "readback_state": "not_started", "refs_invalidated": true,
            "error_code": "target_activation_failed",
            "error": "foreground target activation failed"
        }}),
        json!({"structuredContent": {
            "status": "ok", "target_id": "target-1", "tab_id": "tab-1",
            "url": "https://example.test/other", "delivery_mode": "foreground",
            "dispatched": true, "activated": true, "activation_state": "succeeded",
            "readback_state": "succeeded", "refs_invalidated": true,
            "current_url": "https://example.test/redirected", "title": "Title",
            "heading": "Heading", "visibility_state": "visible", "ready_state": "complete"
        }}),
    ] {
        assert!(
            navigation_response_is_success(
                &receipt,
                "target-1",
                "tab-1",
                "https://example.test/next",
                "foreground"
            )
            .is_err()
        );
    }
}

#[rstest]
fn navigation_failure_receipt_requires_the_sdk_owned_error_contract() {
    for (error_code, error) in [
        ("invented_error", "invented diagnostic"),
        (
            "target_activation_failed",
            "C:\\sensitive\\fixture with token=opaque-test-value",
        ),
        (
            "foreground_readback_failed",
            "foreground target activation failed",
        ),
    ] {
        let receipt = json!({"structuredContent": {
            "status": "error", "target_id": "target-1", "tab_id": "tab-1",
            "url": "https://example.test/next", "delivery_mode": "foreground",
            "dispatched": true, "activated": false, "activation_state": "failed",
            "readback_state": "not_started", "refs_invalidated": true,
            "error_code": error_code, "error": error
        }});
        assert!(
            navigation_response_is_success(
                &receipt,
                "target-1",
                "tab-1",
                "https://example.test/next",
                "foreground"
            )
            .is_err(),
            "{receipt}"
        );
    }
}

#[rstest]
fn every_sdk_owned_navigation_failure_contract_is_accepted() {
    for (
        delivery_mode,
        dispatched,
        activated,
        activation_state,
        readback_state,
        error_code,
        error,
    ) in [
        (
            "foreground",
            false,
            false,
            "not_started",
            "not_started",
            "navigation_dispatch_failed",
            "browser navigation dispatch failed",
        ),
        (
            "background",
            true,
            false,
            "not_started",
            "not_started",
            "navigation_rejected",
            "browser navigation was rejected",
        ),
        (
            "foreground",
            true,
            false,
            "failed",
            "not_started",
            "target_activation_failed",
            "foreground target activation failed",
        ),
        (
            "foreground",
            true,
            true,
            "succeeded",
            "failed",
            "navigation_identity_missing",
            "browser navigation did not return a frame identity",
        ),
        (
            "foreground",
            true,
            true,
            "succeeded",
            "stale_document",
            "navigation_commit_not_observed",
            "foreground readback did not reach the dispatched navigation",
        ),
        (
            "foreground",
            true,
            true,
            "succeeded",
            "failed",
            "foreground_readback_failed",
            "foreground browser readback failed",
        ),
        (
            "foreground",
            true,
            true,
            "succeeded",
            "timeout",
            "foreground_readback_timeout",
            "foreground browser readback timed out",
        ),
        (
            "foreground",
            true,
            true,
            "succeeded",
            "succeeded",
            "target_drift_after_navigation",
            "browser target identity changed after navigation",
        ),
    ] {
        let receipt = json!({"structuredContent": {
            "status": "error", "target_id": "target-1", "tab_id": "tab-1",
            "url": "https://example.test/next", "delivery_mode": delivery_mode,
            "dispatched": dispatched, "activated": activated,
            "activation_state": activation_state, "readback_state": readback_state,
            "refs_invalidated": dispatched, "error_code": error_code, "error": error
        }});
        assert!(
            !navigation_response_is_success(
                &receipt,
                "target-1",
                "tab-1",
                "https://example.test/next",
                delivery_mode
            )
            .unwrap(),
            "{receipt}"
        );
    }
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

fn prepared_browser_result() -> Value {
    json!({
        "isError": false,
        "content": [{"type": "text", "text": "Bind the native window again."}],
        "structuredContent": {
            "status": "ok",
            "prepared": true,
            "refusal": null,
            "action": "attached_existing_profile",
            "prepared_pid": 42,
            "side_effects": {"opened_setup_page": true, "closed_setup_page": true},
            "attachment": {
                "kind": "existing_profile",
                "browser": "chromium",
                "capabilities_invalidated": true,
                "next_action": "get_browser_state"
            }
        }
    })
}

#[rstest]
fn prepared_browser_result_translates_only_the_public_continuation() {
    let value = prepared_browser_result();
    let mut expected = value.clone();
    expected["structuredContent"]["attachment"]["next_action"] = json!("browser_snapshot");
    let result = BrowserResult::from_prepare_value(value).unwrap();
    assert_eq!(result.value, expected);
    assert!(result.images.is_empty());
    assert!(!result.publishes_snapshot_evidence());
}

#[rstest]
#[case::unprepared("/structuredContent/prepared", json!(false))]
#[case::error("/isError", json!(true))]
#[case::refused_status("/structuredContent/status", json!("refused"))]
#[case::error_status("/structuredContent/status", json!("error"))]
#[case::refusal("/structuredContent/refusal", json!({"code": "browser_consent_required"}))]
#[case::unknown_attachment("/structuredContent/attachment/kind", json!("unknown"))]
#[case::unknown_action("/structuredContent/attachment/next_action", json!("manual_setup"))]
#[case::public_action("/structuredContent/attachment/next_action", json!("browser_snapshot"))]
#[case::null_action("/structuredContent/attachment/next_action", Value::Null)]
#[case::missing_action("/structuredContent/attachment", json!({"kind": "existing_profile"}))]
#[case::missing_attachment("/structuredContent/attachment", Value::Null)]
#[case::missing_structure("/structuredContent", Value::Null)]
fn prepare_guidance_preserves_nonmatching_receipts(
    #[case] pointer: &str,
    #[case] replacement: Value,
) {
    let mut value = prepared_browser_result();
    *value.pointer_mut(pointer).unwrap() = replacement;
    let result = BrowserResult::from_prepare_value(value.clone()).unwrap();
    assert_eq!(result.value, value);
    assert!(!result.publishes_snapshot_evidence());
}

#[rstest]
fn navigation_unknown_completion_is_preserved_without_claiming_dispatch() {
    for delivery_mode in ["background", "foreground"] {
        let receipt = json!({"structuredContent": {
            "status": "error", "target_id": "target-1", "tab_id": "tab-1",
            "url": "https://example.test/next", "delivery_mode": delivery_mode,
            "dispatched": null, "dispatch_state": "unknown", "activated": false,
            "activation_state": "not_started", "readback_state": "not_started",
            "refs_invalidated": true, "error_code": "navigation_completion_unknown",
            "error": "browser navigation completion is unknown; refresh browser state before retrying"
        }});
        assert!(
            !navigation_response_is_success(
                &receipt,
                "target-1",
                "tab-1",
                "https://example.test/next",
                delivery_mode
            )
            .unwrap()
        );
        let public = BrowserResult::from_value(receipt.clone())
            .unwrap()
            .into_public_navigation_receipt(false)
            .unwrap();
        assert!(public.value["structuredContent"]["dispatched"].is_null());
        assert_eq!(
            public.value["structuredContent"]["dispatch_state"],
            "unknown"
        );
        assert_eq!(public.value["structuredContent"]["refs_invalidated"], true);
        for (field, invalid_value) in [
            ("dispatched", json!(false)),
            ("dispatched", json!(true)),
            ("dispatch_state", json!("failed")),
            ("error_code", json!("navigation_dispatch_failed")),
            ("status", json!("ok")),
            ("refs_invalidated", json!(false)),
            ("activated", json!(true)),
            ("activation_state", json!("succeeded")),
            ("readback_state", json!("succeeded")),
            ("error", json!("untrusted transport message")),
        ] {
            let mut invalid = receipt.clone();
            invalid["structuredContent"][field] = invalid_value;
            assert!(
                navigation_response_is_success(
                    &invalid,
                    "target-1",
                    "tab-1",
                    "https://example.test/next",
                    delivery_mode
                )
                .is_err(),
                "{invalid}"
            );
        }
        let mut omitted = receipt.clone();
        omitted["structuredContent"]
            .as_object_mut()
            .unwrap()
            .remove("dispatched");
        assert!(
            navigation_response_is_success(
                &omitted,
                "target-1",
                "tab-1",
                "https://example.test/next",
                delivery_mode
            )
            .is_err()
        );
    }
}

// --- browser_download completion contract enrichment (GitHub #357) ---

fn canonical_temp_dir() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("create temp download root");
    let canonical = std::fs::canonicalize(dir.path())
        .expect("canonicalize temp download root")
        .to_string_lossy()
        .into_owned();
    (dir, canonical)
}

fn completed_download_value(download_id: &str, bytes: u64) -> Value {
    json!({
        "content": [{"type": "text", "text": "browser download completed"}],
        "structuredContent": {
            "status": "completed",
            "download_id": download_id,
            "bytes": bytes,
        }
    })
}

#[rstest]
#[case::completed(Some("completed"), "completed")]
#[case::accepted(Some("accepted"), "accepted")]
#[case::in_progress(Some("in_progress"), "in_progress")]
#[case::refused(Some("refused"), "failed")]
#[case::error(Some("error"), "failed")]
#[case::canceled(Some("canceled"), "failed")]
#[case::unknown(Some("something_else"), "failed")]
#[case::missing(None, "failed")]
fn download_lifecycle_state_maps_driver_status_to_stable_vocabulary(
    #[case] status: Option<&str>,
    #[case] expected: &str,
) {
    assert_eq!(download_lifecycle_state(status), expected);
}

#[rstest]
#[case::opaque_guid("3f2b1c4d-5e6f-7081", true)]
#[case::plain("opaque-guid", true)]
#[case::parent_escape("../escape", false)]
#[case::nested("a/b", false)]
#[case::absolute("/etc/passwd", false)]
#[case::dot(".", false)]
#[case::empty("", false)]
fn download_id_accepts_only_a_single_normal_component(#[case] id: &str, #[case] safe: bool) {
    assert_eq!(download_id_is_safe_component(id), safe);
}

#[rstest]
#[case::zip(b"PK\x03\x04rest", "application/zip")]
#[case::zip_empty(b"PK\x05\x06rest", "application/zip")]
#[case::gzip(b"\x1F\x8B\x08rest", "application/gzip")]
#[case::seven_zip(b"7z\xBC\xAF\x27\x1Crest", "application/x-7z-compressed")]
#[case::rar(b"Rar!\x1A\x07\x01\x00", "application/vnd.rar")]
#[case::pdf(b"%PDF-1.7 rest", "application/pdf")]
#[case::png(b"\x89PNG\r\n\x1a\nrest", "image/png")]
#[case::jpeg(b"\xFF\xD8\xFF\xE0rest", "image/jpeg")]
#[case::gif(b"GIF89arest", "image/gif")]
#[case::webp(b"RIFF\x24\x00\x00\x00WEBPVP8 ", "image/webp")]
#[case::tiff_le(b"II*\x00rest", "image/tiff")]
#[case::tiff_be(b"MM\x00*rest", "image/tiff")]
#[case::exr(b"\x76\x2f\x31\x01rest", "image/x-exr")]
#[case::gltf(b"glTF\x02\x00\x00\x00", "model/gltf-binary")]
#[case::ogg(b"OggS\x00\x02", "application/ogg")]
#[case::elf(b"\x7fELFrest", "application/x-executable")]
#[case::mp4(b"\x00\x00\x00\x18ftypisom", "video/mp4")]
#[case::quicktime(b"\x00\x00\x00\x14ftypqt  ", "video/quicktime")]
#[case::heic(b"\x00\x00\x00\x18ftypheic", "image/heic")]
#[case::avif(b"\x00\x00\x00\x18ftypavif", "image/avif")]
#[case::m4a(b"\x00\x00\x00\x18ftypM4A ", "audio/mp4")]
#[case::unknown(b"\x00\x01\x02\x03\x04", "application/octet-stream")]
#[case::too_short(b"PK", "application/octet-stream")]
fn download_mime_type_is_detected_from_a_bounded_byte_window(
    #[case] head: &[u8],
    #[case] expected: &str,
) {
    assert_eq!(sniff_download_mime_type(head), expected);
}

#[rstest]
fn tar_mime_type_is_detected_from_the_ustar_offset() {
    let mut head = vec![0u8; 512];
    head[257..262].copy_from_slice(b"ustar");
    assert_eq!(sniff_download_mime_type(&head), "application/x-tar");
}

#[rstest]
#[tokio::test]
async fn completed_download_is_enriched_with_boundary_proven_artifact_metadata() {
    let (_dir, root) = canonical_temp_dir();
    let download_id = "3f2b1c4d-5e6f-7081";
    let contents = b"PK\x03\x04fake-zip-payload-for-unreal-import";
    let file = Path::new(&root).join(download_id);
    std::fs::write(&file, contents).expect("write completed download");

    let mut value = completed_download_value(download_id, contents.len() as u64);
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    assert_eq!(structured["status"], "completed");
    assert_eq!(structured["download_id"], download_id);
    // Existing fields are preserved, not recomputed or removed.
    assert_eq!(structured["bytes"], contents.len() as u64);
    // Lifecycle state distinguishes the completion.
    assert_eq!(structured["download_state"], "completed");
    // Stored filename equals the opaque download id (extension-less download).
    assert_eq!(structured["stored_filename"], download_id);
    // file_path is the canonical location proven to be a direct child of root.
    let file_path = structured["file_path"].as_str().expect("file_path");
    assert_eq!(
        Path::new(file_path).parent(),
        Some(Path::new(&root)),
        "reported file_path must stay inside destination_root"
    );
    // artifact_ref is a portable reference to the same artifact.
    let artifact_ref = structured["artifact_ref"].as_str().expect("artifact_ref");
    assert!(
        artifact_ref.starts_with("file:"),
        "artifact_ref should be a file:// reference, got {artifact_ref}"
    );
    // sha256 matches an independent computation of the stored bytes.
    let expected_sha256 = format!("{:x}", Sha256::digest(contents));
    assert_eq!(structured["sha256"], expected_sha256);
    // mime_type is detected from content, not a filename.
    assert_eq!(structured["mime_type"], "application/zip");
}

#[rstest]
#[tokio::test]
async fn completed_download_hashes_a_multi_chunk_artifact_off_the_executor() {
    let (_dir, root) = canonical_temp_dir();
    let download_id = "multi-chunk-zip";
    // Larger than the 16 KiB streaming buffer so the digest spans many reads.
    // This runs on the current-thread test runtime (matching the MCP server
    // flavor), proving the offloaded full-file hash completes and stays correct
    // instead of stalling the executor.
    let mut contents = b"PK\x03\x04".to_vec();
    contents.resize(4 + 64 * 1024, 0x5a);
    std::fs::write(Path::new(&root).join(download_id), &contents)
        .expect("write multi-chunk download");

    let mut value = completed_download_value(download_id, contents.len() as u64);
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    assert_eq!(structured["download_state"], "completed");
    assert_eq!(
        structured["sha256"],
        format!("{:x}", Sha256::digest(&contents))
    );
    assert_eq!(structured["mime_type"], "application/zip");
    assert_eq!(structured["bytes"], contents.len() as u64);
}

#[rstest]
#[tokio::test]
async fn refused_download_reports_failed_state_without_any_artifact_path() {
    let (_dir, root) = canonical_temp_dir();
    let mut value = json!({
        "structuredContent": {
            "status": "refused",
            "refusal": {"code": "browser_action_unavailable", "message": "canceled"},
        }
    });
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    assert_eq!(structured["download_state"], "failed");
    assert!(structured.get("file_path").is_none());
    assert!(structured.get("artifact_ref").is_none());
    assert!(structured.get("stored_filename").is_none());
    assert!(structured.get("sha256").is_none());
    assert!(structured.get("mime_type").is_none());
}

#[rstest]
#[tokio::test]
async fn completed_download_with_missing_file_keeps_completed_state_but_omits_path() {
    let (_dir, root) = canonical_temp_dir();
    let mut value = completed_download_value("does-not-exist", 42);
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    // The driver completion is preserved as the source of truth for state.
    assert_eq!(structured["download_state"], "completed");
    // No artifact can be proven in-bounds, so no path or digest is reported.
    assert!(structured.get("file_path").is_none());
    assert!(structured.get("sha256").is_none());
}

#[rstest]
#[tokio::test]
async fn completed_download_never_follows_a_path_escaping_download_id() {
    let (_dir, root) = canonical_temp_dir();
    // Keep the out-of-bounds target in its own disposable directory rather than
    // the shared system temp, so the test never writes a fixed name into a
    // shared location. A traversal id is rejected by the id-component guard
    // before any filesystem access, so this secret is never resolved or exposed.
    let outside_dir = tempfile::tempdir().expect("create outside temp dir");
    let outside = outside_dir.path().join("outside-secret");
    std::fs::write(&outside, b"top-secret").expect("write outside secret");

    let mut value = completed_download_value("../outside-secret", 10);
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    assert!(structured.get("file_path").is_none());
    assert!(structured.get("sha256").is_none());
}

#[rstest]
#[tokio::test]
async fn completed_download_rejects_a_directory_target() {
    let (_dir, root) = canonical_temp_dir();
    let download_id = "a-directory";
    std::fs::create_dir(Path::new(&root).join(download_id)).expect("create directory target");

    let mut value = completed_download_value(download_id, 0);
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    assert!(structured.get("file_path").is_none());
    assert!(structured.get("sha256").is_none());
}

#[cfg(unix)]
#[rstest]
#[tokio::test]
async fn completed_download_rejects_a_symlink_target() {
    let (_dir, root) = canonical_temp_dir();
    // Point the symlink at a target in a dedicated disposable directory instead
    // of the shared system temp, so the test never writes a fixed shared name.
    let outside_dir = tempfile::tempdir().expect("create outside temp dir");
    let target = outside_dir.path().join("symlink-target");
    std::fs::write(&target, b"symlinked").expect("write symlink target");
    let download_id = "a-symlink";
    std::os::unix::fs::symlink(&target, Path::new(&root).join(download_id))
        .expect("create symlink");

    let mut value = completed_download_value(download_id, 8);
    enrich_download_completion(&root, &mut value).await;

    let structured = &value["structuredContent"];
    assert!(structured.get("file_path").is_none());
    assert!(structured.get("sha256").is_none());
}

#[rstest]
#[tokio::test]
async fn completed_download_preserves_driver_bytes_when_present() {
    let (_dir, root) = canonical_temp_dir();
    let download_id = "with-driver-bytes";
    std::fs::write(Path::new(&root).join(download_id), b"abcd").expect("write download");

    // Driver reports a byte count; enrichment must not overwrite it.
    let mut value = completed_download_value(download_id, 999);
    enrich_download_completion(&root, &mut value).await;

    assert_eq!(value["structuredContent"]["bytes"], 999);
}

#[rstest]
#[tokio::test]
async fn completed_download_fills_missing_bytes_from_the_proven_artifact() {
    let (_dir, root) = canonical_temp_dir();
    let download_id = "no-driver-bytes";
    std::fs::write(Path::new(&root).join(download_id), b"abcdefg").expect("write download");

    let mut value = json!({
        "structuredContent": {"status": "completed", "download_id": download_id}
    });
    enrich_download_completion(&root, &mut value).await;

    assert_eq!(value["structuredContent"]["bytes"], 7);
    assert_eq!(value["structuredContent"]["download_state"], "completed");
}

#[rstest]
#[tokio::test]
async fn download_completion_without_structured_content_is_left_untouched() {
    let (_dir, root) = canonical_temp_dir();
    let mut value = json!({"content": [{"type": "text", "text": "x"}]});
    let before = value.clone();
    enrich_download_completion(&root, &mut value).await;
    assert_eq!(value, before);
}
