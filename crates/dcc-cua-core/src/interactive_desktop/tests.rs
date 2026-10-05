use super::*;
use rstest::rstest;

#[rstest]
fn typed_cursor_errors_remain_independent_of_other_probe_success() {
    let value = json!({
        "get_cursor_pos": cursor_probe_result("GetCursorPos", Err(std::io::Error::from_raw_os_error(5))),
        "get_cursor_info": cursor_probe_result("GetCursorInfo", Ok(())),
    });
    assert_eq!(value["get_cursor_pos"]["status"], "failed");
    assert_eq!(value["get_cursor_pos"]["stage"], "GetCursorPos");
    assert_eq!(value["get_cursor_pos"]["os_error"], 5);
    assert_eq!(value["get_cursor_info"]["status"], "ok");
    let untyped = cursor_probe_result("GetCursorInfo", Err(std::io::Error::other("fixture")));
    assert!(untyped["os_error"].is_null());
    assert_eq!(untyped["io_kind"], "Other");
}

#[cfg(windows)]
#[rstest]
fn cursor_info_initialization_sets_required_abi_size_without_native_call() {
    use windows_sys::Win32::UI::WindowsAndMessaging::CURSORINFO;
    let info = cursor_info_for_probe();
    assert_eq!(info.cbSize, std::mem::size_of::<CURSORINFO>() as u32);
    assert_eq!(info.flags, 0);
    assert!(info.hCursor.is_null());
    assert_eq!(info.ptScreenPos.x, 0);
    assert_eq!(info.ptScreenPos.y, 0);
}
