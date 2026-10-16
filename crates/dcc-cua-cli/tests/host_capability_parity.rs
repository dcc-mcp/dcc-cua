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
        cfg!(any(windows, target_os = "linux"))
    );
    if cfg!(target_os = "linux") {
        let pixels = &manifest["runtime"]["exact_window_pixels"];
        assert_eq!(pixels["availability"], "linux_local_x11_non_composited");
        assert_eq!(pixels["read_only"], true);
        assert_eq!(pixels["input_available"], false);
        assert_eq!(pixels["activation_available"], false);
        assert_eq!(pixels["recording_available"], false);
    }
}
