use rstest::rstest;
use serde_json::json;

use super::*;

#[rstest]
#[tokio::test]
async fn task_ids_are_connection_private_and_errors_do_not_disclose_the_owner() {
    let mut owner = test_server();
    let prepared = owner.prepare_task(pixels_task()).unwrap();
    let task_id = prepared["task_id"].as_str().unwrap();
    assert_eq!(
        owner.task_status(json!({"task_id": task_id})).unwrap()["status"],
        "ready"
    );
    let mut other = test_server();
    for tool in ["task_status", "stop_task", "dcc_cua_task_call"] {
        let result = other
            .call_tool(json!({
                "name": tool,
                "arguments": {"task_id": task_id, "method": "snapshot", "params": {}}
            }))
            .await
            .unwrap();
        let payload = &result["structuredContent"];
        assert_eq!(result["isError"], true);
        assert_eq!(payload["code"], "task_not_found");
        assert_eq!(payload["error_code"], "task_not_found");
        assert_eq!(payload["details"]["cross_connection_resume"], false);
        assert!(
            payload["message"]
                .as_str()
                .unwrap()
                .contains("same mcp-server stdio connection")
        );
        assert!(!result.to_string().contains(task_id));
        assert!(other.proposals.is_empty());
        assert_eq!(
            owner.task_status(json!({"task_id": task_id})).unwrap()["status"],
            "ready"
        );
    }
    owner
        .revoke_task(json!({"task_id": task_id}))
        .await
        .unwrap();
    let stopped = owner
        .call_tool(json!({"name":"dcc_cua_task_call", "arguments":{
            "task_id":task_id, "method":"snapshot", "params":{}
        }}))
        .await
        .unwrap();
    assert_eq!(stopped["structuredContent"]["code"], "task_stopped");
}

#[rstest]
#[tokio::test]
async fn expired_task_guidance_does_not_renew_its_lease() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let task_id = prepared["task_id"].as_str().unwrap();
    server
        .proposals
        .get_mut(task_id)
        .unwrap()
        .registration
        .expires_at_unix_ms = 0;
    let result = server
        .call_tool(json!({"name":"dcc_cua_task_call", "arguments":{
            "task_id":task_id, "method":"snapshot", "params":{}
        }}))
        .await
        .unwrap();
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["code"], "task_expired");
    assert_eq!(server.proposals[task_id].registration.expires_at_unix_ms, 0);
    assert!(server.proposals[task_id].session.is_none());
    server
        .revoke_task(json!({"task_id":task_id}))
        .await
        .unwrap();
}

#[rstest]
fn lifetime_guidance_is_discoverable_before_starting_a_task() {
    let tools = tool_definitions();
    for name in ["start_task", "task_status", "dcc_cua_task_call"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert!(tool["description"].as_str().unwrap().contains("connection"));
    }
    let ordinary = tool_error("PRIVATE_UNCLASSIFIED_ERROR".into());
    assert!(ordinary["structuredContent"].get("code").is_none());
}
