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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WindowBinding {
    pub process_id: u32,
    pub window_handle: u64,
    pub title: String,
    pub app_name: String,
    pub process_creation_time_100ns: u64,
    pub window_thread_id: u32,
    pub window_class_hash: u64,
    pub owner_window_handle: u64,
    pub window_user_data: isize,
}

impl WindowBinding {
    pub(super) fn matches_inventory(
        &self,
        process_id: u32,
        window_handle: u64,
        title: &str,
        app_name: &str,
    ) -> bool {
        self.process_id == process_id
            && self.window_handle == window_handle
            && self.title == title
            && self.app_name.eq_ignore_ascii_case(app_name)
    }
}

fn non_client_focus_point(window_rect: [i32; 4], caption_height: i32) -> (i32, i32) {
    let [left, top, right, bottom] = window_rect;
    let x = left + (right - left) / 2;
    let y = top + caption_height.max(2) / 2;
    (x.clamp(left + 1, right - 1), y.clamp(top + 1, bottom - 1))
}

#[rstest]
fn physical_focus_fallback_clicks_the_non_client_caption() {
    let point = non_client_focus_point([16, 16, 916, 666], 30);
    assert_eq!(point, (466, 31));
    assert!(point.1 < 46, "caption click must stay above client content");
}

pub(super) fn physically_focus_exact_window(process_id: u32, window_handle: u64) {
    try_physically_focus_exact_window(process_id, window_handle)
        .unwrap_or_else(|error| panic!("{error}"));
}

pub(super) fn try_physically_focus_exact_window(
    process_id: u32,
    window_handle: u64,
) -> Result<(), String> {
    let expected = observe_window_binding(window_handle)?;
    if expected.process_id != process_id {
        return Err(format!(
            "controlled exact-window focus target identity drifted before raw input: expected_pid={process_id} expected_hwnd={window_handle} observed_pid={}",
            expected.process_id
        ));
    }
    if foreground_window_binding()?.as_ref() == Some(&expected) {
        return Ok(());
    }
    request_physical_focus_exact_window(&expected)?;
    const OBSERVATION_LIMIT: usize = 61;
    let mut last_observed = None;
    for observation in 1..=OBSERVATION_LIMIT {
        let current_target = observe_window_binding(window_handle)?;
        if current_target != expected {
            return Err(format!(
                "controlled exact-window focus target instance drifted before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
            ));
        }
        last_observed = foreground_window_binding()?;
        if last_observed.as_ref() == Some(&expected) {
            return Ok(());
        }
        if observation < OBSERVATION_LIMIT {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let (observed_pid, observed_hwnd) = last_observed
        .as_ref()
        .map(|binding| (binding.process_id, binding.window_handle))
        .unwrap_or((0, 0));
    Err(format!(
        "controlled exact-window focus timed out before raw input: expected_pid={process_id} expected_hwnd={window_handle} observed_pid={} observed_hwnd={} observations={OBSERVATION_LIMIT}",
        observed_pid, observed_hwnd
    ))
}

pub(super) fn request_physical_focus_exact_window(expected: &WindowBinding) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
        SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, SM_CYCAPTION,
        SetCursorPos,
    };

    let process_id = expected.process_id;
    let window_handle = expected.window_handle;
    let window = window_handle as *mut core::ffi::c_void;
    let mut rect = RECT::default();
    let mut cursor = POINT::default();
    let original_cursor;
    unsafe {
        let observed = observe_window_binding(window_handle)?;
        if observed != *expected {
            return Err(format!(
                "controlled exact-window focus target title/app/instance drifted before raw input: expected_pid={process_id} expected_hwnd={window_handle} expected_title={:?} expected_app={:?} observed_title={:?} observed_app={:?}",
                expected.title, expected.app_name, observed.title, observed.app_name
            ));
        }
        let mut owner_process_id = 0;
        if GetWindowThreadProcessId(window, &mut owner_process_id) == 0
            || owner_process_id != process_id
        {
            return Err(format!(
                "controlled exact-window focus target identity drifted before raw input: expected_pid={process_id} expected_hwnd={window_handle} observed_pid={owner_process_id}"
            ));
        }
        if GetWindowRect(window, &mut rect) == 0 {
            return Err(format!(
                "controlled exact-window focus could not read fixture bounds before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
            ));
        }
        if foreground_window_binding()?.as_ref() == Some(expected) {
            return Ok(());
        }
        original_cursor = (GetCursorPos(&mut cursor) != 0).then_some(cursor);
        let (focus_x, focus_y) = non_client_focus_point(
            [rect.left, rect.top, rect.right, rect.bottom],
            GetSystemMetrics(SM_CYCAPTION),
        );
        if SetCursorPos(focus_x, focus_y) == 0 {
            return Err(format!(
                "controlled exact-window focus could not move onto fixture before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
            ));
        }
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
        if let Some(original_cursor) = original_cursor {
            SetCursorPos(original_cursor.x, original_cursor.y);
        }
    }
    if sent != inputs.len() as u32 {
        return Err(format!(
            "controlled exact-window focus request was incomplete before raw input: expected_pid={process_id} expected_hwnd={window_handle} sent={sent} required={}",
            inputs.len()
        ));
    }
    Ok(())
}

