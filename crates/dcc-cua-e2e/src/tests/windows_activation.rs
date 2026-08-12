use super::*;
#[allow(unused_imports)]
use rstest::rstest;

pub(super) async fn assert_first_key_reaches_retained_focus(
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
        .expect("WPF input match for first-key activation regression");
    let (input_x, input_y) = screenshot_point(&initial_snapshot.value, input);
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

    let reactivated = client_request(
        client,
        "change_window_state",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "operation": "activate"
        }),
    )
    .await;
    assert_eq!(
        reactivated.value["result"]["success"], true,
        "{}",
        reactivated.value
    );

    let select_snapshot = snapshot(client, session_id, grant_id, capability).await;
    let selected = client_request(
        client,
        "execute_action",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "observation_id": select_snapshot.value["observation_id"],
            "accessibility_state_id": select_snapshot.value["accessibility_state_id"],
            "action": {
                "action": "keyboard_shortcut",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "delivery_mode": "foreground",
                "keys": ["CTRL", "A"]
            },
            "capture_after": false
        }),
    )
    .await;
    assert_eq!(selected.value["success"], true, "{}", selected.value);

    let delete_snapshot = snapshot(client, session_id, grant_id, capability).await;
    let deleted = client_request(
        client,
        "execute_action",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "observation_id": delete_snapshot.value["observation_id"],
            "accessibility_state_id": delete_snapshot.value["accessibility_state_id"],
            "action": {
                "action": "keypress",
                "input_kind": "raw_input",
                "intent": "ordinary_edit",
                "delivery_mode": "foreground",
                "keys": ["BACKSPACE"]
            },
            "capture_after": false
        }),
    )
    .await;
    assert_eq!(deleted.value["success"], true, "{}", deleted.value);

    let cleared = client_request(
        client,
        "accessibility_snapshot",
        json!({
            "session_id": session_id,
            "task_grant_id": grant_id,
            "window_capability": capability,
            "max_nodes": 1_000,
            "max_depth": 20
        }),
    )
    .await;
    let tree = cleared.value["root"].to_string();
    assert!(
        tree.contains("mirror="),
        "the first key after activation must reach the retained WPF text focus: {}",
        cleared.value
    );
    assert!(
        !tree.contains("windows-background-uia-e2e-0")
            && !tree.contains("windows-background-uia-e2e-1"),
        "Ctrl+A must not be consumed by foreground activation: {}",
        cleared.value
    );
}

async fn snapshot(
    client: &mut HostClient,
    session_id: &str,
    grant_id: &str,
    capability: &str,
) -> HostResponse {
    client_request(
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
    .await
}
