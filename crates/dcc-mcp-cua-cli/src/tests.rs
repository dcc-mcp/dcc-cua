use rstest::rstest;

use super::*;

#[rstest]
fn response_image_reads_host_owned_shared_memory() {
    let image = dcc_mcp_cua_shm::SharedImage::from_bytes(b"png", "image/png").unwrap();
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
fn parallel_discovery_is_limited_to_stateless_methods() {
    assert!(is_parallel_discovery_method("list_apps"));
    assert!(is_parallel_discovery_method("cursor_position"));
    assert!(!is_parallel_discovery_method("snapshot"));
    assert!(!is_parallel_discovery_method("desktop_snapshot"));
}

#[rstest]
fn daemon_reuses_the_official_serve_command() {
    assert_eq!(upstream_command("daemon"), "serve");
    assert_eq!(upstream_command("mcp"), "mcp");
    assert_eq!(upstream_command("recording"), "recording");
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
fn list_window_filters_select_one_exact_window() {
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
    filter_window_rows(
        &mut windows,
        Some("ue5editor.exe"),
        Some(7),
        Some("PCG Fab"),
    )
    .unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["window_id"], 7);
    assert!(filter_window_rows(&mut windows, Some(""), None, None).is_err());
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

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}
