use super::*;
use rstest::rstest;

#[rstest]
fn snapshot_pixels_only_is_an_explicit_cli_contract() {
    reject_unknown_flags(&strings([
        "--pid",
        "42",
        "--window-id",
        "77",
        "--pixels-only",
    ]))
    .expect("the published CLI accepts its documented pixels-only flag");
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
    let manifest = manifest::document_for_platform(true);
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

#[rstest]
fn manifest_omits_windows_only_exact_pixels_on_non_windows_platforms() {
    let manifest = manifest::document_for_platform(false);
    assert!(manifest["runtime"].get("exact_window_pixels").is_none());
    assert!(
        !manifest["host"]["capabilities"]
            .as_array()
            .expect("manifest capabilities")
            .iter()
            .any(|value| value == "exact_window_pixels")
    );
}

#[rstest]
fn manifest_advertises_exact_pixels_only_when_windows_capture_is_available() {
    let manifest = manifest::document_for_platform(true);
    assert_eq!(
        manifest["runtime"]["exact_window_pixels"]["availability"],
        "windows"
    );
    assert_eq!(
        manifest["host"]["capabilities"],
        json!(dcc_cua_host::host_capabilities(true))
    );
}

#[rstest]
#[case(true)]
#[case(false)]
fn runtime_platform_availability_does_not_extend_the_host_protocol(#[case] windows: bool) {
    let manifest = manifest::document_for_platform(windows);
    assert_eq!(
        manifest["host"]["capabilities"],
        json!(dcc_cua_host::host_capabilities(true))
    );
    assert_eq!(
        manifest["runtime"].get("exact_window_pixels").is_some(),
        windows
    );
}

#[rstest]
fn packaged_skill_describes_the_verified_visible_desktop_crop_truthfully() {
    let skill = include_str!("../../../../skills/cua-cli/SKILL.md");
    let normalized = skill.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(normalized.contains("`VisibleDesktopCrop` fallback"));
    assert!(normalized.contains("physical rectangle from the desktop DC"));
    assert!(normalized.contains("never publishes a whole-desktop screenshot"));
    assert!(!normalized.contains("never falls back to a whole-desktop screenshot or desktop crop"));
}
