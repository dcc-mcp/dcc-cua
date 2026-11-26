use super::*;
use crate::frame_quality::{CaptureMode, FrameAssessment, assess_bgra};
use rstest::rstest;

fn capability() -> InProcessQtCapability {
    InProcessQtCapability {
        mode: CaptureMode::InProcessQt,
        readiness: InProcessQtReadiness::Ready,
        target: InProcessQtTargetBinding {
            connection_id: "connection-1".into(),
            task_id: "task-1".into(),
            process_id: 42,
            window_handle: 9001,
            document_generation: 7,
        },
        queue: InProcessQtQueuePolicy {
            max_depth: 8,
            max_command_lifetime_ms: 5_000,
        },
    }
}

fn request() -> InProcessQtCaptureRequest {
    InProcessQtCaptureRequest {
        target: capability().target,
        widget: QtWidgetIdentity {
            widget_path: "MainWindow/NetworkEditor".into(),
            class_name: "QWidget".into(),
            object_name: Some("networkEditor".into()),
            widget_generation: 3,
        },
        deadline_ms: 1_000,
    }
}

#[rstest]
fn request_requires_the_capability_target_and_bounded_deadline() {
    let capability = capability();
    assert!(request().validate(&capability).is_ok());

    let mut changed = request();
    changed.target.document_generation += 1;
    assert_eq!(
        changed.validate(&capability),
        Err(InProcessQtContractError::BindingChanged)
    );

    let mut expired = request();
    expired.deadline_ms = capability.queue.max_command_lifetime_ms + 1;
    assert_eq!(
        expired.validate(&capability),
        Err(InProcessQtContractError::InvalidDeadline)
    );
}

#[rstest]
#[case(
    InProcessQtReadiness::Unavailable,
    InProcessQtContractError::UnsupportedMode
)]
#[case(
    InProcessQtReadiness::WaitingForDocument,
    InProcessQtContractError::NotReady
)]
#[case(InProcessQtReadiness::Disconnected, InProcessQtContractError::NotReady)]
fn request_requires_ready_capture_capability(
    #[case] readiness: InProcessQtReadiness,
    #[case] expected: InProcessQtContractError,
) {
    let mut capability = capability();
    capability.readiness = readiness;
    assert_eq!(request().validate(&capability), Err(expected));
}

#[rstest]
#[case(0, 1_000)]
#[case(33, 1_000)]
#[case(8, 0)]
#[case(8, 30_001)]
fn invalid_queue_policy_is_rejected(#[case] max_depth: u32, #[case] max_command_lifetime_ms: u64) {
    let mut capability = capability();
    capability.queue = InProcessQtQueuePolicy {
        max_depth,
        max_command_lifetime_ms,
    };
    assert_eq!(
        capability.validate(),
        Err(InProcessQtContractError::InvalidQueuePolicy)
    );
}

#[rstest]
fn widget_identity_rejects_empty_or_ambiguous_fields() {
    let mut widget = request().widget;
    widget.widget_path.clear();
    assert_eq!(
        widget.validate(),
        Err(InProcessQtContractError::InvalidIdentifier)
    );

    let mut widget = request().widget;
    widget.widget_generation = 0;
    assert_eq!(
        widget.validate(),
        Err(InProcessQtContractError::InvalidWidget)
    );
}

#[rstest]
fn capability_and_receipt_keep_typed_wire_values() {
    let capability = capability();
    let encoded = serde_json::to_value(&capability).unwrap();
    assert_eq!(encoded["mode"], "in_process_qt");
    assert_eq!(encoded["readiness"], "ready");

    let receipt = InProcessQtCaptureReceipt {
        status: InProcessQtCommandStatus::BlankCapture,
        target: capability.target,
        widget: request().widget,
        assessment: None,
        failure: Some(InProcessQtFailure::BlankCapture),
    };
    let encoded = serde_json::to_value(receipt).unwrap();
    assert_eq!(encoded["status"], "blank_capture");
    assert_eq!(encoded["failure"], "blank_capture");
}

#[rstest]
fn receipt_requires_coherent_capture_evidence() {
    let capability = capability();
    let widget = request().widget;
    let informative = InProcessQtCaptureReceipt {
        status: InProcessQtCommandStatus::Captured,
        target: capability.target.clone(),
        widget: widget.clone(),
        assessment: Some(
            assess_bgra(
                CaptureMode::InProcessQt,
                &[0, 0, 0, 255, 0, 0, 255, 255],
                2,
                1,
            )
            .unwrap(),
        ),
        failure: None,
    };
    assert!(informative.validate().is_ok());

    let blank = InProcessQtCaptureReceipt {
        status: InProcessQtCommandStatus::BlankCapture,
        target: capability.target,
        widget,
        assessment: Some(assess_bgra(CaptureMode::InProcessQt, &[0, 0, 0, 255], 1, 1).unwrap()),
        failure: Some(InProcessQtFailure::BlankCapture),
    };
    assert!(blank.validate().is_ok());

    let mut forged = informative;
    forged.failure = Some(InProcessQtFailure::BlankCapture);
    assert_eq!(
        forged.validate(),
        Err(InProcessQtContractError::InvalidReceipt)
    );

    let mut wrong_mode = blank;
    wrong_mode.assessment = Some(FrameAssessment::BlankCapture {
        attempted_mode: CaptureMode::WindowContent,
        statistics: match wrong_mode.assessment.take().unwrap() {
            FrameAssessment::BlankCapture { statistics, .. } => statistics,
            _ => unreachable!(),
        },
    });
    assert_eq!(
        wrong_mode.validate(),
        Err(InProcessQtContractError::InvalidReceipt)
    );
}
