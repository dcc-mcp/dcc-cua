use rstest::rstest;
use serde_json::json;

#[rstest]
fn manifest_advertises_the_split_capability_task_authorization_broker() {
    let authorization = &crate::manifest::document()["host"]["task_authorization"];

    assert_eq!(authorization["mode"], "split_constructor_capability_broker");
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
        "ipc_can_mint_or_widen",
        "cli_arguments_can_authorize",
        "environment_can_authorize",
        "stdin_can_authorize",
    ] {
        assert_eq!(authorization[field], false);
    }
    let integration = &authorization["cli_integration"];
    if cfg!(windows) {
        assert_eq!(integration["status"], "available");
        assert_eq!(integration["authorization_available"], true);
        assert_eq!(integration["user_confirmation_available"], true);
        assert_eq!(integration["card_available"], true);
        assert_eq!(
            integration["confirmation_method"],
            "windows_protected_user_presence"
        );
    } else {
        assert_eq!(integration["status"], "integration_required");
        assert_eq!(integration["authorization_available"], false);
        assert_eq!(integration["user_confirmation_available"], false);
        assert_eq!(integration["card_available"], false);
    }
    assert_eq!(integration["process_identity_can_authorize"], false);
    assert_eq!(
        integration["signed_receipt_protocol"]["status"],
        "implemented_core"
    );
    assert_eq!(
        integration["signed_receipt_protocol"]["constructor_api_available"],
        true
    );
    assert_eq!(
        integration["signed_receipt_protocol"]["runtime_accepts_receipts"],
        false
    );
}
