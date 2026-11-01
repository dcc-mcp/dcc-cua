use super::*;
use rstest::rstest;
use tempfile::TempDir;

fn policy(root: &Path, kind: &str, timeout: u64, allow_cancel: bool) -> Value {
    let executable = std::env::current_exe().expect("test executable");
    let mut file = File::open(&executable).unwrap();
    json!({
        "version":1,"environment_id":"synthetic-windows","session_id":"fixture-session",
        "expires_at_unix_ms":unix_ms()+60_000,
        "recipes":[{
            "id":"fixture","kind":kind,"executable":executable,
            "executable_sha256":hash_reader(&mut file).unwrap(),
            "arguments":["--ignored","--exact","workflow::tests::synthetic_owned_process","--test-threads=1"],
            "working_directory":root,"artifact_root":root,
            "result":{"relative_path":"result.txt","present":kind=="install"},
            "allowed_exit_codes":[0],"timeout_ms":timeout,"allow_cancel":allow_cancel,"gui":false,
            "allowed_methods":["snapshot","execute_action"],
            "allowed_actions":[{"action":"click","input_kind":"semantic","secret_input":false,"authorization_category":"content_change"}]
        }]
    })
}

fn fixture_root(mode: &str) -> (TempDir, PathBuf) {
    // Keep the entire ancestor chain in the task workspace; sandboxed user-home
    // ancestors may intentionally deny native directory attribute handles.
    let test_executable = std::env::current_exe().unwrap();
    let directory = tempfile::tempdir_in(test_executable.parent().unwrap()).unwrap();
    let root = directory.path().join(mode);
    fs::create_dir(&root).unwrap();
    (directory, root)
}

fn manager(root: &Path, policy: &Value) -> Result<WorkflowManager, String> {
    let file = root.join("trusted-policy.json");
    fs::write(&file, serde_json::to_vec(policy).unwrap()).unwrap();
    WorkflowManager::from_policy_file(&file, "synthetic-windows", "fixture-session")
}

