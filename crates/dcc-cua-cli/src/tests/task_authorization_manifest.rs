use rstest::rstest;

#[rstest]
fn manifest_advertises_the_split_capability_task_authorization_broker() {
    let authorization = &crate::manifest::document()["host"]["task_authorization"];

    assert_eq!(authorization["mode"], "split_constructor_capability_broker");
    assert_eq!(authorization["registration_single_use"], true);
    assert_eq!(authorization["max_ttl_ms"], 86_400_000);
    for field in [
        "ipc_can_mint_or_widen",
        "cli_arguments_can_authorize",
        "environment_can_authorize",
        "stdin_can_authorize",
    ] {
        assert_eq!(authorization[field], false);
    }
}
