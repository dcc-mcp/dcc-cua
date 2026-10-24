use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn pixel_drag_requires_its_own_live_exact_task_scope() {
    let host = pixel_input_host(&["drag"]).await;
    let action: HostAction = serde_json::from_value(json!({
        "action":"drag", "input_kind":"raw_input", "intent":"ordinary_edit",
        "delivery_mode":"foreground", "path":[{"x":10,"y":20},{"x":30,"y":40}]
    }))
    .unwrap();
    host.require_pixels_input_grant("session-1", &action)
        .unwrap();
    assert!(
        host.require_pixels_input_grant("other-session", &action)
            .is_err()
    );
    let click_host = pixel_input_host(&["click"]).await;
    assert!(
        click_host
            .require_pixels_input_grant("session-1", &action)
            .is_err()
    );
}
