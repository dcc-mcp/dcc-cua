use rstest::rstest;
use serde_json::json;
use sha2::Digest;

use super::actions::{
    action_from_command, action_from_json, action_result_value, bind_fresh_element_token,
    default_activated_action_to_foreground, map_visible_snapshot_coordinates, menu_request,
    require_exact_window_target, visible_snapshot_dimensions, window_frame_request,
};
use super::authorization::{existing_profile_grant_requested, host_private_worker_options};
use super::host_lifecycle::validate_host_version;
use super::*;

#[rstest]
#[case("full", true)]
#[case("visual", true)]
#[case("semantic", false)]
fn doctor_selects_the_requested_readiness_route(#[case] route: &str, #[case] expected: bool) {
    let report = json!({
        "ready": true,
        "routes": {
            "full": {"ready": false},
            "visual": {"ready": true},
            "semantic": {"ready": false}
        }
    });

    assert_eq!(diagnostic_route_ready(&report, route), expected);
}

#[rstest]
fn doctor_route_rejects_unknown_contracts() {
    assert_eq!(doctor_route(&[]).unwrap(), "full");
    assert_eq!(
        doctor_route(&["--route".into(), "visual".into()]).unwrap(),
        "visual"
    );
    assert!(doctor_route(&["--route".into(), "pixels".into()]).is_err());
}

#[rstest]
fn input_state_methods_are_counted_as_nonvisual_semantic_observations() {
    assert!(is_semantic_observation_request("get_input_state"));
    assert!(is_semantic_observation_request("poll_session_events"));
    assert!(is_semantic_observation_request("session_health"));
    assert!(!is_action_request("poll_session_events"));
    assert!(!is_action_request("session_health"));
}

#[rstest]
fn manifest_advertises_the_session_input_event_contract() {
    let manifest = manifest::document();
    let events = &manifest["host"]["session_events"];
    assert_eq!(events["state_method"], "get_input_state");
    assert_eq!(events["poll_method"], "poll_session_events");
    assert_eq!(
        events["max_poll_timeout_ms"],
        dcc_cua_host::MAX_SESSION_EVENT_POLL_TIMEOUT_MS
    );
    assert_eq!(
        events["queue_capacity"],
        dcc_cua_host::MAX_SESSION_INPUT_EVENTS
    );
    assert_eq!(events["recovery_notifies_only"], true);
    assert_eq!(events["automatic_input"], false);
    assert_eq!(
        events["state_components"],
        json!(["interactive_input", "target_window"])
    );
    assert_eq!(events["cursor_field"], "latest_sequence");
    assert_eq!(events["component_sequence"], "last_transition");
    assert_eq!(
        events["target_event_types"],
        json!([
            "target_minimized",
            "target_restored",
            "target_unavailable",
            "target_available"
        ])
    );
    assert_eq!(events["target_recovery"]["operation"], "restore_activate");
    assert_eq!(
        events["target_recovery"]["exact_grant_binding_required"],
        true
    );
    assert_eq!(events["target_recovery"]["blind_retry"], false);
}

#[rstest]
fn manifest_advertises_the_atomic_session_health_preflight_contract() {
    let manifest = manifest::document();
    let health = &manifest["host"]["session_health"];

    assert_eq!(health["method"], "session_health");
    assert_eq!(
        health["components"],
        json!([
            "interactive_input",
            "exact_target_window",
            "recording",
            "action_evidence_epoch",
            "transition_sequence"
        ])
    );
    assert_eq!(
        health["policy_defaults"]["require_recording_healthy"],
        false
    );
    assert_eq!(
        health["policy_defaults"]["require_recording_progress"],
        false
    );
    assert_eq!(health["safe_to_input_authority"], "preflight_only");
    assert_eq!(health["automatic_activation"], false);
    assert_eq!(health["automatic_input"], false);
    assert_eq!(health["fresh_observation_required"], true);
    assert_eq!(health["replaces_execute_action_gate"], false);
    assert_eq!(
        health["recording_progress_authority"]["video_present"],
        "video"
    );
    assert_eq!(
        health["state_changed_during_probe_blocker"],
        "state_changed_during_probe"
    );
    assert_eq!(
        health["recording_progress_fingerprint"],
        json!([
            "lane",
            "trajectory_turn",
            "finalized_segments",
            "current_partial_size_bytes",
            "current_partial_modified_at_unix_ms"
        ])
    );
}

#[rstest]
#[case("help", &[])]
#[case("--help", &[])]
#[case("host-jsonl", &["--help"])]
#[case("profile", &["-h"])]
fn help_requests_are_handled_before_driver_or_host_start(
    #[case] command: &str,
    #[case] flags: &[&str],
) {
    let flags = flags
        .iter()
        .map(|flag| (*flag).to_owned())
        .collect::<Vec<_>>();
    assert!(is_help_request(command, &flags));
}

#[rstest]
fn ordinary_subcommands_are_not_help_requests() {
    assert!(!is_help_request(
        "host-jsonl",
        &["--metrics-output".into(), "metrics.json".into()]
    ));
}

#[rstest]
fn escalation_help_lists_the_typed_policy_contract() {
    let help = escalation_reason_help();
    for reason in dcc_cua_core::COMPUTER_USE_ESCALATION_REASONS {
        assert!(help.contains(reason.value));
        assert!(help.contains(reason.meaning));
    }
    assert!(help.contains("uia_timeout"));
    assert!(help.contains("escalation-detail"));
}

