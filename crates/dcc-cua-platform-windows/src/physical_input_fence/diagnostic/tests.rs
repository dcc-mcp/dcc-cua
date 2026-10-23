use super::*;
use rstest::rstest;

fn sample() -> RejectedFenceDiagnostic {
    let fence = WindowsPhysicalInputFence {
        target: UiaTarget {
            process_id: 42,
            window_handle: 99,
        },
        native_instance: ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 1,
            window_thread_id: 2,
            window_class_hash: 3,
            owner_window_handle: 0,
        },
        native_window_bounds: [1200, 260, 1800, 1050],
        native_visible_bounds: [1211, 260, 1778, 1039],
        window_dpi: 192,
    };
    RejectedFenceDiagnostic::new(
        fence,
        Some((1461, 1060)),
        ExactWindowPixelEvidence {
            process_id: 42,
            window_handle: 99,
            bounds: fence.native_window_bounds,
            visible_bounds: fence.native_visible_bounds,
            dpi: 192,
            visible: true,
            minimized: false,
            unobscured: true,
            visibility_failure: None,
            instance: fence.native_instance,
        },
    )
}

#[rstest]
#[case(0)]
#[case(16)]
#[case(17)]
#[case(4096)]
fn rejected_point_retains_hit_and_bounds_without_granting_authority(#[case] count: usize) {
    let mut diagnostic = sample();
    diagnostic.attach_point(crate::visible_capture::ExactWindowPointEvidence {
        visible: false,
        hit_window_handle: Some(123),
        hit_process_id: Some(456),
        hit_root_window_handle: Some(789),
        hit_is_target_or_child: false,
        redacted_regions: vec![[1211, 260, 1778, 1039]; count],
    });
    let diagnostic = diagnostic.sample;
    assert!(!diagnostic.authorizes_input);
    assert_eq!(diagnostic.point, Some([1461, 1060]));
    assert!(diagnostic.unobscured);
    assert!(diagnostic.point_proof_complete);
    assert_eq!(diagnostic.point_visible, Some(false));
    assert_eq!(diagnostic.hit_process_id, Some(456));
    assert_eq!(diagnostic.hit_root_window_handle, Some(789));
    assert_eq!(diagnostic.redaction_count, count as u32);
    assert_eq!(diagnostic.redacted_rectangles.len(), count.min(16));
    assert_eq!(diagnostic.redactions_complete, count <= 16);
    assert_eq!(
        serde_json::to_value(diagnostic)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        15
    );
}

#[rstest]
fn incomplete_point_proof_is_explicit_and_contains_no_guessed_hit() {
    let diagnostic = sample().sample;
    assert!(!diagnostic.point_proof_complete);
    assert!(!diagnostic.redactions_complete);
    assert_eq!(diagnostic.hit_window_handle, None);
    assert_eq!(diagnostic.point_visible, None);
    assert!(diagnostic.redacted_rectangles.is_empty());
}