#[cfg(windows)]
fn wait_terminal(manager: &mut WorkflowManager, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = manager.status(id).unwrap();
        if state["state"] != "running" && state["state"] != "verifying" {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "fixture deadline exceeded: {state}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// This is a real isolated subprocess fixture, not a GUI or installer E2E claim.
#[rstest]
#[ignore = "invoked only through its exact trusted workflow recipe"]
fn synthetic_owned_process() {
    let directory = std::env::current_dir().unwrap();
    match directory.file_name().unwrap().to_str().unwrap() {
        "install" => fs::write(
            directory.join("result.txt"),
            b"synthetic installed artifact",
        )
        .unwrap(),
        "uninstall" => fs::remove_file(directory.join("result.txt")).unwrap(),
        "exit-fail" => panic!("synthetic process failure"),
        "no-result" => (),
        "hang" => std::thread::sleep(Duration::from_secs(20)),
        _ => panic!("fixture requires a known disposable working directory"),
    }
}

#[rstest]
#[case("bypass", json!(true))]
#[case("capability", json!("self-issued"))]
fn closed_policy_rejects_unknown_fields(#[case] key: &str, #[case] value: Value) {
    let (_directory, root) = fixture_root("install");
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy[key] = value;
    assert!(manager(&root, &value_policy).is_err());
}

#[rstest]
#[case("environment_id", json!("another-machine"))]
#[case("session_id", json!("another-session"))]
#[case("expires_at_unix_ms", json!(1))]
#[case("version", json!(2))]
fn rejects_policy_binding_changes(#[case] key: &str, #[case] value: Value) {
    let (_directory, root) = fixture_root("install");
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy[key] = value;
    assert!(manager(&root, &value_policy).is_err());
}

#[rstest]
#[case("relative_path", json!("../outside"))]
#[case("relative_path", json!("/absolute"))]
#[case("present", json!(false))]
#[case("sha256", json!("not-a-digest"))]
#[case("relative_path", json!("result.txt:stream"))]
#[case("relative_path", json!("NUL"))]
fn rejects_result_scope_changes(#[case] key: &str, #[case] value: Value) {
    let (_directory, root) = fixture_root("install");
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy["recipes"][0]["result"][key] = value;
    assert!(manager(&root, &value_policy).is_err());
}

#[rstest]
fn policy_scope_is_immutable_and_never_self_widened() {
    let (_directory, root) = fixture_root("install");
    let value_policy = policy(&root, "install", 1000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    assert_eq!(host.environment_id(), Some("synthetic-windows"));
    assert_eq!(host.session_id(), Some("fixture-session"));
    assert!(
        host.recipe_authorizes("fixture", &["snapshot".into()], &[])
            .is_ok()
    );
    assert!(
        host.recipe_authorizes("fixture", &["launch_app".into()], &[])
            .is_err()
    );
    let forged = TrustedTaskActionScope {
        action: "type".into(),
        input_kind: "semantic".into(),
        secret_input: true,
        authorization_category: "credential".into(),
        browser_origin: None,
    };
    assert!(host.recipe_authorizes("fixture", &[], &[forged]).is_err());
    assert!(host.start("untrusted").is_err());
    assert!(host.status("workflow-from-another-connection").is_err());
    assert!(host.process_id("workflow-from-another-connection").is_err());
    let file = root.join("trusted-policy.json");
    fs::write(file, b"{}").unwrap();
    assert!(
        host.recipe_authorizes("fixture", &["snapshot".into()], &[])
            .is_ok()
    );
    assert!(host.stop_all()["policy_revoked"].as_bool().unwrap());
    assert!(
        host.recipe_authorizes("fixture", &["snapshot".into()], &[])
            .is_err()
    );
}

#[rstest]
fn method_and_action_ceilings_are_validated_before_any_process_launch() {
    let (_directory, root) = fixture_root("install");
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy["recipes"][0]["allowed_methods"] = json!(["run_script"]);
    assert!(manager(&root, &value_policy).is_err());
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy["recipes"][0]["allowed_actions"][0]["authorization_category"] = json!("bypass");
    assert!(manager(&root, &value_policy).is_err());
    assert!(!root.join("result.txt").exists());
}

#[rstest]
fn monotonic_policy_deadline_prevents_wall_clock_extension() {
    let (_directory, root) = fixture_root("install");
    let value_policy = policy(&root, "install", 1000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    host.policy_deadline = Some(Instant::now());
    assert!(
        host.recipe_authorizes("fixture", &["snapshot".into()], &[])
            .is_err()
    );
}

#[cfg(windows)]
#[rstest]
fn pinned_ancestors_prevent_directory_replacement_and_reparse_writes() {
    use std::os::windows::fs::OpenOptionsExt;
    let (directory, root) = fixture_root("install");
    let stage = directory.path().join("stage");
    fs::create_dir(&stage).unwrap();
    let executable = stage.join("fixture.exe");
    fs::write(&executable, b"disposable non-executed image bytes").unwrap();
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy["recipes"][0]["executable"] = json!(executable);
    value_policy["recipes"][0]["executable_sha256"] =
        json!(hash_reader(&mut File::open(&executable).unwrap()).unwrap());
    let host = manager(&root, &value_policy).unwrap();
    let recipe = host.recipe("fixture").unwrap();
    let directory_guards = pin_workflow_directories(recipe, false).unwrap();
    let executable_guard = pin_executable(recipe).unwrap();
    assert!(fs::rename(&stage, directory.path().join("stage-moved")).is_err());
    assert!(fs::rename(&root, directory.path().join("root-moved")).is_err());
    assert!(
        fs::OpenOptions::new()
            .access_mode(0x4000_0000)
            .share_mode(7)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(&stage)
            .is_err(),
        "reparse changes require a write handle, which the retained guard denies"
    );
    assert!(fs::write(&executable, b"replacement image bytes").is_err());
    fs::write(
        root.join("ordinary-child.txt"),
        b"normal artifact creation remains supported",
    )
    .unwrap();
    drop(executable_guard);
    drop(directory_guards);
    fs::rename(&stage, directory.path().join("stage-moved")).unwrap();
}

#[cfg(windows)]
#[rstest]
fn rejects_network_and_device_path_namespaces() {
    assert!(validate_local_path(Path::new(r"\\server\share\setup.exe")).is_err());
    assert!(validate_local_path(Path::new(r"\\.\GLOBALROOT\Device\setup.exe")).is_err());
}

#[rstest]
#[case("powershell.exe")]
#[case("consent.exe")]
#[case("python3.12.exe")]
#[case("rundll32.exe")]
#[case("setup.cmd")]
fn rejects_interpreter_and_system_entry_points(#[case] path: &str) {
    assert!(restricted_executable(Path::new(path)));
}

#[cfg(windows)]
#[rstest]
fn owned_install_and_normal_uninstall_verify_real_artifacts() {
    let (_directory, install_root) = fixture_root("install");
    let value_policy = policy(&install_root, "install", 5_000, true);
    let mut host = manager(&install_root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    let result = wait_terminal(&mut host, id);
    assert_eq!(result["state"], "succeeded", "{result}");
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["result_verified"], true);
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("trusted-policy.json"));
    assert!(!encoded.contains("--test-threads"));
    assert!(!encoded.contains("result.txt"));
    assert_eq!(result["rollback_verified"], false);
    assert!(host.process_id(id).is_err());

    let (_directory, uninstall_root) = fixture_root("uninstall");
    fs::write(
        uninstall_root.join("result.txt"),
        b"disposable installed artifact",
    )
    .unwrap();
    let value_policy = policy(&uninstall_root, "uninstall", 5_000, true);
    let mut host = manager(&uninstall_root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    let result = wait_terminal(&mut host, id);
    assert_eq!(result["state"], "succeeded", "{result}");
    assert!(!uninstall_root.join("result.txt").exists());
    assert!(host.start("fixture").unwrap_err().contains("consumed"));
}

#[cfg(windows)]
#[rstest]
fn terminal_result_releases_file_guards_while_retaining_audit_state() {
    let (directory, root) = fixture_root("install");
    let stage = directory.path().join("stage");
    fs::create_dir(&stage).unwrap();
    let executable = stage.join("fixture.exe");
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let mut value_policy = policy(&root, "install", 5_000, true);
    value_policy["recipes"][0]["executable"] = json!(executable);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    let result = wait_terminal(&mut host, id);
    assert_eq!(result["state"], "succeeded", "{result}");
    fs::write(
        &executable,
        b"staged executable can be cleaned after completion",
    )
    .unwrap();
    fs::rename(&root, directory.path().join("completed-root-moved")).unwrap();
    assert_eq!(host.status(id).unwrap(), result);
    assert_eq!(host.stop_all()["success"], true);
}

#[cfg(windows)]
#[rstest]
fn late_verification_only_confirms_cleanup_without_changing_unknown_outcome() {
    let (_directory, root) = fixture_root("install");
    let value_policy = policy(&root, "install", 5000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    assert_eq!(wait_terminal(&mut host, id)["state"], "succeeded");
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    {
        // A controlled stalled read models the worker's completion channel;
        // the child lifecycle above still uses a real isolated subprocess.
        let mut run = host.runs.get(id).unwrap().lock().unwrap();
        run.state = "verifying";
        run.result_verified = false;
        run.verification = Some(Verification {
            receiver,
            deadline: Instant::now(),
        });
        refresh(&mut run);
        assert_eq!(run.state, "outcome_unknown");
        assert!(run.verification.is_some());
    }
    assert_eq!(host.stop_all()["success"], false);
    sender.send(Ok(true)).unwrap();
    let state = host.status(id).unwrap();
    assert_eq!(state["state"], "outcome_unknown");
    assert_eq!(state["result_verified"], false);
    assert_eq!(host.stop_all()["success"], true);
}

#[cfg(windows)]
#[rstest]
#[case("exit-fail", "unexpected_process_exit")]
#[case("no-result", "result_verification_failed")]
fn exit_and_result_are_independent_checks(#[case] mode: &str, #[case] reason: &str) {
    let (_directory, root) = fixture_root(mode);
    let value_policy = policy(&root, "install", 5_000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let result = wait_terminal(&mut host, started["workflow_id"].as_str().unwrap());
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["reason"], reason);
    assert_eq!(result["result_verified"], false);
}

#[cfg(windows)]
#[rstest]
fn verifies_digest_again_before_each_launch_without_outputting_paths() {
    let (_directory, root) = fixture_root("install");
    let mut value_policy = policy(&root, "install", 1000, true);
    value_policy["recipes"][0]["executable_sha256"] = json!("0".repeat(64));
    let mut host = manager(&root, &value_policy).unwrap();
    let error = host.start("fixture").unwrap_err();
    assert_eq!(error, "workflow executable digest changed");
    assert!(!root.join("result.txt").exists());
}

#[cfg(windows)]
#[rstest]
fn deadline_is_enforced_without_status_polling() {
    let (_directory, root) = fixture_root("hang");
    let value_policy = policy(&root, "install", 100, true);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let result = wait_terminal(&mut host, id);
    assert_eq!(result["state"], "timed_out", "{result}");
    assert!(result["exit_code"].is_number());
    assert_eq!(result["result_verified"], false);
}

#[cfg(windows)]
#[rstest]
fn cancellation_is_owned_scoped_and_requires_recipe_permission() {
    let (_directory, root) = fixture_root("hang");
    let value_policy = policy(&root, "install", 5_000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    assert!(host.process_id(id).unwrap() > 0);
    let process_id = host.process_id(id).unwrap();
    assert!(host.owns_process(process_id));
    assert!(host.authorizes_process(id, 1).is_err());
    assert!(host.cancel("workflow-foreign").is_err());
    assert!(host.start("fixture").is_err());
    let _ = host.cancel(id).unwrap();
    let result = wait_terminal(&mut host, id);
    assert_eq!(result["state"], "cancelled", "{result}");
    assert_eq!(host.status(id).unwrap(), result);

    let mut no_cancel = value_policy;
    no_cancel["recipes"][0]["allow_cancel"] = json!(false);
    let mut host = manager(&root, &no_cancel).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    assert!(host.cancel(id).unwrap_err().contains("not authorized"));
    // Explicit test cleanup owns this fixture; production never gains this override.
    let state = host.runs.get(id).unwrap();
    let mut state = state.lock().unwrap();
    state.child.kill().unwrap();
    state.child.wait().unwrap();
}

#[cfg(windows)]
#[rstest]
fn policy_expiry_stops_owned_process_and_cross_connection_replay_fails() {
    let (_directory, root) = fixture_root("hang");
    let value_policy = policy(&root, "install", 5000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    let mut second = manager(&root, &value_policy).unwrap();
    assert!(second.status(id).is_err());
    host.runs.get(id).unwrap().lock().unwrap().policy_expiry = 1;
    let result = wait_terminal(&mut host, id);
    assert_eq!(result["state"], "expired", "{result}");
}

#[cfg(windows)]
#[rstest]
fn no_cancel_deadline_reports_live_unknown_and_preserves_ownership() {
    let (_directory, root) = fixture_root("hang");
    let value_policy = policy(&root, "install", 100, false);
    let mut host = manager(&root, &value_policy).unwrap();
    let started = host.start("fixture").unwrap();
    let id = started["workflow_id"].as_str().unwrap();
    let pid = started["process_id"].as_u64().unwrap() as u32;
    std::thread::sleep(Duration::from_millis(300));
    let state = host.status(id).unwrap();
    assert_eq!(state["state"], "outcome_unknown", "{state}");
    assert_eq!(state["process_exited"], false);
    assert!(host.owns_process(pid));
    assert!(host.start("fixture").is_err());
    assert_eq!(host.stop_all()["success"], false);
    let mut state = host.runs.get(id).unwrap().lock().unwrap();
    state.child.kill().unwrap();
    state.child.wait().unwrap();
}

#[cfg(not(windows))]
#[rstest]
fn unpinned_platform_launch_fails_closed() {
    let (_directory, root) = fixture_root("install");
    let value_policy = policy(&root, "install", 1000, true);
    let mut host = manager(&root, &value_policy).unwrap();
    assert!(host.start("fixture").unwrap_err().contains("unsupported"));
}
