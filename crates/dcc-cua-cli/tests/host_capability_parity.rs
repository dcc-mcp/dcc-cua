//! Metadata-only executable contract: no Host, driver or window is started.
use rstest::rstest;
use serde_json::Value;
use std::process::Command;

#[rstest]
fn executable_manifest_matches_public_host_capabilities() {
    let binary = std::env::var_os("DCC_CUA_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_dcc-cua").into());
    let output = Command::new(binary)
        .arg("manifest")
        .output()
        .expect("manifest process");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let manifest: Value = serde_json::from_slice(&output.stdout).expect("manifest JSON");
    assert_eq!(
        manifest["host"]["capabilities"],
        serde_json::json!(dcc_cua_host::host_capabilities(true)),
        "manifest must not add capabilities absent from the public Host/hello contract"
    );
    assert_eq!(
        manifest["runtime"].get("exact_window_pixels").is_some(),
        cfg!(windows)
    );
}
