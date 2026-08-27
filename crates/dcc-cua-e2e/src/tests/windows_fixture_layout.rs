use dcc_cua_client::HostClient;
#[allow(unused_imports)]
use rstest::rstest;
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use super::{client_request, windows_activation};

type Session = (usize, String, String, String, u64);
type WindowTarget = (u32, u64, String);

fn rectangles_intersect(left: [i32; 4], right: [i32; 4]) -> bool {
    let [left_x, left_y, left_width, left_height] = left;
    let [right_x, right_y, right_width, right_height] = right;
    left_x < right_x + right_width
        && left_x + left_width > right_x
        && left_y < right_y + right_height
        && left_y + left_height > right_y
}

fn controlled_wpf_frames() -> [[i32; 4]; 2] {
    let [screen_width, screen_height] =
        unsafe { [GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)] };
    const TARGET_X: i32 = 16;
    const TARGET_WIDTH: i32 = 900;
    const TASKBAR_CLEARANCE: i32 = 64;
    assert!(
        screen_width >= 960 && screen_height >= 480,
        "controlled two-window pixel E2E requires at least a 960x480 desktop; got {screen_width}x{screen_height}"
    );
    let height = (screen_height - TARGET_X * 2 - TASKBAR_CLEARANCE).min(650);
    let frames = [
        [TARGET_X, TARGET_X, TARGET_WIDTH, height],
        // Client zero is the only fixture receiving foreground raw input. Keep
        // its full 900px content visible, while retaining eight on-screen
        // pixels of the background/UIA-only peer so the exact target remains
        // live without covering any pixel of the foreground fixture.
        [screen_width - 8, TARGET_X, TARGET_WIDTH, height],
    ];
    assert!(
        !rectangles_intersect(frames[0], frames[1]),
        "controlled WPF frames must not overlap: {frames:?}"
    );
    frames
}

pub(super) fn assert_banner_visible(opened: &Value) {
    assert_eq!(
        opened["banner"]["visible"], true,
        "each independent Host must present its own DCC-CUA banner before the cross-process exact-window capture regression: {opened}"
    );
}

pub(super) fn focus_exact_window(window_targets: &[WindowTarget], window_handle: u64) -> u32 {
    let process_id = window_targets
        .iter()
        .find(|(_, candidate, _)| *candidate == window_handle)
        .map(|(process_id, _, _)| *process_id)
        .expect("active WPF process identity");
    windows_activation::physically_focus_exact_window(process_id, window_handle);
    process_id
}

pub(super) async fn arrange_and_assert_cross_host_pixels(
    clients: &mut [HostClient],
    sessions: &[Session],
    window_targets: &[WindowTarget],
) {
    let requested_frames = controlled_wpf_frames();
    let mut confirmed_frames = Vec::with_capacity(sessions.len());
    for (client_index, session_id, grant_id, capability, window_handle) in sessions {
        let requested_frame = requested_frames[*client_index];
        let [x, y, width, height] = requested_frame;
        let positioned = client_request(
            &mut clients[*client_index],
            "set_window_frame",
            json!({
                "session_id": session_id,
                "task_grant_id": grant_id,
                "window_capability": capability,
                "frame": {"x": x, "y": y, "width": width, "height": height}
            }),
        )
        .await;
        assert_eq!(
            positioned.value["result"]["success"], true,
            "{}",
            positioned.value
        );
        assert_eq!(positioned.value["result"]["effect"], "confirmed");
        assert_eq!(
            positioned.value["result"]["target"]["pid"],
            window_targets
                .iter()
                .find(|(_, candidate, _)| candidate == window_handle)
                .map(|(process_id, _, _)| *process_id)
                .expect("positioned WPF process identity"),
            "set_window_frame must retain the exact PID: {}",
            positioned.value
        );
        assert_eq!(
            positioned.value["result"]["target"]["window_id"], *window_handle,
            "set_window_frame must retain the exact HWND: {}",
            positioned.value
        );
        let confirmed = positioned.value["result"]["target"]["bounds"]
            .as_array()
            .expect("confirmed WPF bounds")
            .iter()
            .map(|value| {
                i32::try_from(value.as_i64().expect("numeric WPF bound"))
                    .expect("WPF bound fits i32")
            })
            .collect::<Vec<_>>();
        let confirmed: [i32; 4] = confirmed.try_into().expect("four WPF bounds");
        for (actual, requested) in confirmed.iter().zip(requested_frame) {
            assert!(
                (actual - requested).abs() <= 2,
                "DCC-CUA did not independently confirm the requested disposable WPF frame: {}",
                positioned.value
            );
        }
        confirmed_frames.push(confirmed);
    }
    assert!(
        !rectangles_intersect(confirmed_frames[0], confirmed_frames[1]),
        "controlled exact-window pixel fixtures still overlap after DCC-CUA confirmation: {confirmed_frames:?}"
    );
    assert_cross_host_exact_pixels(clients, sessions, window_targets).await;
}

async fn assert_cross_host_exact_pixels(
    clients: &mut [HostClient],
    sessions: &[Session],
    window_targets: &[WindowTarget],
) {
    let (client_index, session_id, grant_id, capability, window_handle) = sessions
        .iter()
        .find(|(client_index, ..)| *client_index == 0)
        .expect("first endpoint pixel session");
    focus_exact_window(window_targets, *window_handle);
    client_request(
        &mut clients[*client_index],
        "escalate_session",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "reason": "ax_tree_pixel_mismatch",
            "detail": "controlled two-Host E2E permits exact-window pixel observation"
        }),
    )
    .await;
    let snapshot = client_request(
        &mut clients[*client_index],
        "snapshot",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "max_nodes": 1_000,
            "max_depth": 20,
        }),
    )
    .await;
    assert_eq!(
        snapshot.value["observation"]["capture_backend"], "dcc-cua-visible-exact-window",
        "two same-executable roots must use independently proven visible pixels: {}",
        snapshot.value
    );
    assert_eq!(
        snapshot.value["observation"]["capture_provenance"]["fallback"],
        "same_executable_multi_window_exact_visible_proof",
        "cross-Host overlays must be excluded without widening the exact PID/HWND scope: {}",
        snapshot.value
    );
}