#[rstest]
fn manifest_advertises_session_escalation_reasons() {
    let escalation = &manifest::document()["host"]["session_escalation"];
    assert_eq!(escalation["method"], "escalate_session");
    assert_eq!(escalation["requires_explicit_grant"], true);
    assert_eq!(
        escalation["reason"]["enum"],
        json!(
            dcc_cua_core::COMPUTER_USE_ESCALATION_REASONS
                .iter()
                .map(|reason| reason.value)
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(
        escalation["reason"]["recommended"]["exact_window_uia_timeout"],
        "uia_timeout"
    );
    assert_eq!(
        escalation["fresh_observation_required_after_escalation"],
        true
    );
}

#[rstest]
fn showcase_is_added_only_to_open_session_grants() {
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-showcase-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut open = JsonlRequest {
        request_id: None,
        method: "open_session".into(),
        params: json!({"grant": {"task_grant_id": "task-1"}}),
    };
    enable_showcase(&mut open, directory.to_str().unwrap(), 7).unwrap();
    assert_eq!(open.params["grant"]["allow_recording"], true);
    assert!(
        open.params["grant"]["showcase_output_dir"]
            .as_str()
            .unwrap()
            .ends_with("session-7")
    );

    let mut snapshot = JsonlRequest {
        request_id: None,
        method: "snapshot".into(),
        params: json!({}),
    };
    enable_showcase(&mut snapshot, directory.to_str().unwrap(), 8).unwrap();
    assert_eq!(snapshot.params, json!({}));
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
fn showcase_session_directory_never_reuses_an_existing_recording() {
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-showcase-collision-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let first = reserve_showcase_session_directory(directory.to_str().unwrap(), 0).unwrap();
    std::fs::write(first.join("showcase.mp4"), b"existing").unwrap();
    let second = reserve_showcase_session_directory(directory.to_str().unwrap(), 0).unwrap();

    assert_ne!(first, second);
    assert_eq!(
        std::fs::read(first.join("showcase.mp4")).unwrap(),
        b"existing"
    );
    assert!(second.ends_with("session-0-1"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[rstest]
fn target_binding_requires_the_profile_selector_to_match() {
    let profile = dcc_cua_semantic_profiles::builtin_profile("ue").expect("UE profile");
    assert!(semantic_profile::target_matches_profile(
        profile,
        Some(&json!({
            "app_name": "UE4Editor.exe",
            "title": "UE426LookdevTest - 虚幻编辑器"
        }))
    ));
    assert!(!semantic_profile::target_matches_profile(
        profile,
        Some(&json!({
            "app_name": "notepad.exe",
            "title": "notes"
        }))
    ));
}

#[rstest]
fn profile_matching_prefers_a_unique_host_version_variant() {
    let catalog = ["maya", "maya-2024"]
        .into_iter()
        .map(|id| {
            (
                "builtin".to_owned(),
                dcc_cua_semantic_profiles::builtin_profile(id)
                    .expect("built-in Profile")
                    .clone(),
            )
        })
        .collect();
    let result = profile_match_result(catalog, "maya.exe", "Autodesk Maya 2024: scene.ma");

    assert_eq!(result["selected"], "maya-2024");
    assert_eq!(result["ambiguous"], false);
    assert_eq!(result["candidates"][0]["id"], "maya-2024");
    assert_eq!(result["candidates"][1]["id"], "maya");
}

#[rstest]
#[case(vec![], false)]
#[case(vec!["--stdio"], false)]
#[case(vec!["--grant", "existing-profile"], true)]
#[case(vec!["--grant=existing_profile"], true)]
fn host_existing_profile_grant_is_explicit(#[case] flags: Vec<&str>, #[case] expected: bool) {
    let flags = flags.into_iter().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(existing_profile_grant_requested(&flags).unwrap(), expected);
}

#[rstest]
fn host_grant_parser_rejects_missing_and_unknown_values() {
    assert!(existing_profile_grant_requested(&["--grant".into()]).is_err());
    assert!(existing_profile_grant_requested(&["--grant".into(), "everything".into()]).is_err());
}

#[rstest]
#[case(vec!["profiles"], None)]
#[case(vec!["__private-worker", "--generation", "worker-1"], Some("worker-1"))]
#[case(vec!["__private-worker", "--generation=worker-2"], Some("worker-2"))]
fn private_worker_entry_is_selected_before_the_async_cli(
    #[case] arguments: Vec<&str>,
    #[case] expected: Option<&str>,
) {
    let arguments = arguments.into_iter().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(
        private_worker_generation_from(&arguments)
            .unwrap()
            .as_deref(),
        expected
    );
}

#[rstest]
fn private_worker_entry_requires_a_generation() {
    assert!(private_worker_generation_from(&["__private-worker".into()]).is_err());
}

#[rstest]
fn macos_host_worker_is_private_and_standard_by_default() {
    let path = if cfg!(windows) {
        std::path::Path::new(r"C:\release\dcc-cua.exe")
    } else {
        std::path::Path::new("/release/dcc-cua")
    };
    let options = host_private_worker_options(path, false);
    assert_eq!(options.binary_path, path.to_string_lossy());
    assert_eq!(options.host_bundle_id, "com.dcc-cua.host");
    assert_eq!(
        options.configured_driver.authorization.compatibility_mode,
        dcc_cua_core::SessionPermissionMode::Standard
    );
    assert!(
        !options
            .configured_driver
            .authorization
            .unrestricted_acknowledged
    );
    assert!(options.environment.is_empty());
    assert!(options.inherit_stderr);
}

#[rstest]
fn explicit_existing_profile_grant_raises_only_the_private_worker_ceiling() {
    let path = if cfg!(windows) {
        std::path::Path::new(r"C:\release\dcc-cua.exe")
    } else {
        std::path::Path::new("/release/dcc-cua")
    };
    let options = host_private_worker_options(path, true);
    assert_eq!(
        options.configured_driver.authorization.compatibility_mode,
        dcc_cua_core::SessionPermissionMode::Unrestricted
    );
    assert!(
        options
            .configured_driver
            .authorization
            .unrestricted_acknowledged
    );
}

#[rstest]
fn response_image_reads_host_owned_shared_memory() {
    let image = dcc_cua_shm::SharedImage::from_bytes(b"png", "image/png").unwrap();
    let response = HostResponse {
        value: json!({"image": image.descriptor()}),
        binary_attachment: None,
    };
    assert_eq!(
        response_image(&response, SnapshotTransport::SharedMemory).unwrap(),
        b"png"
    );
}

#[rstest]
fn jsonl_response_writes_host_owned_shared_memory() {
    let image = dcc_cua_shm::SharedImage::from_bytes(b"png", "image/png").unwrap();
    let mut descriptor = serde_json::to_value(image.descriptor()).unwrap();
    descriptor["encoding"] = json!("shared_memory");
    let response = HostResponse {
        value: json!({"image": descriptor}),
        binary_attachment: None,
    };
    let output_dir = std::env::temp_dir().join(format!(
        "dcc-cua-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&output_dir).unwrap();
    let value = jsonl_response_value_with_metrics(response, output_dir.to_str(), 7)
        .unwrap()
        .value;
    let output = output_dir.join("response-7.bin");
    assert_eq!(std::fs::read(&output).unwrap(), b"png");
    assert_eq!(
        std::path::Path::new(value["_dcc_cua_binary_output"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        output.canonicalize().unwrap()
    );
    std::fs::remove_dir_all(output_dir).unwrap();
}

#[rstest]
fn jsonl_response_preserves_shared_memory_when_no_output_directory_is_requested() {
    let image = dcc_cua_shm::SharedImage::from_bytes(b"png", "image/png").unwrap();
    let mut descriptor = serde_json::to_value(image.descriptor()).unwrap();
    descriptor["encoding"] = json!("shared_memory");
    let response = HostResponse {
        value: json!({"request_id": "core-snapshot-1", "image": descriptor.clone()}),
        binary_attachment: None,
    };

    let value = jsonl_response_value_with_metrics(response, None, 7)
        .unwrap()
        .value;

    assert_eq!(value["request_id"], "core-snapshot-1");
    assert_eq!(value["image"], descriptor);
    assert!(value.get("_dcc_cua_binary_output").is_none());
}

#[rstest]
fn jsonl_output_error_preserves_the_host_request_id() {
    let request_id = json!("core-snapshot-1");
    let value = jsonl_output_error_value("shared memory unavailable".into(), Some(&request_id));

    assert_eq!(value["type"], "error");
    assert_eq!(value["code"], "output_error");
    assert_eq!(value["request_id"], "core-snapshot-1");
}

#[rstest]
fn host_jsonl_metrics_charge_binary_and_shared_memory_images_equally() {
    let png = [
        137, 80, 78, 71, 13, 10, 26, 10, // PNG signature
        0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR
        0, 0, 0, 3, 0, 0, 0, 2, // 3 x 2 pixels
    ];
    let binary = HostResponse {
        value: json!({
            "type": "snapshot",
            "image": {
                "encoding": "binary_frame",
                "length": png.len(),
                "mime_type": "image/png"
            }
        }),
        binary_attachment: Some(png.to_vec()),
    };
    let shared_image = dcc_cua_shm::SharedImage::from_bytes(&png, "image/png").unwrap();
    let mut descriptor = serde_json::to_value(shared_image.descriptor()).unwrap();
    descriptor["encoding"] = json!("shared_memory");
    let shared = HostResponse {
        value: json!({"type": "snapshot", "image": descriptor}),
        binary_attachment: None,
    };
    let output_dir = tempfile::tempdir().unwrap();

    let mut binary_metrics = HostJsonlMetrics::default();
    measured_jsonl_response_value(
        binary,
        output_dir.path().to_str(),
        1,
        &mut binary_metrics,
        "snapshot",
    )
    .unwrap();
    let mut shared_metrics = HostJsonlMetrics::default();
    measured_jsonl_response_value(
        shared,
        output_dir.path().to_str(),
        2,
        &mut shared_metrics,
        "snapshot",
    )
    .unwrap();
    let binary_report =
        binary_metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);
    let shared_report =
        shared_metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);

    assert_eq!(
        binary_report.image_outputs_total,
        shared_report.image_outputs_total
    );
    assert_eq!(
        binary_report.image_pixels_total,
        shared_report.image_pixels_total
    );
    assert_eq!(
        binary_report.image_encoded_bytes_total,
        shared_report.image_encoded_bytes_total
    );
    assert_eq!(binary_report.image_outputs_total, 1);
    assert_eq!(binary_report.image_pixels_total, 6);
    assert_eq!(binary_report.image_encoded_bytes_total, png.len() as u64);
    assert_eq!(binary_report.image_unknown_dimensions_total, 0);
    assert_eq!(shared_report.image_unknown_dimensions_total, 0);
}

#[rstest]
fn host_jsonl_metrics_count_images_with_unknown_dimensions() {
    let jpeg = [0xff, 0xd8, 0xff, 0xd9];
    let response = HostResponse {
        value: json!({
            "type": "snapshot",
            "image": {
                "encoding": "binary_frame",
                "length": jpeg.len(),
                "mime_type": "image/jpeg"
            }
        }),
        binary_attachment: Some(jpeg.to_vec()),
    };
    let output_dir = tempfile::tempdir().unwrap();
    let mut metrics = HostJsonlMetrics::default();

    measured_jsonl_response_value(
        response,
        output_dir.path().to_str(),
        1,
        &mut metrics,
        "snapshot",
    )
    .unwrap();
    let report = metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);

    assert_eq!(report.image_outputs_total, 1);
    assert_eq!(report.image_pixels_total, 0);
    assert_eq!(report.image_encoded_bytes_total, jpeg.len() as u64);
    assert_eq!(report.image_unknown_dimensions_total, 1);
}

#[rstest]
fn failed_post_snapshot_reports_that_the_action_already_ran() {
    let value = window_post_snapshot_value(
        Err(dcc_cua_core::ComputerUseError::new(
            dcc_cua_core::ComputerUseErrorCode::CaptureFailed,
            "capture failed",
        )),
        None,
    );
    assert_eq!(value["success"], false);
    assert_eq!(value["action_was_executed"], true);
    assert_eq!(value["code"], "capture_failed");
}

#[rstest]
fn semantic_post_snapshot_keeps_accessibility_evidence_without_pixels() {
    let accessibility = json!({"elements": [{"role": "button"}]});
    let value = semantic_post_snapshot_value(Ok(accessibility.clone()), Some("ignored.png".into()));
    assert_eq!(value["success"], true);
    assert_eq!(value["observation_kind"], "accessibility");
    assert_eq!(value["accessibility"], accessibility);
    assert_eq!(value["node_count"], 1);
    assert!(value["output"].is_null());
    assert!(value["output_error"].is_string());
}

#[rstest]
fn profile_action_requires_a_supported_target_action() {
    let profile = builtin_profile("maya").unwrap();
    let target = profile.resolve_target("home", "new_scene").unwrap();
    assert!(target.supports_action("click"));
    assert!(!target.supports_action("set_value"));
}

#[rstest]
fn profile_loader_accepts_user_authored_json() {
    let path = std::env::temp_dir().join(format!("dcc-cua-profile-{}.json", std::process::id()));
    std::fs::write(
        &path,
        r#"{
            "schema_version": 3,
            "id": "custom-maya",
            "profile_version": "1.0.0",
            "application": {"family": "autodesk-maya", "versions": []},
            "display_name": "Custom Maya",
            "selectors": [{
                "application_names": ["maya.exe"],
                "localized_window_title_contains": {"zh-CN": ["玛雅"]}
            }],
            "surfaces": [],
            "settings": {"dialog_style": "os_native", "preferred_route": "accessibility"}
        }"#,
    )
    .unwrap();
    let path = path.to_string_lossy().into_owned();
    let profile = load_semantic_profile(&strings(["--profile-file", &path])).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(profile.id, "custom-maya");
    assert_eq!(profile.supported_locales(), ["zh-CN"]);
}

#[rstest]
fn host_batch_parser_requires_read_only_request_shapes() {
    let requests = parse_host_batch(json!([
        {"method":"list_apps"},
        {"method":"screen_size","params":{}}
    ]))
    .unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "list_apps");
    assert_eq!(requests[0].request_id, None);
    let requests = parse_host_batch(json!([
        {"request_id":"core-read-1","method":"list_apps"}
    ]))
    .unwrap();
    assert_eq!(requests[0].request_id.as_deref(), Some("core-read-1"));
    assert!(parse_host_batch(json!({"method":"list_apps"})).is_err());
    assert!(
        parse_host_batch(json!([
            {"request_id":"","method":"list_apps"}
        ]))
        .is_err()
    );
    assert!(
        parse_host_batch(json!([
            {"method":"screen_size","params":[]}
        ]))
        .is_err()
    );
}

#[rstest]
fn jsonl_parser_requires_method_and_object_params() {
    let request = parse_jsonl_request(r#"{"method":"list_apps"}"#).unwrap();
    assert_eq!(request.request_id, None);
    assert_eq!(request.method, "list_apps");
    assert_eq!(request.params, json!({}));
    let request = parse_jsonl_request(r#"{"request_id":"core-42","method":"list_apps"}"#).unwrap();
    assert_eq!(request.request_id.as_deref(), Some("core-42"));
    assert!(parse_jsonl_request(r#"{"request_id":"","method":"list_apps"}"#).is_err());
    assert!(parse_jsonl_request("[]").is_err());
    assert!(parse_jsonl_request(r#"{"method":"list_apps","params":[]}"#).is_err());
}

#[rstest]
fn jsonl_parser_accepts_a_utf8_bom_on_the_first_line() {
    let request = parse_jsonl_request("\u{feff}{\"method\":\"list_apps\"}").unwrap();
    assert_eq!(request.method, "list_apps");
}

#[rstest]
fn parallel_discovery_is_limited_to_stateless_methods() {
    assert!(is_parallel_discovery_method("ping"));
    assert!(is_parallel_discovery_method("list_apps"));
    assert!(is_parallel_discovery_method("cursor_position"));
    assert!(!is_parallel_discovery_method("snapshot"));
    assert!(!is_parallel_discovery_method("desktop_snapshot"));
}

#[rstest]
fn host_jsonl_metrics_separate_actions_from_observation_cost() {
    let mut metrics = HostJsonlMetrics::default();
    let action =
        parse_jsonl_request(r#"{"method":"execute_action","params":{"capture_after":true}}"#)
            .unwrap();
    let snapshot = parse_jsonl_request(r#"{"method":"snapshot","params":{}}"#).unwrap();
    metrics.record_input(r#"{"method":"execute_action"}"#);
    metrics.record_request(&action);
    metrics.record_request(&snapshot);
    metrics.record_output(&json!({"type": "action_completed"}), 40);

    let report = metrics.report(
        HostJsonlRunStatus::Succeeded,
        std::time::Duration::from_millis(125),
    );
    assert_eq!(report.run_status, HostJsonlRunStatus::Succeeded);
    assert_eq!(report.transport_success, Some(true));
    assert_eq!(report.action_requests_total, 1);
    assert_eq!(report.post_action_snapshot_requests_total, 1);
    assert_eq!(report.standalone_snapshot_requests_total, 1);
    assert_eq!(report.errors_total, 0);
    assert_eq!(report.elapsed_ms, 125);
    assert_eq!(report.json_output_bytes, 40);
    assert_eq!(report.action_kinds.get("click"), None);
}

#[rstest]
fn host_jsonl_metrics_count_capture_after_as_a_visual_observation_request() {
    let mut metrics = HostJsonlMetrics::default();
    let request = parse_jsonl_request(
        r#"{"method":"execute_action","params":{"capture_after":true,"action":{"action":"click"}}}"#,
    )
    .unwrap();

    metrics.record_request(&request);
    let report = metrics.report(
        HostJsonlRunStatus::Succeeded,
        std::time::Duration::from_millis(10),
    );

    assert_eq!(report.action_attempts_total, 1);
    assert_eq!(report.visual_observation_requests_total, 1);
    assert_eq!(report.semantic_observation_requests_total, 0);
    assert_eq!(report.post_action_observation_requests_total, 1);
}

#[rstest]
fn host_jsonl_metrics_count_desktop_capture_after_as_a_visual_observation_request() {
    let mut metrics = HostJsonlMetrics::default();
    let request = parse_jsonl_request(
        r#"{"method":"execute_desktop_action","params":{"capture_after":true,"action":{"action":"click"}}}"#,
    )
    .unwrap();

    metrics.record_request(&request);
    let report = metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);

    assert_eq!(report.action_attempts_total, 1);
    assert_eq!(report.visual_observation_requests_total, 1);
    assert_eq!(report.post_action_observation_requests_total, 1);
    assert_eq!(report.post_action_snapshot_requests_total, 1);
}

#[rstest]
fn host_jsonl_metrics_count_failed_actions_as_attempted_and_rejected() {
    let request = parse_jsonl_request(
        r#"{"method":"execute_action","params":{"action":{"action":"click"}}}"#,
    )
    .unwrap();

    for response in [
        json!({"type": "error", "code": "stale_observation"}),
        json!({
            "type": "action_completed",
            "success": false,
            "error": "approval_required"
        }),
    ] {
        let mut metrics = HostJsonlMetrics::default();
        metrics.record_request(&request);
        metrics.record_response(&request.method, &response);
        let report = metrics.report(HostJsonlRunStatus::Failed, std::time::Duration::ZERO);

        assert_eq!(report.action_attempts_total, 1);
        assert_eq!(report.action_succeeded_total, 0);
        assert_eq!(report.action_rejected_total, 1);
    }
}

#[rstest]
fn host_jsonl_metrics_count_successful_action_responses() {
    let mut metrics = HostJsonlMetrics::default();
    let request = parse_jsonl_request(
        r#"{"method":"execute_action","params":{"action":{"action":"click"}}}"#,
    )
    .unwrap();
    metrics.record_request(&request);

    measured_jsonl_response_value(
        HostResponse {
            value: json!({"type": "action_completed", "success": true}),
            binary_attachment: None,
        },
        None,
        0,
        &mut metrics,
        &request.method,
    )
    .unwrap();
    let report = metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);

    assert_eq!(report.action_attempts_total, 1);
    assert_eq!(report.action_succeeded_total, 1);
    assert_eq!(report.action_rejected_total, 0);
}

#[rstest]
fn host_jsonl_metrics_count_live_observation_lifecycle_and_final_state() {
    let mut metrics = HostJsonlMetrics::default();
    let start = parse_jsonl_request(r#"{"method":"live_observation_start","params":{}}"#).unwrap();
    let stop = parse_jsonl_request(r#"{"method":"live_observation_stop","params":{}}"#).unwrap();

    metrics.record_request(&start);
    metrics.record_response(
        &start.method,
        &json!({"type": "live_observation_started", "result": {"active": true}}),
    );
    metrics.record_request(&stop);
    metrics.record_response(
        &stop.method,
        &json!({"type": "live_observation_stopped", "result": {"active": false}}),
    );
    let report = metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);

    assert_eq!(report.live_observation_start_requests_total, 1);
    assert_eq!(report.live_observation_stop_requests_total, 1);
    assert_eq!(report.live_observation_final_states_total, 1);
}

#[rstest]
fn host_jsonl_metrics_separate_semantic_and_visual_observation_requests() {
    let mut metrics = HostJsonlMetrics::default();
    let semantic =
        parse_jsonl_request(r#"{"method":"accessibility_snapshot","params":{}}"#).unwrap();
    let visual = parse_jsonl_request(r#"{"method":"zoom","params":{}}"#).unwrap();
    let verified_with_image =
        parse_jsonl_request(r#"{"method":"verify_state","params":{"include_screenshot":true}}"#)
            .unwrap();

    metrics.record_request(&semantic);
    metrics.record_request(&visual);
    metrics.record_request(&verified_with_image);
    let report = metrics.report(HostJsonlRunStatus::Succeeded, std::time::Duration::ZERO);

    assert_eq!(report.semantic_observation_requests_total, 2);
    assert_eq!(report.visual_observation_requests_total, 2);
    assert_eq!(report.post_action_observation_requests_total, 0);
}

#[rstest]
fn host_jsonl_metrics_count_protocol_errors_without_claiming_task_failure() {
    let mut metrics = HostJsonlMetrics::default();
    metrics.record_output(&json!({"type": "error", "code": "invalid_request"}), 52);
    let report = metrics.report(HostJsonlRunStatus::Failed, std::time::Duration::ZERO);
    assert_eq!(report.transport_success, Some(false));
    assert_eq!(report.errors_total, 1);
    assert_eq!(report.error_codes["invalid_request"], 1);
    assert_eq!(report.schema, "dcc-cua.host-jsonl.metrics.v3");
}

#[rstest]
fn host_jsonl_metrics_break_down_action_kinds() {
    let mut metrics = HostJsonlMetrics::default();
    for request in [
        r#"{"method":"execute_action","params":{"action":{"action":"move"}}}"#,
        r#"{"method":"execute_action","params":{"action":{"action":"click"}}}"#,
        r#"{"method":"execute_desktop_action","params":{"action":{"action":"click"}}}"#,
    ] {
        metrics.record_request(&parse_jsonl_request(request).unwrap());
    }

    let report = metrics.report(HostJsonlRunStatus::Running, std::time::Duration::ZERO);
    assert_eq!(report.action_requests_total, 3);
    assert_eq!(report.action_kinds["move"], 1);
    assert_eq!(report.action_kinds["click"], 2);
}

#[rstest]
fn host_jsonl_metrics_checkpoint_is_readable_before_eof() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("metrics.json");
    let mut metrics = HostJsonlMetrics::with_output(Some(path.clone()), Instant::now());
    metrics.checkpoint().unwrap();

    let running: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(running["schema"], "dcc-cua.host-jsonl.metrics.v3");
    assert_eq!(running["run_status"], "running");
    assert!(running["transport_success"].is_null());
    assert_eq!(running["requests_total"], 0);
    assert_eq!(running["action_attempts_total"], 0);
    assert_eq!(running["action_succeeded_total"], 0);
    assert_eq!(running["action_rejected_total"], 0);
    assert_eq!(running["visual_observation_requests_total"], 0);
    assert_eq!(running["semantic_observation_requests_total"], 0);
    assert_eq!(running["post_action_observation_requests_total"], 0);
    assert_eq!(running["image_outputs_total"], 0);
    assert_eq!(running["image_pixels_total"], 0);
    assert_eq!(running["image_encoded_bytes_total"], 0);
    assert_eq!(running["image_unknown_dimensions_total"], 0);
    assert_eq!(running["live_observation_start_requests_total"], 0);
    assert_eq!(running["live_observation_stop_requests_total"], 0);
    assert_eq!(running["live_observation_final_states_total"], 0);

    let request = parse_jsonl_request(r#"{"method":"snapshot","params":{}}"#).unwrap();
    metrics.record_request(&request);
    metrics.record_output(&json!({"type": "snapshot"}), 24);
    metrics.checkpoint().unwrap();
    let updated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(updated["run_status"], "running");
    assert_eq!(updated["requests_total"], 1);
    assert_eq!(updated["standalone_snapshot_requests_total"], 1);
    assert_eq!(updated["visual_observation_requests_total"], 1);
    assert_eq!(updated["action_kinds"], json!({}));
    assert_eq!(updated["error_codes"], json!({}));

    metrics.finish(true).unwrap();
    let finished: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(finished["run_status"], "succeeded");
    assert_eq!(finished["transport_success"], true);
}

#[rstest]
fn updater_requires_the_exact_archive_and_checksum_sidecar() {
    let target = self_update::get_target();
    let archive = update::release_archive_name("0.1.0", target);
    let releases = [self_update::update::Release {
        version: "0.1.0".into(),
        assets: vec![
            self_update::update::ReleaseAsset {
                name: format!("{archive}.sha256"),
                download_url: format!(
                    "https://github.com/dcc-mcp/dcc-cua/releases/download/v0.1.0/{archive}.sha256"
                ),
            },
            self_update::update::ReleaseAsset {
                name: archive.clone(),
                download_url: format!(
                    "https://github.com/dcc-mcp/dcc-cua/releases/download/v0.1.0/{archive}"
                ),
            },
        ],
        ..Default::default()
    }];
    let (_, selected, checksum) = update::latest_release_assets(&releases, target).unwrap();
    assert_eq!(selected.name, archive);
    assert!(selected.download_url.ends_with(&archive));
    assert_eq!(checksum.name, format!("{archive}.sha256"));
}

#[rstest]
#[case("x86_64-pc-windows-msvc", "zip")]
#[case("x86_64-unknown-linux-gnu", "tar.gz")]
#[case("aarch64-apple-darwin", "tar.gz")]
fn updater_derives_archive_extension_from_selected_target(
    #[case] target: &str,
    #[case] extension: &str,
) {
    assert!(update::release_archive_name("1.2.3", target).ends_with(extension));
}

#[rstest]
fn updater_rejects_an_asset_url_outside_the_exact_official_release() {
    let target = "x86_64-pc-windows-msvc";
    let archive = update::release_archive_name("0.1.0", target);
    let releases = [self_update::update::Release {
        version: "0.1.0".into(),
        assets: vec![
            self_update::update::ReleaseAsset {
                name: archive.clone(),
                download_url: format!("https://example.test/{archive}"),
            },
            self_update::update::ReleaseAsset {
                name: format!("{archive}.sha256"),
                download_url: format!("https://example.test/{archive}.sha256"),
            },
        ],
        ..Default::default()
    }];
    assert!(update::latest_release_assets(&releases, target).is_none());
}

#[rstest]
fn updater_rejects_a_sidecar_for_another_archive() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("dcc-cua.zip");
    std::fs::write(&archive, b"release bytes").unwrap();
    let digest = format!("{:x}", sha2::Sha256::digest(b"release bytes"));
    let error = update::verify_sha256(&archive, &format!("{digest}  other.zip"), "dcc-cua.zip")
        .unwrap_err();
    assert!(error.to_string().contains("exact archive"));
}

#[rstest]
fn manifest_is_a_machine_readable_core_launch_contract() {
    let manifest = manifest::document();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["rust_version"], "1.95");
    assert_eq!(manifest["host"]["protocol_version"], 1);
    assert_eq!(manifest["host"]["ensure_command"], json!(["host-ensure"]));
    assert_eq!(
        manifest["host"]["max_connections"],
        dcc_cua_host::MAX_HOST_CONNECTIONS
    );
    assert_eq!(
        manifest["host"]["session_concurrency"]["max_sessions_per_connection"],
        dcc_cua_host::MAX_SESSIONS_PER_CONNECTION
    );
    assert_eq!(
        manifest["host"]["session_concurrency"]["model"],
        "one_connection_per_agent"
    );
    assert_eq!(
        manifest["host"]["session_concurrency"]["raw_input_arbitration"],
        "host_global_fifo"
    );
    assert_eq!(manifest["host"]["hello_timeout_ms"], 10_000);
    assert_eq!(manifest["host"]["max_parallel_discovery_requests"], 32);
    assert_eq!(
        MAX_PARALLEL_DISCOVERY_REQUESTS,
        dcc_cua_client::MAX_PARALLEL_DISCOVERY_REQUESTS
    );
    assert_eq!(
        manifest["host"]["grant_limits"]["task_grant_id_max_chars"],
        128
    );
    assert_eq!(
        manifest["host"]["grant_limits"]["application_label_max_chars"],
        80
    );
    assert_eq!(manifest["core_bridge"]["rust_crate"], "dcc-cua-client");
    assert_eq!(manifest["core_bridge"]["command"], json!(["host-jsonl"]));
    assert_eq!(
        manifest["core_bridge"]["preferred_snapshot_transport"],
        "shared_memory"
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "browser_exact_binding"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values
                .iter()
                .any(|value| value == "trusted_confirmation_grants"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "two_axis_scroll"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "scoped_window_frame"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "native_menu_path"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "host_ping"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "host_diagnostics"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "host_wide_interrupt"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values
                .iter()
                .any(|value| value == "session_scoped_application_lifecycle"))
    );
    assert!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == "cursor_controls"))
    );
    assert_eq!(
        manifest["host"]["capabilities"]
            .as_array()
            .is_some_and(|values| values
                .iter()
                .any(|value| value == "windows_background_uia_fallback")),
        cfg!(windows)
    );
    assert_eq!(manifest["runtime"]["backend"], "cua-driver-sdk");
    assert_eq!(manifest["runtime"]["separate_driver_required"], false);
    assert!(manifest.get("upstream_driver").is_none());
}

#[rstest]
fn wait_window_builds_a_bounded_window_query() {
    let request = window_wait_request(&strings([
        "--app",
        "UE5Editor.exe",
        "--title",
        "PCG Fab",
        "--on-screen",
        "--timeout-ms",
        "12000",
        "--poll-ms",
        "250",
    ]))
    .unwrap();
    assert_eq!(request.query.app.as_deref(), Some("UE5Editor.exe"));
    assert_eq!(request.query.window_title.as_deref(), Some("PCG Fab"));
    assert!(request.query.on_screen_only);
    assert_eq!(request.timeout_ms, Some(12000));
    assert_eq!(request.interval_ms, Some(250));
    assert!(window_wait_request(&strings(["--on-screen"])).is_err());
}

#[rstest]
fn window_frame_parser_requires_a_valid_complete_frame() {
    let frame = window_frame_request(&strings([
        "--x", "-1920.5", "--y", "12.25", "--width", "1280", "--height", "720",
    ]))
    .unwrap();
    assert_eq!(frame.x, -1920.5);
    assert_eq!(frame.y, 12.25);
    assert_eq!(frame.width, 1280.0);
    assert_eq!(frame.height, 720.0);
    assert!(window_frame_request(&strings(["--x", "0"])).is_err());
    assert!(
        window_frame_request(&strings([
            "--x", "0", "--y", "0", "--width", "0", "--height", "720",
        ]))
        .is_err()
    );
}

#[rstest]
fn menu_parser_preserves_repeated_native_path_segments() {
    let request = menu_request(&strings([
        "--menu", "Window", "--menu", "Arrange", "--menu", "Left",
    ]))
    .unwrap();
    assert_eq!(request.path, ["Window", "Arrange", "Left"]);
    assert!(menu_request(&strings([])).is_err());
    assert!(menu_request(&strings(["--menu", " "])).is_err());
}

#[rstest]
fn list_window_filters_select_one_exact_window() {
    let query = ComputerUseWindowQuery {
        app: Some("ue5editor.exe".into()),
        window_handle: Some(7),
        window_title: Some("PCG Fab".into()),
        ..Default::default()
    };
    query.validate_selectors().unwrap();
    let mut windows = vec![
        json!({
            "app_name": "UE5Editor.exe",
            "window_id": 7,
            "title": "PCG Fab"
        }),
        json!({
            "app_name": "UE5Editor.exe",
            "window_id": 8,
            "title": "Output Log"
        }),
    ];
    windows.retain(|window| query.matches_window(window));
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["window_id"], 7);
    assert!(
        ComputerUseWindowQuery {
            app: Some(String::new()),
            ..Default::default()
        }
        .validate_selectors()
        .is_err()
    );
}

#[rstest]
fn friendly_actions_build_bounded_cua_requests() {
    let click = action_from_command("click", &strings(["--x", "10", "--y", "20"])).unwrap();
    assert_eq!(click.action, "click");
    assert_eq!(click.x, Some(10.0));
    assert_eq!(click.y, Some(20.0));

    let semantic_click = action_from_command("click", &strings(["--element-index", "7"])).unwrap();
    assert_eq!(semantic_click.element_index, Some(7));
    assert_eq!(semantic_click.x, None);

    let middle_click = action_from_command(
        "click",
        &strings(["--x", "10", "--y", "20", "--button", "middle"]),
    )
    .unwrap();
    assert_eq!(middle_click.button.as_deref(), Some("middle"));

    let toggle =
        action_from_command("toggle", &strings(["--element-token", "checkbox-1"])).unwrap();
    assert_eq!(toggle.action, "toggle");
    assert_eq!(toggle.element_token.as_deref(), Some("checkbox-1"));

    let set_value = action_from_command(
        "set-value",
        &strings(["--element-index", "3", "--value", "Published"]),
    )
    .unwrap();
    assert_eq!(set_value.action, "set_value");
    assert_eq!(set_value.text.as_deref(), Some("Published"));

    let drag = action_from_command(
        "drag",
        &strings([
            "--from-x", "1", "--from-y", "2", "--to-x", "30", "--to-y", "40",
        ]),
    )
    .unwrap();
    assert_eq!(drag.path.len(), 2);
    assert!(action_from_command("click", &strings(["--x", "10"])).is_err());
    assert!(
        action_from_command(
            "click",
            &strings(["--element-index", "7", "--element-token", "same"])
        )
        .is_err()
    );
    assert!(
        action_from_command(
            "click",
            &strings(["--element-index", "7", "--x", "10", "--y", "20"])
        )
        .is_err()
    );

    let type_chars = action_from_command(
        "type",
        &strings(["--text", "hello", "--focused", "--delay-ms", "25"]),
    )
    .unwrap();
    assert_eq!(type_chars.action, "type_chars");
    assert!(type_chars.type_chars_only);
    assert_eq!(type_chars.delay_ms, Some(25));

    let semantic_type = action_from_command(
        "type",
        &strings(["--text", "hello", "--element-index", "8"]),
    )
    .unwrap();
    assert_eq!(semantic_type.action, "type");
    assert_eq!(semantic_type.element_index, Some(8));
    assert!(!semantic_type.type_chars_only);
    let pixel_type = action_from_command(
        "type",
        &strings(["--text", "hello", "--x", "12", "--y", "34"]),
    )
    .unwrap();
    assert_eq!(pixel_type.action, "type");
    assert_eq!(pixel_type.x, Some(12.0));
    assert_eq!(pixel_type.y, Some(34.0));

    let press = action_from_command(
        "press",
        &strings(["--key", "S", "--modifier", "CTRL", "--x", "12", "--y", "34"]),
    )
    .unwrap();
    assert_eq!(press.modifiers, vec!["CTRL"]);
    assert_eq!(press.x, Some(12.0));

    let held_click = action_from_command(
        "click",
        &strings(["--x", "12", "--y", "34", "--duration-ms", "320"]),
    )
    .unwrap();
    assert_eq!(held_click.duration_ms, Some(320));

    let drag = action_from_command(
        "drag",
        &strings([
            "--from-x",
            "1",
            "--from-y",
            "2",
            "--to-x",
            "30",
            "--to-y",
            "40",
            "--button",
            "middle",
            "--modifier",
            "ALT",
            "--duration-ms",
            "750",
            "--steps",
            "32",
        ]),
    )
    .unwrap();
    assert_eq!(drag.button.as_deref(), Some("middle"));
    assert_eq!(drag.modifiers, vec!["ALT"]);
    assert_eq!(drag.duration_ms, Some(750));
    assert_eq!(drag.steps, Some(32));
    assert!(action_from_command("type", &strings(["--text", "hello", "--x", "12"])).is_err());
    assert!(action_from_command("hotkey", &strings(["--key", "CTRL"])).is_err());
    assert_eq!(
        bounded_u32(&strings(["--max-depth", "8"]), "--max-depth", 64, 64).unwrap(),
        8
    );
    assert!(bounded_u32(&strings(["--max-depth", "65"]), "--max-depth", 64, 64).is_err());
}

#[rstest]
fn friendly_visual_action_maps_the_visible_snapshot_into_the_fresh_observation() {
    let mut action = ComputerUseAction {
        action: "double_click".into(),
        x: Some(1318.0),
        y: Some(700.0),
        ..Default::default()
    };
    let observation = ComputerUseObservation {
        observation_id: "fresh".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "Houdini".into(),
        width: 3840,
        height: 2280,
        source_rect: [0, 0, 3840, 2280],
        capture_backend: "dcc-cua-wgc-exact-window".into(),
        capture_provenance: json!({"pixels_captured": true}),
        session_id: "session".into(),
    };

    map_visible_snapshot_coordinates(&mut action, Some((1568, 931)), &observation).unwrap();
    assert_eq!(action.x, Some(1318.0 * 3840.0 / 1568.0));
    assert_eq!(action.y, Some(700.0 * 2280.0 / 931.0));
}

#[rstest]
fn restore_activate_requires_an_exact_pid_and_window_pair() {
    assert!(require_exact_window_target(&strings(["--pid", "42", "--window-id", "7"])).is_ok());
    assert!(require_exact_window_target(&strings(["--pid", "42"])).is_err());
    assert!(require_exact_window_target(&strings(["--window-id", "7"])).is_err());
    assert!(require_exact_window_target(&strings(["--app", "TheBazaar.exe"])).is_err());
}

#[rstest]
fn visible_snapshot_dimensions_require_a_complete_positive_pair() {
    assert!(visible_snapshot_dimensions(&strings(["--observation-width", "1568"])).is_err());
    assert!(
        visible_snapshot_dimensions(&strings([
            "--observation-width",
            "0",
            "--observation-height",
            "931",
        ]))
        .is_err()
    );
    assert_eq!(
        visible_snapshot_dimensions(&strings([
            "--observation-width",
            "1568",
            "--observation-height",
            "931",
        ]))
        .unwrap(),
        Some((1568, 931))
    );
}

#[rstest]
#[case("press", "escape", "keypress")]
#[case("press_key", "h", "keypress")]
#[case("hotkey", "b", "keyboard_shortcut")]
fn raw_action_json_accepts_documented_keyboard_aliases(
    #[case] alias: &str,
    #[case] key: &str,
    #[case] canonical: &str,
) {
    let action =
        action_from_json(&serde_json::json!({"action": alias, "key": key}).to_string()).unwrap();

    assert_eq!(action.action, canonical);
    assert_eq!(action.keys, vec![key]);
}

#[rstest]
fn friendly_scroll_preserves_axis_target_and_granularity() {
    let action = action_from_command(
        "scroll",
        &strings([
            "--scroll-x",
            "-4",
            "--by",
            "page",
            "--x",
            "120",
            "--y",
            "80",
        ]),
    )
    .unwrap();
    assert_eq!(action.scroll_x, Some(-4));
    assert_eq!(action.scroll_by.as_deref(), Some("page"));
    assert_eq!((action.x, action.y), (Some(120.0), Some(80.0)));
    assert!(
        action_from_command(
            "scroll",
            &strings(["--element-index", "7", "--x", "120", "--y", "80"])
        )
        .is_err()
    );
}

#[rstest]
fn activated_actions_use_foreground_and_fresh_semantic_tokens() {
    let mut action = ComputerUseAction {
        action: "keypress".into(),
        element_index: Some(8),
        ..Default::default()
    };
    default_activated_action_to_foreground(&strings(["--activate"]), &mut action);
    bind_fresh_element_token(
        &mut action,
        &json!({
            "elements": [{"element_index": 8, "element_token": "snapshot:8"}]
        }),
    );
    assert_eq!(action.delivery_mode.as_deref(), Some("foreground"));
    assert_eq!(action.element_token.as_deref(), Some("snapshot:8"));
    assert_eq!(action.element_index, None);

    action.delivery_mode = Some("background".into());
    default_activated_action_to_foreground(&strings(["--activate"]), &mut action);
    assert_eq!(action.delivery_mode.as_deref(), Some("background"));
}

#[rstest]
#[case("background")]
#[case("foreground")]
fn friendly_actions_honor_explicit_delivery_mode(#[case] mode: &str) {
    let action = action_from_command(
        "press",
        &strings(["--key", "SPACE", "--delivery-mode", mode]),
    )
    .unwrap();
    assert_eq!(action.delivery_mode.as_deref(), Some(mode));
}

#[rstest]
fn friendly_actions_reject_unknown_delivery_mode() {
    let error = action_from_command(
        "press",
        &strings(["--key", "SPACE", "--delivery-mode", "automatic"]),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "--delivery-mode must be background or foreground"
    );
}

#[rstest]
fn host_ensure_requires_the_cli_host_version() {
    validate_host_version(&json!({"host_version": env!("CARGO_PKG_VERSION")})).unwrap();

    let mismatch = validate_host_version(&json!({"host_version": "0.5.2"})).unwrap_err();
    assert!(mismatch.contains("running 0.5.2"));
    assert!(mismatch.contains(env!("CARGO_PKG_VERSION")));

    assert_eq!(
        validate_host_version(&json!({})).unwrap_err(),
        "Host ping did not report host_version"
    );
}

#[rstest]
fn cli_preserves_completed_action_restore_diagnostics() {
    let value = action_result_value(ComputerUseToolResult {
        value: json!({
            "success": true,
            "action_executed": true,
            "foreground_restore": {
                "requested": true,
                "success": false,
                "message": "foreground changed"
            }
        }),
        text: "clicked".into(),
        images: Vec::new(),
        degraded: false,
    });

    assert_eq!(value["success"], true);
    assert_eq!(value["action_executed"], true);
    assert_eq!(value["foreground_restore"]["success"], false);
    assert_eq!(value["degraded"], false);
    assert_eq!(value["image_count"], 0);
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}