pub(super) fn foreground_window_binding() -> Result<Option<WindowBinding>, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return Ok(None);
    }
    observe_window_binding(window as usize as u64).map(Some)
}

pub(super) fn observe_window_binding(window_handle: u64) -> Result<WindowBinding, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_OWNER, GWLP_USERDATA, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindow,
    };

    let window = window_handle as *mut core::ffi::c_void;
    if window.is_null() || unsafe { IsWindow(window) } == 0 {
        return Err(format!(
            "controlled exact-window focus target is absent before raw input: expected_hwnd={window_handle}"
        ));
    }
    let mut process_id = 0;
    let window_thread_id = unsafe { GetWindowThreadProcessId(window, &mut process_id) };
    if window_thread_id == 0 || process_id == 0 {
        return Err(format!(
            "controlled exact-window focus target owner is unavailable before raw input: expected_hwnd={window_handle}"
        ));
    }

    let title_len = unsafe { GetWindowTextLengthW(window) };
    let mut title_units = vec![0_u16; usize::try_from(title_len.max(0)).unwrap_or(0) + 1];
    let copied = unsafe {
        GetWindowTextW(
            window,
            title_units.as_mut_ptr(),
            i32::try_from(title_units.len()).unwrap_or(i32::MAX),
        )
    };
    let title =
        String::from_utf16_lossy(&title_units[..usize::try_from(copied.max(0)).unwrap_or(0)]);

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(format!(
            "controlled exact-window focus target application is unavailable before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
        ));
    }
    let mut image = vec![0_u16; 32_768];
    let mut image_len = image.len() as u32;
    let image_ok =
        unsafe { QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut image_len) != 0 };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let times_ok =
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) != 0 };
    unsafe { CloseHandle(process) };
    if !image_ok || !times_ok {
        return Err(format!(
            "controlled exact-window focus target application instance is unavailable before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
        ));
    }
    let image_path = String::from_utf16_lossy(&image[..image_len as usize]);
    let app_name = std::path::Path::new(&image_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&image_path)
        .to_owned();
    let process_creation_time_100ns =
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);

    let mut class_units = [0_u16; 256];
    let class_len = unsafe { GetClassNameW(window, class_units.as_mut_ptr(), 256) };
    if class_len <= 0 {
        return Err(format!(
            "controlled exact-window focus target class is unavailable before raw input: expected_hwnd={window_handle}"
        ));
    }
    let window_class_hash = class_units[..class_len as usize]
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, unit| {
            (hash ^ u64::from(*unit)).wrapping_mul(0x100000001b3)
        });
    Ok(WindowBinding {
        process_id,
        window_handle,
        title,
        app_name,
        process_creation_time_100ns,
        window_thread_id,
        window_class_hash,
        owner_window_handle: unsafe { GetWindow(window, GW_OWNER) } as usize as u64,
        window_user_data: unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) },
    })
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

pub(super) async fn assert_ordinary_raw_click_focuses_control(
    client: &mut HostClient,
    session_id: &str,
    grant_id: &str,
    capability: &str,
    initial_snapshot: &HostResponse,
    automation_id: &str,
) {
    let input_match = client_request(
        client,
        "find",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "query": {"text": automation_id, "max_results": 1}
        }),
    )
    .await;
    let input = input_match.value["matches"]
        .as_array()
        .and_then(|matches| matches.first())
        .unwrap_or_else(|| panic!("WPF input match for {automation_id}"));
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
