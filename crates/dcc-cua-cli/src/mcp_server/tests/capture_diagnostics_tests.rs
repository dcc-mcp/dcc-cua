use rstest::rstest;
use serde_json::json;

use super::*;

#[rstest]
#[tokio::test]
async fn capture_diagnostics_observation_only_stop_and_expiry_remain_closed() {
    let mut server = test_server();
    let mut task = pixels_task();
    task["allowed_methods"] = json!(["snapshot"]);
    task["allowed_actions"] = json!([]);
    let prepared = server.prepare_task(task.clone()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server.revoke_task(json!({"task_id": id})).await.unwrap();
    let error = server
        .task_call(
            json!({"task_id":id, "method":"snapshot", "params":{"capture_diagnostics":true}}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("stopped"), "{error}");
    let prepared = server.prepare_task(task).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    server
        .proposals
        .get_mut(id)
        .unwrap()
        .registration
        .expires_at_unix_ms = unix_time_millis().saturating_sub(1);
    let error = server
        .task_call(
            json!({"task_id":id, "method":"snapshot", "params":{"capture_diagnostics":true}}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("expired"), "{error}");
}
