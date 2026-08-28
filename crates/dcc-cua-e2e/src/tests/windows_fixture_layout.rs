use dcc_cua_client::HostClient;
#[allow(unused_imports)]
use rstest::rstest;
use serde_json::{Value, json};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use super::{client_request, windows_activation};

type Session = (usize, String, String, String, u64);
type WindowTarget = (u32, u64, String, String);

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
    controlled_wpf_frames_for_screen(screen_width, screen_height)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn controlled_wpf_frames_for_screen(
    screen_width: i32,
    screen_height: i32,
) -> Result<[[i32; 4]; 2], String> {
    const TARGET_X: i32 = 16;
    const TARGET_WIDTH: i32 = 900;
    const TASKBAR_CLEARANCE: i32 = 64;
    if screen_width < 960 || screen_height < 480 {
        return Err(format!(
            "controlled two-window pixel E2E requires at least a 960x480 desktop; got {screen_width}x{screen_height}"
        ));
    }
    let height = (screen_height - TARGET_X * 2 - TASKBAR_CLEARANCE).min(650);
    let frames = [
        [TARGET_X, TARGET_X, TARGET_WIDTH, height],
        // Client zero is the only fixture receiving foreground raw input. Keep
        // its full 900px content visible, while retaining eight on-screen
        // pixels of the background/UIA-only peer so the exact target remains
        // live without covering any pixel of the foreground fixture.
        [screen_width - 8, TARGET_X, TARGET_WIDTH, height],
    ];
    if rectangles_intersect(frames[0], frames[1]) {
        return Err(format!(
            "controlled WPF frames must not overlap: {frames:?}"
        ));
    }
    Ok(frames)
}

pub(super) fn assert_banner_visible(opened: &Value) {
    assert_eq!(
        opened["banner"]["visible"], true,
        "each independent Host must present its own DCC-CUA banner before the cross-process exact-window capture regression: {opened}"
    );
}

pub(super) fn focus_exact_window(window_targets: &[WindowTarget], window_handle: u64) -> u32 {
    focus_exact_window_with(
        window_targets,
        window_handle,
        windows_activation::observe_window_binding,
        windows_activation::foreground_window_binding,
        windows_activation::request_physical_focus_exact_window,
        || std::thread::sleep(std::time::Duration::from_millis(50)),
        61,
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

pub(super) struct DescendantFocusCase<'a> {
    pub process_id: u32,
    pub window_handle: u64,
    pub session_id: &'a str,
    pub grant_id: &'a str,
    pub capability: &'a str,
    pub initial_snapshot: &'a super::HostResponse,
    pub automation_id: &'a str,
    pub key: &'a str,
    pub expected_text: &'a str,
}

pub(super) async fn assert_descendant_focus_survives_exact_focus_fence(
    client: &mut HostClient,
    window_targets: &[WindowTarget],
    case: DescendantFocusCase<'_>,
) {
    let DescendantFocusCase {
        process_id,
        window_handle,
        session_id,
        grant_id,
        capability,
        initial_snapshot,
        automation_id,
        key,
        expected_text,
    } = case;
    focus_exact_window(window_targets, window_handle);
    windows_activation::assert_ordinary_raw_click_focuses_control(
        client,
        session_id,
        grant_id,
        capability,
        initial_snapshot,
        automation_id,
    )
    .await;
    windows_activation::assert_exact_foreground_window(process_id, window_handle);

    let key_snapshot = client_request(
        client,
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
    focus_exact_window(window_targets, window_handle);
    let typed = client_request(
        client,
        "execute_action",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "observation_id": key_snapshot.value["observation_id"],
            "accessibility_state_id": key_snapshot.value["accessibility_state_id"],
            "action": {
                "action": "keypress",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "delivery_mode": "foreground",
                "keys": [key]
            },
            "capture_after": false
        }),
    )
    .await;
    assert_eq!(typed.value["success"], true, "{}", typed.value);
    assert_eq!(typed.value["policy_tier"], "task_grant");
    windows_activation::assert_exact_foreground_window(process_id, window_handle);
    let verified = client_request(
        client,
        "wait_for",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "condition": {
                "kind": "text_contains",
                "text": expected_text,
                "timeout_ms": 30_000,
                "interval_ms": 100
            }
        }),
    )
    .await;
    assert_eq!(
        verified.value["success"], true,
        "exact focus fence did not preserve descendant {automation_id:?}: {}",
        verified.value
    );
}

