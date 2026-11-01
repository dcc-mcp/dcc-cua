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

fn owned_preflight_server(
    gui: bool,
) -> (
    tempfile::TempDir,
    TaskAuthorizationServer,
    std::path::PathBuf,
) {
    use sha2::{Digest, Sha256};
    let executable = std::env::current_exe().unwrap();
    let directory = tempfile::tempdir_in(executable.parent().unwrap()).unwrap();
    let root = directory.path().join("install");
    std::fs::create_dir(&root).unwrap();
    let input = json!({"version":1,"environment_id":"preflight-env","session_id":"preflight-host","expires_at_unix_ms":unix_time_millis()+60_000,
        "recipes":[{"id":"fixture","kind":"install","executable":executable,"executable_sha256":format!("{:x}",Sha256::digest(std::fs::read(&executable).unwrap())),
        "arguments":["--ignored","--exact","workflow::tests::synthetic_owned_process","--test-threads=1"],"working_directory":root,"artifact_root":root,
        "result":{"relative_path":"result.txt","present":true},"allowed_exit_codes":[0],"timeout_ms":1000,"allow_cancel":true,"gui":gui,
        "allowed_methods":["snapshot","execute_action"],"allowed_actions":[{"action":"click","input_kind":"semantic","secret_input":false,"authorization_category":"task_grant"}]}]});
    let file = root.join("policy.json");
    std::fs::write(&file, serde_json::to_vec(&input).unwrap()).unwrap();
    let mut server = TaskAuthorizationServer::automatic();
    server.workflows =
        WorkflowManager::from_policy_file(&file, "preflight-env", "preflight-host").unwrap();
    (directory, server, root)
}

fn owned_preflight_request() -> Value {
    json!({"workflow_recipe_id":"fixture","application_label":"preflight fixture","surface":"window","allowed_methods":["snapshot","execute_action"],
        "allowed_actions":[{"action":"click","input_kind":"semantic","secret_input":false,"authorization_category":"task_grant"}]})
}

#[rstest]
#[case("pixels_only")]
#[case("duplicate_action")]
#[case("label_whitespace")]
#[tokio::test]
async fn invalid_owned_contract_is_refused_before_any_executable_start(#[case] invalid: &str) {
    let (_directory, mut server, root) = owned_preflight_server(true);
    let mut request = owned_preflight_request();
    let expected = match invalid {
        "pixels_only" => {
            request["observation_mode"] = json!("pixels_only");
            "pixels_only"
        }
        "duplicate_action" => {
            let action = request["allowed_actions"][0].clone();
            request["allowed_actions"]
                .as_array_mut()
                .unwrap()
                .push(action);
            "unique"
        }
        _ => {
            request["application_label"] = json!(" fixture ");
            "application_label"
        }
    };
    let error = server.start_automatic_task(request).await.unwrap_err();
    assert!(
        error.contains(expected),
        "unexpected preflight refusal: {error}"
    );
    assert!(!root.join("result.txt").exists());
    assert!(server.proposals.is_empty());
}

#[rstest]
#[tokio::test]
async fn full_task_capacity_and_non_gui_recipes_are_refused_before_launch() {
    let (_directory, mut server, root) = owned_preflight_server(false);
    assert!(
        server
            .start_automatic_task(owned_preflight_request())
            .await
            .unwrap_err()
            .contains("GUI recipe")
    );
    assert!(!root.join("result.txt").exists());
    let (_directory2, mut full, root2) = owned_preflight_server(true);
    for _ in 0..MAX_PENDING_TASKS {
        full.prepare_task(json!({"application_label":"read fixture","surface":"window","target_process_id":7,"target_window_handle":19,"allowed_methods":["snapshot"],"allowed_actions":[]})).unwrap();
    }
    assert_eq!(
        full.start_automatic_task(owned_preflight_request())
            .await
            .unwrap_err(),
        "too many live tasks"
    );
    assert!(!root2.join("result.txt").exists());
}

#[rstest]
#[tokio::test]
async fn capacity_reclaims_expired_completed_owners_but_retains_pending_cleanup() {
    let mut server = TaskAuthorizationServer::automatic();
    let mut senders = Vec::new();
    for _ in 0..MAX_PENDING_TASKS {
        let prepared = server.prepare_task(json!({"application_label":"read fixture","surface":"window","target_process_id":7,"target_window_handle":19,"allowed_methods":["snapshot"],"allowed_actions":[]})).unwrap();
        let id = prepared["task_id"].as_str().unwrap();
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        server.proposals.get_mut(id).unwrap().host_task = Some(OwnedHostTask::spawn(async move {
            let _ = receiver.await;
            Ok(())
        }));
        senders.push(sender);
    }
    for proposal in server.proposals.values_mut() {
        proposal.registration.expires_at_unix_ms = 0;
    }
    assert_eq!(
        server.ensure_task_capacity().unwrap_err(),
        "too many live tasks"
    );
    assert_eq!(server.proposals.len(), MAX_PENDING_TASKS);
    for sender in senders {
        sender.send(()).unwrap();
    }
    for proposal in server.proposals.values_mut() {
        assert_eq!(
            proposal
                .host_task
                .as_mut()
                .unwrap()
                .finish(std::time::Duration::from_secs(1))
                .await["completed"],
            true
        );
    }
    server.ensure_task_capacity().unwrap();
    assert!(server.proposals.is_empty());
}
