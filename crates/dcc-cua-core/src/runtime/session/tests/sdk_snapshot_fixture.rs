use super::*;
use rstest::rstest;

// SDK public elements omit display-only rows. Its `both` projection keeps
// the bounded text alongside exactly the same actionable token array.
pub(super) fn native_snapshot_reply(args: &Value) -> Value {
    let mut snapshot = json!({
        "snapshot_id":"sdk-snapshot",
        "elements":[{"element_index":1,"element_token":"sdk-snapshot:1","label":"txt-input"}],
        "elements_complete":false,
        "truncated":false,
    });
    if args["tree_format"] == "both" {
        snapshot["tree_markdown"] = json!(
            "- [1] text \"txt-input\"\n- label \"menu_action=none\"\n- label \"render_status=idle [9]\"\n- dialog \"Save changes?\""
        );
    }
    snapshot
}

#[rstest]
fn both_projection_keeps_the_same_tokens_and_adds_display_only_text() {
    let elements = native_snapshot_reply(&json!({"tree_format":"elements"}));
    let both = native_snapshot_reply(&json!({"tree_format":"both"}));
    assert_eq!(elements["elements"], both["elements"]);
    assert!(elements.get("tree_markdown").is_none());
    assert!(
        both["tree_markdown"]
            .as_str()
            .unwrap()
            .contains("menu_action=none")
    );
}
