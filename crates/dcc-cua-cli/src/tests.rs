use rstest::rstest;
use serde_json::json;

use super::actions::{
    action_from_command, bind_fresh_element_token, default_activated_action_to_foreground,
    menu_request, window_frame_request,
};
use super::authorization::{existing_profile_grant_requested, host_private_worker_options};
use super::*;

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
    let value = jsonl_response_value(response, output_dir.to_str(), 7).unwrap();
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
fn host_jsonl_metrics_separate_decisions_from_observation_cost() {
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
fn host_jsonl_metrics_count_protocol_errors_without_claiming_task_failure() {
    let mut metrics = HostJsonlMetrics::default();
    metrics.record_output(&json!({"type": "error", "code": "invalid_request"}), 52);
    let report = metrics.report(HostJsonlRunStatus::Failed, std::time::Duration::ZERO);
    assert_eq!(report.transport_success, Some(false));
    assert_eq!(report.errors_total, 1);
    assert_eq!(report.error_codes["invalid_request"], 1);
    assert_eq!(report.schema, "dcc-cua.host-jsonl.metrics.v2");
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
    assert_eq!(running["schema"], "dcc-cua.host-jsonl.metrics.v2");
    assert_eq!(running["run_status"], "running");
    assert!(running["transport_success"].is_null());
    assert_eq!(running["requests_total"], 0);

    let request = parse_jsonl_request(r#"{"method":"snapshot","params":{}}"#).unwrap();
    metrics.record_request(&request);
    metrics.record_output(&json!({"type": "snapshot"}), 24);
    metrics.checkpoint().unwrap();
    let updated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(updated["run_status"], "running");
    assert_eq!(updated["requests_total"], 1);
    assert_eq!(updated["standalone_snapshot_requests_total"], 1);
    assert_eq!(updated["action_kinds"], json!({}));
    assert_eq!(updated["error_codes"], json!({}));

    metrics.finish(true).unwrap();
    let finished: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(finished["run_status"], "succeeded");
    assert_eq!(finished["transport_success"], true);
}

#[rstest]
fn updater_selects_the_exact_archive_instead_of_its_checksum() {
    let target = self_update::get_target();
    let archive = update::release_archive_name("0.1.0", target);
    let releases = [self_update::update::Release {
        version: "0.1.0".into(),
        assets: vec![
            self_update::update::ReleaseAsset {
                name: format!("{archive}.sha256"),
                download_url: "checksum".into(),
            },
            self_update::update::ReleaseAsset {
                name: archive.clone(),
                download_url: "archive".into(),
            },
        ],
        ..Default::default()
    }];
    let (_, selected) = update::latest_release_asset(&releases, target).unwrap();
    assert_eq!(selected.name, archive);
    assert_eq!(selected.download_url, "archive");
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

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}
