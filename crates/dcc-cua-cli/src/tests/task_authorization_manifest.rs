use rstest::rstest;
use serde_json::json;

#[rstest]
fn manifest_keeps_the_internal_task_lease_broker_private() {
    let authorization = &crate::manifest::document()["host"]["task_authorization"];

    assert_eq!(authorization["mode"], "split_constructor_capability_broker");
    assert_eq!(authorization["authorization_owner"], "connected_agent_host");
    assert_eq!(authorization["ipc_can_mint_exact_retained_proposal"], true);
    assert_eq!(authorization["ipc_can_widen"], false);
    assert_eq!(authorization["registration_single_use"], true);
    assert_eq!(authorization["max_ttl_ms"], 86_400_000);
    assert_eq!(
        authorization["targets"],
        json!(["exact_window", "owned_browser"])
    );
    assert_eq!(
        authorization["owned_browser"]["client_can_nominate_identity"],
        false
    );
    assert_eq!(
        authorization["owned_browser"]["repeat_browser_prepare"],
        false
    );
    assert_eq!(
        authorization["owned_browser"]["hidden_file_input_method"],
        "browser_set_input_files"
    );
    for field in [
        "cli_arguments_can_authorize",
        "environment_can_authorize",
        "stdin_can_authorize",
    ] {
        assert_eq!(authorization[field], false);
    }
    assert!(authorization.get("cli_integration").is_none());
}
