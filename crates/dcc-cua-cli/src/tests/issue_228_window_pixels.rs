use super::*;
use rstest::rstest;

#[rstest]
fn snapshot_pixels_only_is_an_explicit_cli_contract() {
    assert_eq!(
        snapshot_mode(&strings([
            "--window-id",
            "77",
            "--pid",
            "42",
            "--pixels-only",
        ]))
        .expect("parse explicit pixel mode"),
        SnapshotMode::PixelsOnly
    );
    assert_eq!(
        snapshot_mode(&strings(["--window-id", "77", "--pid", "42"]))
            .expect("parse default snapshot mode"),
        SnapshotMode::AccessibilityPreferred
    );
}

#[rstest]
fn pixels_only_rejects_unbound_window_selection() {
    let error = snapshot_mode(&strings(["--app", "game.exe", "--pixels-only"]))
        .expect_err("pixels-only requires an exact PID/HWND pair");
    assert!(error.to_string().contains("--pid"));
    assert!(error.to_string().contains("--window-id"));
}

#[rstest]
#[case("--activate")]
#[case("--escalate")]
fn pixels_only_rejects_mutating_or_provider_starting_options(#[case] flag: &str) {
    let error = snapshot_mode(&strings([
        "--pid",
        "42",
        "--window-id",
        "77",
        "--pixels-only",
        flag,
    ]))
    .expect_err("pixels-only remains provider-free and read-only");
    assert!(error.to_string().contains("read-only"));
}

#[rstest]
fn manifest_advertises_the_provider_free_exact_window_contract() {
    let manifest = manifest::document();
    assert_eq!(manifest["runtime"]["backend"], "cua-driver-sdk");
    assert_eq!(manifest["runtime"]["separate_driver_required"], false);
    assert_eq!(
        manifest["runtime"]["exact_window_pixels"]["cli_flag"],
        "--pixels-only"
    );
    assert_eq!(
        manifest["runtime"]["exact_window_pixels"]["required_selectors"],
        json!(["--pid", "--window-id"])
    );
    assert_eq!(
        manifest["runtime"]["exact_window_pixels"]["accessibility_provider_started"],
        false
    );
    assert_eq!(
        manifest["runtime"]["exact_window_pixels"]["whole_desktop_fallback"],
        false
    );
}
