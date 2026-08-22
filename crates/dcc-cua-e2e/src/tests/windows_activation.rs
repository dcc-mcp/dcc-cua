use super::*;
#[allow(unused_imports)]
use rstest::rstest;

pub(super) async fn assert_raw_click_requires_confirmation(
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
    assert_eq!(focused.value["success"], false, "{}", focused.value);
    assert_eq!(focused.value["error"], "approval_required");
    assert_eq!(focused.value["policy_tier"], "action_confirmation");
}
