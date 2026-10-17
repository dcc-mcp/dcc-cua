//! End-to-end menu cancellation through the shipped MCP -> Host -> Core route.
use super::native_fixture::Fixture;
use dcc_cua_platform_windows::{UiaTarget, exact_window_native_state, read_exact_native_menu};
#[allow(unused_imports)]
use rstest::rstest;
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

mod fixture;
mod mcp;

fn content(response: &Value) -> &Value {
    assert_ne!(response["isError"], true, "{response}");
    response
        .get("structuredContent")
        .expect("structured MCP result")
}

pub(super) fn verify(binary: &Path, sentinel: &Fixture) {
    sentinel.focus_target();
    let foreground = super::foreground();
    assert_eq!(foreground as u64, sentinel.target);
    let menu = fixture::MenuFixture::new();
    let owner_before = exact_window_native_state(std::process::id(), menu.owner).unwrap();
    assert!(!owner_before.visible, "tray-style owner must remain hidden");
    menu.open();
    let deadline = Instant::now() + Duration::from_secs(5);
    let proof = loop {
        if let Some(popup) = menu.popup()
            && let Ok(proof) = read_exact_native_menu(UiaTarget {
                process_id: std::process::id(),
                window_handle: popup,
            })
        {
            break proof;
        }
        assert!(
            Instant::now() < deadline,
            "real TrackPopupMenu never established strict native authority"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(proof.owner.window_handle, menu.owner);
    assert_eq!(proof.owner.instance, owner_before.instance);
    assert_eq!(
        proof.popup.instance.owner_window_handle, 0,
        "exercise the real ownerless native popup contract"
    );
    assert_ne!(foreground as u64, proof.popup.window_handle);
    assert_eq!(
        super::foreground(),
        foreground,
        "fixture popup must not acquire foreground"
    );
    eprintln!(
        "provider=dcc-cua runtime={} target_pid={} target_hwnd={} native-menu proof={proof:?}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        proof.popup.window_handle
    );
    let mut client = mcp::Mcp::new(binary);
    let started = client.tool("start_task", json!({
        "application_label":"controlled native menu dismiss", "surface":"window", "observation_mode":"pixels_only",
        "target_process_id":std::process::id(), "target_window_handle":proof.popup.window_handle,
        "allowed_methods":["get_window_state","change_window_state"],
        "allowed_actions":[{"action":"dismiss_menu","input_kind":"window_state","secret_input":false,"authorization_category":"window_state"}]
    }));
    let started = content(&started);
    assert_eq!(started["provider"], "dcc-cua");
    let task = started["task_id"].as_str().unwrap().to_owned();
    let read = client.call(&task, "get_window_state", json!({}));
    let state = &content(&read)["state"];
    let token = state["menu_dismiss_window_state_id"]
        .as_str()
        .expect("strict native menu token");
    let before = menu.counts();
    assert_eq!(before, (0, 0, 0, 0, 0));
    // An ordinary frame token or unknown token grants no menu authority.
    let wrong = state["window_state_id"]
        .as_str()
        .unwrap_or("not-a-menu-token");
    assert_ne!(wrong, token);
    let denied = client.call(
        &task,
        "change_window_state",
        json!({"operation":"dismiss_menu","window_state_id":wrong}),
    );
    assert_eq!(denied["isError"], true, "{denied}");
    assert_eq!(menu.counts(), before);
    let read = client.call(&task, "get_window_state", json!({}));
    let token = content(&read)["state"]["menu_dismiss_window_state_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dismissed = client.call(
        &task,
        "change_window_state",
        json!({"operation":"dismiss_menu","window_state_id":token}),
    );
    let result = &content(&dismissed)["result"];
    assert_eq!(result["success"], true, "{dismissed}");
    for field in ["popup_hidden_or_gone", "menu_inactive", "owner_alive"] {
        assert_eq!(result[field], true, "{dismissed}");
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while menu.counts().3 == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        menu.counts(),
        (1, 0, 0, 1, 0),
        "one standard cancel, no menu command, key, mouse, close or selected item"
    );
    assert!(menu.popup().is_none(), "no replacement native popup");
    let owner_after = exact_window_native_state(std::process::id(), menu.owner).unwrap();
    assert_eq!(owner_after.instance, owner_before.instance);
    assert!(!owner_after.visible);
    assert_eq!(
        super::foreground(),
        foreground,
        "dismiss route activated a window"
    );
    let reuse = client.call(
        &task,
        "change_window_state",
        json!({"operation":"dismiss_menu","window_state_id":token}),
    );
    assert_eq!(reuse["isError"], true, "menu authority must be one-use");
    assert_eq!(menu.counts(), (1, 0, 0, 1, 0));
    let stopped = client.tool("stop_task", json!({"task_id":task}));
    assert_ne!(stopped["isError"], true, "{stopped}");
    client.close();
    assert_eq!(super::foreground(), foreground);
    assert_eq!(menu.counts(), (1, 0, 0, 1, 0));
    eprintln!(
        "native-menu-dismiss public MCP: strict hidden owner + unique #32768 proof; wrong-purpose zero messages; one WM_CANCELMODE; popup gone/menu inactive/owner alive; commands=0 keys/mouse/close=0 selected=0; unrelated foreground preserved; token reuse refused; task stopped and runtime exited0"
    );
}
