use rstest::rstest;

use super::*;

#[rstest]
fn profile_key_binding_becomes_a_fenced_foreground_action() {
    let action = key_binding_action(&["SPACE".into()], "observation-1");
    assert_eq!(action.action, "keypress");
    assert_eq!(action.keys, ["SPACE"]);
    assert_eq!(action.delivery_mode.as_deref(), Some("foreground"));
    assert_eq!(action.observation_id.as_deref(), Some("observation-1"));
}