fn focus_exact_window_with<ObserveTarget, ObserveForeground, Request, Pause>(
    window_targets: &[WindowTarget],
    window_handle: u64,
    mut observe_target: ObserveTarget,
    mut observe_foreground: ObserveForeground,
    mut request_focus: Request,
    mut pause: Pause,
    observation_limit: usize,
) -> Result<u32, String>
where
    ObserveTarget: FnMut(u64) -> Result<windows_activation::WindowBinding, String>,
    ObserveForeground: FnMut() -> Result<Option<windows_activation::WindowBinding>, String>,
    Request: FnMut(&windows_activation::WindowBinding) -> Result<(), String>,
    Pause: FnMut(),
{
    let mut matches = window_targets
        .iter()
        .filter(|(_, candidate, _, _)| *candidate == window_handle);
    let (process_id, _, title, app_name) = matches.next().ok_or_else(|| {
        format!(
            "controlled exact-window focus target is absent before raw input: expected_hwnd={window_handle}"
        )
    })?;
    if matches.next().is_some() {
        return Err(format!(
            "controlled exact-window focus target is ambiguous before raw input: expected_hwnd={window_handle}"
        ));
    }
    if *process_id == 0 || window_handle == 0 {
        return Err(format!(
            "controlled exact-window focus target is invalid before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
        ));
    }
    if observation_limit == 0 {
        return Err(format!(
            "controlled exact-window focus has no observation budget before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
        ));
    }

    let captured_target = observe_target(window_handle)?;
    if !captured_target.matches_inventory(*process_id, window_handle, title, app_name) {
        return Err(format!(
            "controlled exact-window focus target title/app drifted before raw input: expected_pid={process_id} expected_hwnd={window_handle} expected_title={title:?} expected_app={app_name:?} observed_pid={} observed_hwnd={} observed_title={:?} observed_app={:?}",
            captured_target.process_id,
            captured_target.window_handle,
            captured_target.title,
            captured_target.app_name
        ));
    }
    let first_foreground = observe_foreground()?;
    if first_foreground.as_ref() == Some(&captured_target) {
        return Ok(*process_id);
    }

    request_focus(&captured_target)?;
    let mut last_observed = first_foreground;
    for observation in 1..=observation_limit {
        let current_target = observe_target(window_handle)?;
        if current_target != captured_target {
            return Err(format!(
                "controlled exact-window focus target title/app/instance drifted after focus request before raw input: expected_pid={process_id} expected_hwnd={window_handle}"
            ));
        }
        last_observed = observe_foreground()?;
        if last_observed.as_ref() == Some(&captured_target) {
            return Ok(*process_id);
        }
        if observation < observation_limit {
            pause();
        }
    }
    let (observed_pid, observed_hwnd) = last_observed
        .as_ref()
        .map(|binding| (binding.process_id, binding.window_handle))
        .unwrap_or((0, 0));
    Err(format!(
        "controlled exact-window focus timed out before raw input: expected_pid={process_id} expected_hwnd={window_handle} observed_pid={} observed_hwnd={} observations={observation_limit}",
        observed_pid, observed_hwnd
    ))
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
                .find(|(_, candidate, _, _)| candidate == window_handle)
                .map(|(process_id, _, _, _)| *process_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;

    fn binding(
        process_id: u32,
        window_handle: u64,
        title: &str,
        app_name: &str,
        instance: u64,
    ) -> windows_activation::WindowBinding {
        windows_activation::WindowBinding {
            process_id,
            window_handle,
            title: title.to_owned(),
            app_name: app_name.to_owned(),
            process_creation_time_100ns: instance,
            window_thread_id: 7,
            window_class_hash: 11,
            owner_window_handle: 12,
            window_user_data: 13,
        }
    }

    #[rstest]
    fn exact_foreground_preserves_descendant_focus_without_activation_or_click() {
        let exact = binding(4_101, 0x101, "exact", "fixture.exe", 1);
        let targets = [(
            4_101,
            0x101_u64,
            "exact".to_owned(),
            "fixture.exe".to_owned(),
        )];
        let requests = Cell::new(0);

        let focused_pid = focus_exact_window_with(
            &targets,
            0x101,
            |_| Ok(exact.clone()),
            || Ok(Some(exact.clone())),
            |_| {
                requests.set(requests.get() + 1);
                Ok(())
            },
            || {},
            3,
        )
        .expect("already-foreground exact target");

        assert_eq!(focused_pid, 4_101);
        assert_eq!(requests.get(), 0, "already-foreground must be action-free");
    }

    #[rstest]
    fn cross_host_pixels_focus_exact_target_before_raw_input() {
        let frames = controlled_wpf_frames_for_screen(1_920, 1_080).expect("fixture frames");
        assert!(!rectangles_intersect(frames[0], frames[1]));
        assert_eq!(frames[0], [16, 16, 900, 650]);
        assert_eq!(frames[1], [1_912, 16, 900, 650]);

        let targets = [
            (
                4_101,
                0x101_u64,
                "exact".to_owned(),
                "fixture.exe".to_owned(),
            ),
            (
                4_202,
                0x202_u64,
                "decoy".to_owned(),
                "fixture.exe".to_owned(),
            ),
        ];
        let exact = binding(4_101, 0x101, "exact", "fixture.exe", 1);
        let decoy = binding(4_202, 0x202, "decoy", "fixture.exe", 2);
        let requested = Cell::new(None);
        let trace = std::cell::RefCell::new(Vec::new());
        let mut observations =
            VecDeque::from([Some(decoy.clone()), Some(decoy), Some(exact.clone())]);
        let pauses = Cell::new(0);

        let focused_pid = focus_exact_window_with(
            &targets,
            0x101,
            |_| {
                trace.borrow_mut().push("target");
                Ok(exact.clone())
            },
            || {
                trace.borrow_mut().push("foreground");
                Ok(observations
                    .pop_front()
                    .expect("scripted foreground observation"))
            },
            |target| {
                trace.borrow_mut().push("request");
                requested.set(Some((target.process_id, target.window_handle)));
                Ok(())
            },
            || pauses.set(pauses.get() + 1),
            3,
        )
        .expect("exact disposable target becomes foreground");

        assert_eq!(requested.get(), Some((4_101, 0x101)));
        assert_eq!(focused_pid, 4_101);
        assert_eq!(pauses.get(), 1);
        assert_eq!(
            *trace.borrow(),
            [
                "target",
                "foreground",
                "request",
                "target",
                "foreground",
                "target",
                "foreground"
            ],
            "fresh target and foreground observations must fence the caption request"
        );
    }

    #[rstest]
    fn cross_host_pixels_refuse_wrong_foreground_before_raw_input() {
        let frames = controlled_wpf_frames_for_screen(1_920, 1_080).expect("fixture frames");
        assert!(!rectangles_intersect(frames[0], frames[1]));

        let targets = [
            (
                4_101,
                0x101_u64,
                "exact".to_owned(),
                "fixture.exe".to_owned(),
            ),
            (
                4_202,
                0x202_u64,
                "decoy".to_owned(),
                "fixture.exe".to_owned(),
            ),
        ];
        let exact = binding(4_101, 0x101, "exact", "fixture.exe", 1);
        let decoy = binding(4_202, 0x202, "decoy", "fixture.exe", 2);
        let requested = Cell::new(None);
        let raw_input_attempts = Cell::new(0);

        let error = focus_exact_window_with(
            &targets,
            0x101,
            |_| Ok(exact.clone()),
            || Ok(Some(decoy.clone())),
            |target| {
                requested.set(Some((target.process_id, target.window_handle)));
                Ok(())
            },
            || {},
            3,
        )
        .inspect(|_| {
            raw_input_attempts.set(raw_input_attempts.get() + 1);
        })
        .expect_err("decoy foreground must fail closed");

        assert_eq!(requested.get(), Some((4_101, 0x101)));
        assert_eq!(raw_input_attempts.get(), 0);
        assert_eq!(
            error,
            "controlled exact-window focus timed out before raw input: expected_pid=4101 expected_hwnd=257 observed_pid=4202 observed_hwnd=514 observations=3"
        );
    }

    #[rstest]
    fn same_pid_hwnd_title_or_app_replacement_fails_before_focus_request() {
        let targets = [(
            4_101,
            0x101_u64,
            "exact".to_owned(),
            "fixture.exe".to_owned(),
        )];
        let replaced = binding(4_101, 0x101, "replacement", "other.exe", 1);
        let requests = Cell::new(0);

        let error = focus_exact_window_with(
            &targets,
            0x101,
            |_| Ok(replaced.clone()),
            || Ok(None),
            |_| {
                requests.set(requests.get() + 1);
                Ok(())
            },
            || {},
            3,
        )
        .expect_err("title/app replacement must fail closed");

        assert_eq!(requests.get(), 0);
        assert!(
            error.contains("target title/app drifted before raw input"),
            "{error}"
        );
    }

    #[rstest]
    fn same_numeric_target_instance_replacement_fails_after_caption_request() {
        let targets = [(
            4_101,
            0x101_u64,
            "exact".to_owned(),
            "fixture.exe".to_owned(),
        )];
        let exact = binding(4_101, 0x101, "exact", "fixture.exe", 1);
        let replacement = binding(4_101, 0x101, "exact", "fixture.exe", 2);
        let mut target_observations = VecDeque::from([exact.clone(), replacement]);
        let requests = Cell::new(0);

        let error = focus_exact_window_with(
            &targets,
            0x101,
            |_| Ok(target_observations.pop_front().expect("target observation")),
            || Ok(None),
            |_| {
                requests.set(requests.get() + 1);
                Ok(())
            },
            || {},
            3,
        )
        .expect_err("same numeric HWND with a new instance must fail closed");

        assert_eq!(requests.get(), 1);
        assert!(
            error.contains("title/app/instance drifted after focus request"),
            "{error}"
        );
    }
}
