//! Pure MCP permission/schema tests. Never start a Host or native fixture.
use super::*;

fn task() -> Value {
    json!({"application_label":"Passive capture", "surface":"window", "observation_mode":"pixels_only",
        "target_process_id":42, "target_window_handle":77, "allow_capture_preparation":true,
        "allowed_methods":["get_window_state","capture_preparation_begin","capture_preparation_state",
            "capture_preparation_stop","capture_preparation_snapshot"],
        "allowed_actions":[{"action":"capture_preparation_begin","input_kind":"window_state",
            "secret_input":false,"authorization_category":"window_state"}]})
}

fn server(root: Option<std::path::PathBuf>) -> TaskAuthorizationServer {
    let mut server = TaskAuthorizationServer::automatic();
    server.recording_output_root = None;
    server.capture_preparation_journal_root = root;
    server
}

fn ordinary_root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn ordinary_path(root: &tempfile::TempDir) -> std::path::PathBuf {
    let path = root.path().to_owned();
    #[cfg(not(windows))]
    let path = path.canonicalize().unwrap();
    path
}

#[test]
fn capture_preparation_requires_explicit_permission_exact_pixels_scope_and_stable_operator_root() {
    assert!(
        server(None)
            .prepare_task(task())
            .unwrap_err()
            .contains("operator configuration")
    );
    let root = ordinary_root();
    let path = ordinary_path(&root);
    let mut server = server(Some(path.clone()));
    for (field, value) in [
        ("allow_capture_preparation", json!(false)),
        ("observation_mode", json!("semantic")),
        ("surface", json!("browser")),
        ("allowed_actions", json!([])),
        (
            "allowed_methods",
            json!([
                "get_window_state",
                "capture_preparation_begin",
                "capture_preparation_state"
            ]),
        ),
        ("journal_directory", json!("caller-root")),
        ("capture_preparation_journal_root", json!("caller-root")),
    ] {
        let mut invalid = task();
        invalid[field] = value;
        assert!(server.prepare_task(invalid).is_err(), "{field}");
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    for _ in 0..2 {
        let prepared = server.prepare_task(task()).unwrap();
        let proposal = &server.proposals[prepared["task_id"].as_str().unwrap()];
        let authorization = proposal.registration.capture_preparation.as_ref().unwrap();
        assert_eq!(authorization.journal_directory, path.to_str().unwrap());
        assert_eq!(authorization.max_lifetime_ms, 30_000);
        let grant = task_session_grant(proposal, proposal.receipt.as_ref().unwrap());
        assert_eq!(grant["allow_capture_preparation"], true);
        assert_eq!(grant["allow_raw_input"], false);
        assert_eq!(grant["allow_recording"], false);
        assert!(grant.get("capture_preparation").is_none());
        assert!(grant.get("journal_directory").is_none());
    }
    // Native code, not a fresh task directory, owns the durable epoch/index.
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    server.recording_output_root = Some(path);
    assert!(
        server
            .prepare_task(task())
            .unwrap_err()
            .contains("separate from recording")
    );
}

#[test]
fn capture_preparation_call_params_are_closed_and_never_nominate_a_journal_or_target() {
    let valid = json!({"request":{"window_state_id":"fresh-state", "lifetime_ms":1_000}});
    validate_task_method_params("capture_preparation_begin", &valid).unwrap();
    for params in [
        json!({}),
        json!({"request":{"window_state_id":"s","lifetime_ms":0}}),
        json!({"request":{"window_state_id":"s","lifetime_ms":30_001}}),
        json!({"request":{"window_state_id":"s","lifetime_ms":true}}),
        json!({"request":{"window_state_id":"s","lifetime_ms":1,"journal_directory":"caller"}}),
        json!({"request":{"window_state_id":"s","lifetime_ms":1},"target":77}),
    ] {
        assert!(
            validate_task_method_params("capture_preparation_begin", &params).is_err(),
            "{params}"
        );
    }
    for method in [
        "capture_preparation_state",
        "capture_preparation_stop",
        "capture_preparation_snapshot",
    ] {
        validate_task_method_params(method, &json!({})).unwrap();
        for field in [
            "journal_directory",
            "target",
            "window_state_id",
            "observation_id",
            "request",
        ] {
            let mut invalid = json!({});
            invalid[field] = json!("caller");
            assert!(
                validate_task_method_params(method, &invalid).is_err(),
                "{method}/{field}"
            );
        }
    }
}

#[test]
fn capture_preparation_schema_and_method_allowlists_retain_exact_lifecycle_boundaries() {
    for method in [
        "capture_preparation_begin",
        "capture_preparation_state",
        "capture_preparation_stop",
        "capture_preparation_snapshot",
    ] {
        assert!(method_allowed(TaskSurface::Window, method));
        assert!(!method_allowed(TaskSurface::Browser, method));
        assert!(TaskObservationMode::PixelsOnly.permits_method(method));
    }
    let methods = [
        "get_window_state",
        "capture_preparation_begin",
        "capture_preparation_state",
        "capture_preparation_stop",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    validate_native_lifecycle_methods(&methods).unwrap();
    for removed in &methods {
        let partial = methods
            .iter()
            .filter(|method| *method != removed)
            .cloned()
            .collect::<Vec<_>>();
        assert!(validate_native_lifecycle_methods(&partial).is_err());
    }
    let definitions = tool_definitions();
    let start = definitions
        .iter()
        .find(|item| item["name"] == "start_task")
        .unwrap();
    let schema = &start["inputSchema"];
    assert_eq!(
        schema["properties"]["allow_capture_preparation"]["default"],
        false
    );
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"].get("journal_directory").is_none());
    assert_eq!(
        capture_preparation_scope_schema()["properties"]["secret_input"]["const"],
        false
    );
    assert!(schema.to_string().contains("capture_preparation_snapshot"));
    let call = definitions
        .iter()
        .find(|item| item["name"] == "dcc_cua_task_call")
        .unwrap();
    assert!(
        call["inputSchema"]["allOf"]
            .to_string()
            .contains("lifetime_ms")
    );
}
