use super::*;
use rstest::rstest;

#[rstest]
#[case(false)]
#[case(true)]
fn semantic_root_activation_without_existing_worker_consumes_context_without_startup(
    #[case] existing_snapshot: bool,
) {
    let target = UiaTarget {
        process_id: 42,
        window_handle: 77,
    };
    let mut session = UiaSession::new(target);
    if existing_snapshot {
        session.snapshot = Some(
            normalize(&json!({"root":{
                "runtime_id":"root-id", "control_type":"ControlType.Window", "children":[]
            }}))
            .unwrap()
            .1,
        );
    }
    let instance = crate::ExactWindowPixelInstanceEvidence {
        process_creation_time_100ns: 7,
        window_thread_id: 8,
        window_class_hash: 9,
        owner_window_handle: 0,
    };
    session.root_activation_snapshot = Some((instance, std::time::Instant::now()));
    let before = crate::ExactWindowNativeState {
        process_id: 42,
        window_handle: 77,
        bounds: Some([0, 0, 800, 600]),
        visible_bounds: Some([0, 0, 800, 600]),
        dpi: 96,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: false,
        instance,
    };
    let diagnostic = serde_json::from_value(json!({
        "caller_process_id":1,"caller_thread_id":2,"target":{"process_id":42,"window_handle":77},
        "target_thread_id":8,"target_root_window_handle":77,"target_owner_window_handle":0,
        "initial_foreground":null,"final_foreground":null,"attempts":[],"foreground_poll_count":20
    }))
    .unwrap();
    let result = session.focus_root_after_activation_refusal(before, diagnostic, || {
        panic!("no worker means no native gate, request, focus or capture")
    });
    assert!(result.prerequisite_error.is_none());
    let focus = result.diagnostic.semantic_root_focus.unwrap();
    assert_eq!(
        focus.status,
        crate::WindowsSemanticRootFocusStatus::UnavailableContext
    );
    assert_eq!(focus.focus_attempted, Some(false));
    assert!(!focus.provider_completed);
    assert!(session.snapshot.is_none());
    assert!(session.root_activation_snapshot.is_none());
    assert!(session.worker.is_none());
    assert!(result.diagnostic.attempts.is_empty());
    assert_eq!(result.diagnostic.foreground_poll_count, 20);
}
