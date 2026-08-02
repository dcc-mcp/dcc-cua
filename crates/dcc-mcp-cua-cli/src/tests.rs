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
fn friendly_actions_build_bounded_cua_requests() {
    let click = action_from_command("click", &strings(["--x", "10", "--y", "20"])).unwrap();
    assert_eq!(click.action, "click");
    assert_eq!(click.x, Some(10.0));
    assert_eq!(click.y, Some(20.0));

    let type_chars = action_from_command(
        "type",
        &strings(["--text", "hello", "--focused", "--delay-ms", "25"]),
    )
    .unwrap();
    assert_eq!(type_chars.action, "type_chars");
    assert!(type_chars.type_chars_only);
    assert_eq!(type_chars.delay_ms, Some(25));
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
