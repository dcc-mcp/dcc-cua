use super::*;
use rstest::rstest;

#[rstest]
fn window_tool_scroll_contract_maps_to_the_typed_action() {
    let action = action_from_tool_call(
        "scroll",
        &serde_json::json!({
            "direction": "down",
            "amount": 6,
            "by": "line",
            "delivery_mode": "background",
            "x": 1600,
            "y": 780
        }),
    )
    .unwrap()
    .unwrap();

    assert_eq!(action.action, "scroll");
    assert_eq!((action.scroll_x, action.scroll_y), (None, Some(6)));
    assert_eq!(action.scroll_by.as_deref(), Some("line"));
    assert_eq!(action.delivery_mode.as_deref(), Some("background"));
    assert_eq!((action.x, action.y), (Some(1600.0), Some(780.0)));
}

#[rstest]
fn window_tool_drag_contract_maps_to_the_typed_action() {
    let action = action_from_tool_call(
        "drag",
        &serde_json::json!({
            "from_x": 10,
            "from_y": 20,
            "to_x": 30,
            "to_y": 40,
            "duration_ms": 750,
            "steps": 32,
            "modifier": ["ALT"],
            "button": "middle",
            "delivery_mode": "foreground"
        }),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        action.path[0],
        dcc_cua_core::ComputerUsePoint { x: 10.0, y: 20.0 }
    );
    assert_eq!(
        action.path[1],
        dcc_cua_core::ComputerUsePoint { x: 30.0, y: 40.0 }
    );
    assert_eq!(action.modifiers, ["ALT"]);
    assert_eq!(action.steps, Some(32));
}

#[rstest]
fn raw_action_json_normalizes_legacy_wheel_delta_and_drag_endpoints() {
    let scroll = action_from_json(
        &serde_json::json!({"action": "scroll", "scroll_x": 0, "scroll_y": 756}).to_string(),
    )
    .unwrap();
    assert_eq!(scroll.scroll_y, Some(6));

    let drag = action_from_json(
        &serde_json::json!({
            "action": "drag",
            "from_x": 1,
            "from_y": 2,
            "to_x": 3,
            "to_y": 4
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(drag.path.len(), 2);
    assert_eq!(
        drag.path[1],
        dcc_cua_core::ComputerUsePoint { x: 3.0, y: 4.0 }
    );
}
