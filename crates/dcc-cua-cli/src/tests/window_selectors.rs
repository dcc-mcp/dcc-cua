use rstest::rstest;
use serde_json::json;

use super::*;

#[rstest]
fn restore_activate_requires_an_exact_pid_and_window_pair() {
    assert!(require_exact_window_target(&strings(["--pid", "42", "--window-id", "7"])).is_ok());
    assert!(require_exact_window_target(&strings(["--pid", "42"])).is_err());
    assert!(require_exact_window_target(&strings(["--window-id", "7"])).is_err());
    assert!(require_exact_window_target(&strings(["--app", "TheBazaar.exe"])).is_err());
}

#[rstest]
fn parser_preserves_every_conjunctive_identity() {
    let query = window_selector_query_from_flags(&strings([
        "--app",
        "fixture.exe",
        "--pid",
        "42",
        "--window-id",
        "77",
        "--title",
        "Fixture A",
    ]))
    .unwrap();

    assert_eq!(query.app.as_deref(), Some("fixture.exe"));
    assert_eq!(query.process_id, Some(42));
    assert_eq!(query.window_handle, Some(77));
    assert_eq!(query.window_title.as_deref(), Some("Fixture A"));
}

#[rstest]
fn app_only_selector_rejects_a_multi_window_inventory() {
    let query = window_selector_query_from_flags(&strings(["--app", "fixture.exe"])).unwrap();
    let rows = vec![
        json!({
            "app_name": "fixture.exe", "pid": 42, "window_id": 77,
            "title": "Fixture A", "is_on_screen": true
        }),
        json!({
            "app_name": "fixture.exe", "pid": 42, "window_id": 78,
            "title": "Fixture B", "is_on_screen": true
        }),
    ];

    let error = resolve_window_selector_from_inventory(&query, &rows).unwrap_err();
    assert_eq!(
        error.to_string(),
        "expected one on-screen fixture.exe window, found 2"
    );
}

#[rstest]
fn parser_rejects_conflicting_duplicate_identities() {
    let error = window_selector_query_from_flags(&strings([
        "--pid",
        "42",
        "--pid",
        "43",
        "--window-id",
        "77",
    ]))
    .unwrap_err();

    assert_eq!(error.to_string(), "conflicting values for --pid");
}

#[rstest]
#[case("--pid")]
#[case("--window-id")]
fn parser_rejects_zero_native_identity(#[case] selector: &str) {
    let error = window_selector_query_from_flags(&strings([selector, "0"])).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{selector} must be greater than zero")
    );
}

#[rstest]
#[case(0, 77, "zero pid")]
#[case(42, 0, "zero window id")]
#[case(u64::from(u32::MAX) + 1, 77, "pid outside the public u32 contract")]
fn inventory_resolution_rejects_invalid_native_identity(
    #[case] pid: u64,
    #[case] window_id: u64,
    #[case] reason: &str,
) {
    let query = window_selector_query_from_flags(&strings(["--app", "fixture.exe"])).unwrap();
    let rows = vec![json!({
        "app_name": "fixture.exe",
        "pid": pid,
        "window_id": window_id,
        "title": "Fixture A",
        "is_on_screen": true
    })];

    let result = resolve_window_selector_from_inventory(&query, &rows);
    assert!(result.is_err(), "{reason} was accepted as {result:?}");
}

#[rstest]
fn parser_rejects_a_selector_without_a_value() {
    let result = window_selector_query_from_flags(&strings(["--pid"]));

    assert!(
        result.is_err(),
        "missing selector value widened to {result:?}"
    );
}

#[rstest]
#[case(&["--pid", "42", "--pid"])]
#[case(&["--pid", "--window-id", "77"])]
#[case(&["--pid="])]
fn parser_rejects_every_malformed_selector_occurrence(#[case] arguments: &[&str]) {
    let arguments = arguments
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect::<Vec<_>>();
    let result = window_selector_query_from_flags(&arguments);

    assert!(
        result.is_err(),
        "malformed selector was accepted as {result:?}"
    );
}

