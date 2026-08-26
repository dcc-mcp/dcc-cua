use super::*;
#[allow(unused_imports)]
use rstest::rstest;

pub(super) fn is_safe_foreground_refusal(error: &HostClientError) -> bool {
    matches!(
        error,
        HostClientError::Remote { code, response, .. }
            if code == "foreground_activation_refused"
                && response["details"]["action_attempted"] == false
                && response["details"]["input_sent"] == "not_sent"
    )
}

#[rstest]
fn typed_foreground_refusal_is_only_safe_before_input_dispatch() {
    let refusal = |action_attempted, input_sent| HostClientError::Remote {
        code: "foreground_activation_refused".into(),
        message: "controlled fixture refused activation".into(),
        response: json!({
            "details": {
                "action_attempted": action_attempted,
                "input_sent": input_sent,
            }
        }),
    };
    assert!(is_safe_foreground_refusal(&refusal(false, "not_sent")));
    assert!(!is_safe_foreground_refusal(&refusal(true, "not_sent")));
    assert!(!is_safe_foreground_refusal(&refusal(false, "unknown")));
}

pub(super) fn physically_focus_exact_window(process_id: u32, window_handle: u64) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
        SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, GetWindowRect, SetCursorPos, SetForegroundWindow,
    };

    let window = window_handle as *mut core::ffi::c_void;
    let mut rect = RECT::default();
    let mut cursor = POINT::default();
    unsafe {
        assert_ne!(GetWindowRect(window, &mut rect), 0, "read fixture bounds");
        assert_ne!(GetCursorPos(&mut cursor), 0, "read cursor position");
        assert_ne!(
            SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2),
            0,
            "move cursor onto fixture"
        );
    }
    let inputs = [
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: MOUSEEVENTF_LEFTDOWN,
                    ..MOUSEINPUT::default()
                },
            },
        },
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: MOUSEEVENTF_LEFTUP,
                    ..MOUSEINPUT::default()
                },
            },
        },
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    unsafe {
        assert_ne!(
            SetCursorPos(cursor.x, cursor.y),
            0,
            "restore cursor position"
        );
        SetForegroundWindow(window);
    }
    assert_eq!(sent, inputs.len() as u32, "focus fixture with one click");
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { GetForegroundWindow() } != window {
        assert!(
            Instant::now() < deadline,
            "fixture did not become foreground"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_exact_foreground_window(process_id, window_handle);
}

pub(super) fn assert_exact_foreground_window(process_id: u32, window_handle: u64) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    let mut foreground_process_id = 0;
    let foreground_window = unsafe { GetForegroundWindow() };
    assert_ne!(
        unsafe { GetWindowThreadProcessId(foreground_window, &mut foreground_process_id) },
        0,
        "read exact foreground process identity"
    );
    assert_eq!(
        foreground_window, window_handle as *mut core::ffi::c_void,
        "exact foreground HWND drifted"
    );
    assert_eq!(
        foreground_process_id, process_id,
        "exact foreground HWND belongs to another process"
    );
}

pub(super) async fn assert_ordinary_raw_click_uses_task_grant(
    client: &mut HostClient,
    session_id: &str,
    grant_id: &str,
    capability: &str,
    initial_snapshot: &HostResponse,
) {
    let input_match = client_request(
        client,
        "find",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "query": {"text": "txt-input", "max_results": 1}
        }),
    )
    .await;
    let input = input_match.value["matches"]
        .as_array()
        .and_then(|matches| matches.first())
        .expect("WPF input match for confirmation regression");
    let (input_x, input_y) = screenshot_point_at(&initial_snapshot.value, input, 0.9);
    let focused = client_request(
        client,
        "execute_action",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "observation_id": initial_snapshot.value["observation_id"],
            "accessibility_state_id": initial_snapshot.value["accessibility_state_id"],
            "action": {
                "action": "click",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "delivery_mode": "foreground",
                "x": input_x,
                "y": input_y
            },
            "capture_after": false
        }),
    )
    .await;
    assert_eq!(focused.value["success"], true, "{}", focused.value);
    assert_eq!(focused.value["policy_tier"], "task_grant");
}
