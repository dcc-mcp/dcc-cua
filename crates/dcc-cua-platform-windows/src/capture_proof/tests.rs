use super::*;
use crate::visible_capture::{RootBoundsClass, RootBoundsFailureDiagnostic, RootBoundsRole};
use rstest::rstest;

fn sample() -> NativeProofPhase {
    NativeProofPhase {
        elapsed_us: 1,
        thread_dpi_context: NativeDpiContext::PerMonitorAwareV2,
        target_root_window_handle: 99,
        win32_bounds: Some([80, 80, 1500, 1400]),
        win32_bounds_error: None,
        dwm_bounds: Some([80, 80, 1500, 1400]),
        dwm_bounds_error: None,
        virtual_desktop: [0, 0, 3840, 2400],
        target_inside_virtual_desktop: true,
        target_visible: true,
        target_minimized: false,
        target_dpi: 240,
        instance: Some(ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 42,
            window_class_hash: 123,
            owner_window_handle: 0,
        }),
        proof_passed: true,
        error: None,
        roots_above_through_target: Vec::new(),
    }
}

#[rstest]
#[case("unchanged", true)]
#[case("creation", false)]
#[case("thread", false)]
#[case("class", false)]
#[case("owner", false)]
#[case("instance_missing", false)]
#[case("root", false)]
#[case("win32", false)]
#[case("dwm", false)]
#[case("dpi", false)]
#[case("desktop", false)]
#[case("visible", false)]
#[case("minimized", false)]
#[case("dpi_context", false)]
fn capture_proof_stability_detects_instance_and_coordinate_drift(
    #[case] mutation: &str,
    #[case] stable: bool,
) {
    let before = sample();
    let mut after = before.clone();
    match mutation {
        "unchanged" => {}
        "creation" => after.instance.as_mut().unwrap().process_creation_time_100ns += 1,
        "thread" => after.instance.as_mut().unwrap().window_thread_id += 1,
        "class" => after.instance.as_mut().unwrap().window_class_hash += 1,
        "owner" => after.instance.as_mut().unwrap().owner_window_handle += 1,
        "instance_missing" => after.instance = None,
        "root" => after.target_root_window_handle += 1,
        "win32" => after.win32_bounds.as_mut().unwrap()[0] += 1,
        "dwm" => after.dwm_bounds.as_mut().unwrap()[0] += 1,
        "dpi" => after.target_dpi += 1,
        "desktop" => after.virtual_desktop[0] += 1,
        "visible" => after.target_visible = false,
        "minimized" => after.target_minimized = true,
        "dpi_context" => after.thread_dpi_context = NativeDpiContext::SystemAware,
        _ => unreachable!(),
    }
    assert_eq!(target_evidence_stable(&before, &after), stable);
}

#[rstest]
fn capture_proof_error_payload_contains_only_closed_numeric_metadata() {
    let mut failure = diagnostic(VisibleWindowCaptureReason::RootCloakingUnavailable);
    failure.blocker_process_id = Some(42);
    failure.blocker_window_handle = Some(123);
    failure.os_error = Some(-2147024891);
    assert_eq!(
        serde_json::to_value(failure).unwrap(),
        serde_json::json!({
            "reason":"root_cloaking_unavailable", "target_bounds":null,
            "blocker_process_id":42, "blocker_window_handle":123, "blocker_bounds":null,
            "cloaked":null, "os_error":-2147024891,
        })
    );
    failure.root_bounds_failure = Some(RootBoundsFailureDiagnostic {
        root_role: RootBoundsRole::AboveTargetRoot,
        proof_target_root_window_handle: 99,
        dwm_raw_rect_edges: [0, 0, 0, 20],
        dwm_classification: RootBoundsClass::ZeroArea,
        visible: true,
        cloaked: Some(0),
        win32_read_after_dwm_rejection: true,
        win32_raw_rect_edges: Some([0, 0, 1, 20]),
        win32_classification: Some(RootBoundsClass::Positive),
        win32_os_error: None,
        zero_area_status_mismatch: Some(true),
    });
    assert_eq!(
        serde_json::to_value(failure).unwrap()["root_bounds_failure"],
        serde_json::json!({
            "root_role": "above_target_root",
            "proof_target_root_window_handle": 99,
            "dwm_raw_rect_edges": [0, 0, 0, 20],
            "dwm_classification": "zero_area",
            "visible": true,
            "cloaked": 0,
            "win32_read_after_dwm_rejection": true,
            "win32_raw_rect_edges": [0, 0, 1, 20],
            "win32_classification": "positive",
            "win32_os_error": null,
            "zero_area_status_mismatch": true,
        })
    );
}

#[rstest]
fn capture_proof_composited_metadata_does_not_imply_capture_authorization() {
    let phase = sample();
    assert!(target_evidence_stable(&phase, &phase));
    let report = NativeCaptureProof {
        schema_version: 1,
        process_id: 42,
        window_handle: 99,
        pixels_read: false,
        input_sent: false,
        authorizes_capture_or_input: false,
        max_root_entries_per_phase: MAX_ROOT_WINDOWS,
        caller_dpi_context: NativeDpiContext::SystemAware,
        restored_dpi_context: NativeDpiContext::SystemAware,
        dpi_scope_error: None,
        before_flush: Some(phase.clone()),
        after_flush: Some(phase),
        flush_error: None,
        desktop_dc_after_flush: None,
        target_evidence_stable: true,
        display_color: None,
    };
    let value = serde_json::to_value(report).unwrap();
    assert_eq!(value["authorizes_capture_or_input"], false);
    assert_eq!(value["pixels_read"], false);
    assert_eq!(value["input_sent"], false);
}