#[rstest]
#[case(json!({"pid": -1, "window_id": 77}), "negative pid")]
#[case(json!({"pid": "42", "window_id": 77}), "wrong-type pid")]
#[case(json!({"pid": 42, "window_id": -1}), "negative window id")]
#[case(json!({"pid": 42, "window_id": "77"}), "wrong-type window id")]
fn inventory_resolution_rejects_non_unsigned_identity(
    #[case] identity: serde_json::Value,
    #[case] reason: &str,
) {
    let query = window_selector_query_from_flags(&strings(["--app", "fixture.exe"])).unwrap();
    let rows = vec![json!({
        "app_name": "fixture.exe",
        "pid": identity["pid"],
        "window_id": identity["window_id"],
        "title": "Fixture A",
        "is_on_screen": true
    })];

    let result = resolve_window_selector_from_inventory(&query, &rows);
    assert!(result.is_err(), "{reason} was accepted as {result:?}");
}

#[cfg(windows)]
struct SelectorTestWindow(windows_sys::Win32::Foundation::HWND);

#[cfg(windows)]
impl SelectorTestWindow {
    fn new(title: &str, x: i32) -> Self {
        use windows_sys::Win32::System::Threading::GetCurrentProcessId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        let class = "STATIC".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let title = format!("{title}-{}", unsafe { GetCurrentProcessId() })
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                x,
                80,
                280,
                180,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "create controlled selector fixture window");
        Self(hwnd)
    }

    fn id(&self) -> u64 {
        self.0 as usize as u64
    }
}

#[cfg(windows)]
impl Drop for SelectorTestWindow {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0) };
    }
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn windows_fixture_closes_app_ambiguity_with_exact_identity() {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let first = SelectorTestWindow::new("dcc-cua-selector-a", 80);
    let second = SelectorTestWindow::new("dcc-cua-selector-b", 400);
    let pid = unsafe { GetCurrentProcessId() };
    println!(
        "provider=dcc-cua runtime={} pid={pid} hwnd={}",
        env!("CARGO_PKG_VERSION"),
        first.id()
    );
    if let Ok(delay_ms) = std::env::var("DCC_CUA_SELECTOR_FIXTURE_ATTEST_DELAY_MS")
        && let Ok(delay_ms) = delay_ms.parse::<u64>()
    {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms.min(30_000)));
    }

    let driver = ComputerUseDriver::create().unwrap();
    let rows = driver.list_windows_filtered(Some(pid), true).await.unwrap();
    let fixture_rows = rows
        .into_iter()
        .filter(|row| [first.id(), second.id()].contains(&row["window_id"].as_u64().unwrap_or(0)))
        .collect::<Vec<_>>();
    assert_eq!(
        fixture_rows.len(),
        2,
        "controlled fixture windows were not enumerated"
    );
    let app = fixture_rows[0]["app_name"].as_str().unwrap().to_owned();

    let ambiguous = window_selector_query_from_flags(&strings(["--app", &app])).unwrap();
    assert!(
        resolve_window_selector_from_inventory(&ambiguous, &fixture_rows)
            .unwrap_err()
            .to_string()
            .ends_with("window, found 2")
    );

    let first_row = fixture_rows
        .iter()
        .find(|row| row["window_id"].as_u64() == Some(first.id()))
        .unwrap();
    let title = first_row["title"].as_str().unwrap();
    let exact = window_selector_query_from_flags(&strings([
        "--app",
        &app,
        "--pid",
        &pid.to_string(),
        "--window-id",
        &first.id().to_string(),
        "--title",
        title,
    ]))
    .unwrap();
    let scope = resolve_window_selector_from_inventory(&exact, &fixture_rows).unwrap();
    assert_eq!(scope.process_id, Some(pid));
    assert_eq!(scope.window_handle, Some(first.id()));
    assert_eq!(scope.window_title.as_deref(), Some(title));

    let conflicting = ComputerUseWindowQuery {
        window_title: Some("drifted fixture title".into()),
        ..exact
    };
    assert!(resolve_window_selector_from_inventory(&conflicting, &fixture_rows).is_err());
}
