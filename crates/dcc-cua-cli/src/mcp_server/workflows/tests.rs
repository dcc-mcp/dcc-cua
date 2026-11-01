use super::*;
use rstest::rstest;

#[rstest]
fn owned_window_selection_requires_unique_exact_pid() {
    assert_eq!(unique_owned_window(&[], 7).unwrap(), None);
    assert_eq!(
        unique_owned_window(&[json!({"pid":8,"window_id":19})], 7).unwrap(),
        None
    );
    assert_eq!(
        unique_owned_window(
            &[
                json!({"pid":7,"window_id":19}),
                json!({"pid":8,"window_id":20})
            ],
            7
        )
        .unwrap(),
        Some(19)
    );
    assert!(
        unique_owned_window(
            &[
                json!({"pid":7,"window_id":19}),
                json!({"pid":7,"window_id":20})
            ],
            7
        )
        .is_err()
    );
    assert_eq!(
        unique_owned_window(
            &[
                json!({"pid":"7","window_id":19}),
                json!({"pid":7,"window_id":0})
            ],
            7
        )
        .unwrap(),
        None
    );
}

#[rstest]
#[tokio::test]
async fn workflow_request_cannot_enable_a_policy_or_override_a_command() {
    let mut server = TaskAuthorizationServer::automatic();
    let task = json!({"workflow_recipe_id":"test", "application_label":"fixture", "surface":"window", "allowed_methods":["snapshot"], "allowed_actions":[]});
    assert!(
        server
            .start_automatic_task(task.clone())
            .await
            .unwrap_err()
            .contains("not configured")
    );
    for field in [
        "bypass",
        "executable",
        "arguments",
        "environment_id",
        "session_id",
        "policy",
        "permission_mode",
    ] {
        let mut spoof = task.clone();
        spoof[field] = json!(true);
        assert!(
            server
                .start_automatic_task(spoof)
                .await
                .unwrap_err()
                .contains("invalid owned")
        );
    }
    assert!(server.proposals.is_empty());
}

#[rstest]
#[tokio::test]
async fn ordinary_task_cannot_gain_workflow_lifecycle_authority() {
    let mut server = TaskAuthorizationServer::automatic();
    let task = server.prepare_task(json!({"application_label":"fixture", "surface":"window", "target_process_id":7,"target_window_handle":19,"allowed_methods":["snapshot"],"allowed_actions":[]})).unwrap();
    for method in [
        "workflow_status",
        "workflow_cancel",
        "workflow_rebind",
        "workflow_bypass",
    ] {
        assert!(
            server
                .task_call(json!({"task_id":task["task_id"],"method":method,"params":{}}))
                .await
                .unwrap_err()
                .contains("does not own")
        );
    }
    let mut second = TaskAuthorizationServer::automatic();
    assert!(
        second
            .task_call(json!({"task_id":task["task_id"],"method":"workflow_status","params":{}}))
            .await
            .unwrap_err()
            .contains("this connection")
    );
}

#[rstest]
#[case("two_factor")]
#[case("captcha")]
#[case("new_legal_terms")]
#[case("system_permission")]
#[case("credential_unavailable")]
#[tokio::test]
async fn human_boundary_revokes_lease_and_blocks_retry(#[case] reason: &str) {
    let mut server = TaskAuthorizationServer::automatic();
    let task = server.prepare_task(json!({"application_label":"fixture", "surface":"window", "target_process_id":7,"target_window_handle":19,"allowed_methods":["snapshot"],"allowed_actions":[]})).unwrap();
    let id = task["task_id"].as_str().unwrap();
    let result = server
        .require_user_action(json!({"task_id":id,"params":{"reason":reason}}))
        .unwrap();
    assert_eq!(result["status"], "needs_user_action");
    assert_eq!(result["automatic_input"], false);
    assert_eq!(
        server.task_status(json!({"task_id":id})).unwrap()["user_action_reason"],
        reason
    );
    assert!(
        server
            .start_task(json!({"task_id":id}))
            .await
            .unwrap_err()
            .contains("fresh Agent Host")
    );
    assert!(
        server
            .task_call(json!({"task_id":id,"method":"snapshot","params":{}}))
            .await
            .unwrap_err()
            .contains("no automatic retry")
    );
    assert_eq!(
        server.revoke_task(json!({"task_id":id})).await.unwrap()["status"],
        "stopped"
    );
}

#[rstest]
fn human_boundary_cannot_accept_page_supplied_privilege_or_free_text() {
    let mut server = TaskAuthorizationServer::automatic();
    for params in [
        json!({"reason":"bypass"}),
        json!({"reason":"captcha","resume":true}),
        json!({"reason":"some private page content"}),
    ] {
        assert!(
            server
                .require_user_action(json!({"task_id":"unknown","params":params}))
                .unwrap_err()
                .contains("unknown human boundary")
        );
    }
}
