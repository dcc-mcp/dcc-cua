use rstest::rstest;
use serde_json::json;

use super::authorization::{existing_profile_grant_requested, host_private_worker_options};
use super::*;

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
            "schema_version": 1,
            "id": "custom-maya",
            "display_name": "Custom Maya",
            "selectors": [{"application_names": ["maya.exe"]}],
            "surfaces": [],
            "settings": {"dialog_style": "os_native", "preferred_route": "accessibility"}
        }"#,
    )
    .unwrap();
    let path = path.to_string_lossy().into_owned();
    let profile = load_semantic_profile(&strings(["--profile-file", &path])).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(profile.id, "custom-maya");
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
