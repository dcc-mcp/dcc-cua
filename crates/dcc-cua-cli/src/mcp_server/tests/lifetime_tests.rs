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
    let guidance = result["structuredContent"]["message"].as_str().unwrap();
    assert!(guidance.contains("stop_task with this task ID"));
    assert!(guidance.contains("clean up its session, then start a new task"));
    assert_eq!(server.proposals[task_id].registration.expires_at_unix_ms, 0);
    assert!(server.proposals[task_id].session.is_none());
    let stopped = server
        .call_tool(json!({"name":"stop_task", "arguments":{"task_id":task_id}}))
        .await
        .unwrap();
    assert_eq!(stopped["structuredContent"]["status"], "stopped");
    assert!(server.proposals[task_id].revoked);
    assert_eq!(server.proposals[task_id].registration.expires_at_unix_ms, 0);
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

#[rstest]
#[tokio::test]
async fn host_completion_does_not_replace_a_missing_session_cleanup_acknowledgement() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get_mut(id).unwrap();
    proposal.session_open_attempted = true;
    proposal.host_task = Some(OwnedHostTask::spawn(async { Ok(()) }));
    let stopped = server.revoke_task(json!({"task_id":id})).await.unwrap();
    assert_eq!(stopped["status"], "cleanup_unknown");
    assert_eq!(stopped["cleanup"]["host_connection_completion"]["success"], true);
    assert_eq!(
        server.revoke_task(json!({"task_id":id})).await.unwrap(),
        stopped
    );
}

#[rstest]
#[tokio::test]
async fn host_failure_overrides_a_successful_session_cleanup_acknowledgement() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get_mut(id).unwrap();
    proposal.revoked = true;
    proposal.cleanup = Some(json!({"success":true,"active":false,"cleanup_pending":false}));
    proposal.host_task = Some(OwnedHostTask::spawn(async {
        Err("Host finalization failed".into())
    }));
    let stopped = server.revoke_task(json!({"task_id":id})).await.unwrap();
    assert_eq!(stopped["status"], "cleanup_failed");
    assert_eq!(stopped["cleanup"]["success"], false);
    assert_eq!(stopped["cleanup"]["session_cleanup"]["success"], true);
    assert_eq!(stopped["cleanup"]["host_connection_completion"]["success"], false);
}

#[rstest]
fn pending_host_completion_cannot_claim_a_stopped_task() {
    let cleanup = json!({"success":true,"active":false,"cleanup_pending":false,
        "host_connection_completion":{"completed":false,"success":false}});
    assert_eq!(stopped_task_status(Some(&cleanup)), "cleanup_unknown");
    let payload = task_cleanup_payload(Some(&cleanup));
    assert_eq!(payload["success"], false);
    assert_eq!(payload["cleanup_pending"], true);
    assert_eq!(payload["session_cleanup"]["success"], true);
}

#[rstest]
#[tokio::test]
async fn cancelling_completion_wait_keeps_pending_evidence_and_can_resume() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get_mut(id).unwrap();
    let (release, wait) = tokio::sync::oneshot::channel();
    proposal.revoked = true;
    proposal.cleanup = Some(json!({"success":true,"active":false,"cleanup_pending":false}));
    proposal.host_task = Some(OwnedHostTask::spawn(async move {
        wait.await.unwrap();
        Ok(())
    }));
    let mut finish = Box::pin(server.revoke_task(json!({"task_id":id})));
    let waker = std::task::Waker::noop();
    assert!(matches!(
        std::future::Future::poll(finish.as_mut(), &mut std::task::Context::from_waker(waker)),
        std::task::Poll::Pending
    ));
    drop(finish);
    assert_eq!(
        server.task_status(json!({"task_id":id})).unwrap()["status"],
        "cleanup_unknown"
    );
    release.send(()).unwrap();
    let stopped = server.revoke_task(json!({"task_id":id})).await.unwrap();
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["cleanup"]["success"], true);
}

#[rstest]
#[tokio::test]
async fn interrupted_startup_requires_a_new_task_without_replacing_its_host_owner() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get_mut(id).unwrap();
    proposal.session_open_attempted = true;
    proposal.host_task = Some(OwnedHostTask::spawn(async { Ok(()) }));
    let status = server.task_status(json!({"task_id":id})).unwrap();
    assert_eq!(status["status"], "cleanup_unknown");
    assert_eq!(status["ok"], false);
    let error = server.start_task(json!({"task_id":id})).await.unwrap_err();
    assert!(error.contains("call stop_task, then start a new task"));
    assert!(server.proposals[id].host_task.is_some());
    let stopped = server.revoke_task(json!({"task_id":id})).await.unwrap();
    assert_eq!(
        stopped["cleanup"]["host_connection_completion"]["completed"],
        true
    );
    assert!(server.proposals[id].revoked);
}

#[rstest]
#[tokio::test]
async fn shutdown_waits_for_retained_host_completion() {
    let mut server = test_server();
    let prepared = server.prepare_task(pixels_task()).unwrap();
    let id = prepared["task_id"].as_str().unwrap();
    let proposal = server.proposals.get_mut(id).unwrap();
    let (release, wait) = tokio::sync::oneshot::channel();
    proposal.revoked = true;
    proposal.cleanup = Some(json!({"success":true,"active":false,"cleanup_pending":false}));
    proposal.host_task = Some(OwnedHostTask::spawn(async move {
        wait.await.unwrap();
        Ok(())
    }));
    let mut shutdown = Box::pin(server.shutdown_tasks());
    let waker = std::task::Waker::noop();
    assert!(matches!(
        std::future::Future::poll(shutdown.as_mut(), &mut std::task::Context::from_waker(waker)),
        std::task::Poll::Pending
    ));
    release.send(()).unwrap();
    assert!(shutdown.await.is_empty());
    assert_eq!(
        server.task_status(json!({"task_id":id})).unwrap()["status"],
        "stopped"
    );
}

#[rstest]
#[tokio::test]
async fn stopping_one_task_does_not_revoke_or_join_another_host() {
    let mut server = test_server();
    let first = server.prepare_task(pixels_task()).unwrap();
    let second = server.prepare_task(pixels_task()).unwrap();
    let first_id = first["task_id"].as_str().unwrap();
    let second_id = second["task_id"].as_str().unwrap();
    let (release, wait) = tokio::sync::oneshot::channel();
    server.proposals.get_mut(first_id).unwrap().host_task =
        Some(OwnedHostTask::spawn(async { Ok(()) }));
    server.proposals.get_mut(second_id).unwrap().host_task =
        Some(OwnedHostTask::spawn(async move {
            wait.await.unwrap();
            Ok(())
        }));
    let stopped = server.revoke_task(json!({"task_id":first_id})).await.unwrap();
    assert_eq!(
        stopped["cleanup"]["host_connection_completion"]["completed"],
        true
    );
    assert!(!server.proposals[second_id].revoked);
    assert!(server.proposals[second_id].cleanup.is_none());
    release.send(()).unwrap();
    server
        .revoke_task(json!({"task_id":second_id}))
        .await
        .unwrap();
}
