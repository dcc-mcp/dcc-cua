use super::*;
use rstest::rstest;

#[rstest]
fn activation_frame_coordinates_are_physical_and_restore_unaware_caller() {
    std::thread::spawn(|| {
        use windows_sys::Win32::{
            Foundation::{HINSTANCE, HWND},
            UI::{
                HiDpi::{
                    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_UNAWARE,
                    GetThreadDpiAwarenessContext, SetThreadDpiAwarenessContext,
                },
                WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW},
            },
        };

        struct RestoreDpiContext(DPI_AWARENESS_CONTEXT);
        impl Drop for RestoreDpiContext {
            fn drop(&mut self) {
                unsafe { SetThreadDpiAwarenessContext(self.0) };
            }
        }
        struct HiddenWindow(HWND);
        impl Drop for HiddenWindow {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0) };
            }
        }

        let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE) };
        assert!(!previous.is_null(), "enter unaware caller context");
        let _restore_context = RestoreDpiContext(previous);
        let (window, physical_frame) = {
            let _dpi = crate::visible_capture::ThreadDpiAwarenessGuard::per_monitor_v2()
                .expect("enter physical fixture coordinate scope");
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let window = HiddenWindow(unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    80,
                    80,
                    280,
                    220,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    HINSTANCE::default(),
                    std::ptr::null(),
                )
            });
            assert!(!window.0.is_null(), "create hidden frame fixture");
            let mut rect = RECT::default();
            assert_ne!(unsafe { GetWindowRect(window.0, &mut rect) }, 0);
            (window, [rect.left, rect.top, rect.right, rect.bottom])
        };
        let mut actual = RECT::default();
        let mut physical_context = false;
        with_activation_frame_coordinates(|| {
            physical_context = unsafe {
                AreDpiAwarenessContextsEqual(
                    GetThreadDpiAwarenessContext(),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                ) != 0
            };
            assert_ne!(unsafe { GetWindowRect(window.0, &mut actual) }, 0);
            Ok(())
        })
        .expect("read activation frame in physical coordinate scope");

        assert_ne!(
            unsafe {
                AreDpiAwarenessContextsEqual(
                    GetThreadDpiAwarenessContext(),
                    DPI_AWARENESS_CONTEXT_UNAWARE,
                )
            },
            0,
            "restore the unaware caller after frame operations"
        );
        assert!(
            physical_context,
            "activation frame operations must use PMv2"
        );
        assert_eq!(
            [actual.left, actual.top, actual.right, actual.bottom],
            physical_frame,
            "activation frame operations must read physical desktop pixels"
        );
    })
    .join()
    .expect("unaware activation frame worker");
}

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
