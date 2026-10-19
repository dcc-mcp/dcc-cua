use super::*;
use rstest::rstest;

fn extension_session(
    tool: &str,
    binding_fields: &[&str],
) -> (ComputerUseSession, Arc<Mutex<Vec<String>>>) {
    let (mut session, _, names) = counting_session_with_names();
    session.last_upstream_session_refresh = Some(Instant::now());
    session.live_observation = None;
    let mut properties = serde_json::Map::new();
    for field in binding_fields {
        properties.insert((*field).into(), json!({}));
    }
    session.driver.tool_inventory = Arc::new(tokio::sync::OnceCell::new_with(Some(json!({
        "tools": [{
            "name": tool,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "additionalProperties": false
            }
        }]
    }))));
    (session, names)
}

#[rstest]
#[case("run_script", &["session", "script"], json!({"script": "return 1;"}))]
#[case("run_steps", &["pid", "window_id", "session", "steps"], json!({"steps": []}))]
#[tokio::test]
async fn window_extension_refuses_composite_actions_before_any_driver_call(
    #[case] tool: &str,
    #[case] fields: &[&str],
    #[case] arguments: Value,
) {
    let (mut session, names) = extension_session(tool, fields);
    let observation_id = session.observation.as_ref().unwrap().observation_id.clone();

    let error = session.call_tool(tool, arguments).await.unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert!(names.lock().unwrap().is_empty());
    assert_eq!(
        session.observation.as_ref().unwrap().observation_id,
        observation_id
    );
}

#[rstest]
#[case(&[])]
#[case(&["session"])]
#[tokio::test]
async fn window_extension_requires_target_fields_beyond_session_metadata(#[case] fields: &[&str]) {
    let (mut session, names) = extension_session("debug_window_info", fields);

    let error = session
        .call_tool("debug_window_info", json!({}))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
    assert!(
        !names
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "debug_window_info")
    );
}

#[rstest]
#[case(&["pid"])]
#[case(&["window_id"])]
#[case(&["pid", "window_id"])]
#[case(&["pid", "session"])]
#[case(&["window_id", "session"])]
#[case(&["pid", "window_id", "session"])]
#[tokio::test]
async fn window_extension_preserves_target_binding_and_session_forwarding(#[case] fields: &[&str]) {
    let (mut session, names) = extension_session("debug_window_info", fields);
    let mut arguments = serde_json::Map::new();
    for field in fields {
        arguments.insert((*field).into(), json!("caller-supplied-other-target"));
    }

    let result = session
        .call_tool("debug_window_info", Value::Object(arguments))
        .await
        .unwrap();

    let forwarded = &result.value["structuredContent"];
    for (field, expected) in [
        ("pid", json!(42)),
        ("window_id", json!(77)),
        ("session", json!(session.session_id)),
    ] {
        if fields.contains(&field) {
            assert_eq!(forwarded[field], expected);
        } else {
            assert!(forwarded.get(field).is_none());
        }
    }
    assert!(
        names
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "debug_window_info")
    );
}
