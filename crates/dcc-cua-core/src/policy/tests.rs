use super::*;
use rstest::rstest;

#[rstest]
#[case(false)]
#[case(true)]
fn modifier_drag_keeps_its_existing_route_contract(#[case] exact_pixels: bool) {
    let action = ComputerUseAction {
        action: "drag".into(),
        x: Some(1.0),
        y: Some(2.0),
        path: vec![ComputerUsePoint { x: 3.0, y: 4.0 }],
        modifiers: vec!["Shift".into()],
        duration_ms: Some(100),
        ..Default::default()
    };
    assert!(validate_action_route(&action, exact_pixels).is_ok());
}

#[rstest]
#[case(vec!["W"], vec!["Shift"], 100, true)]
#[case(vec!["up", "left"], vec!["shift"], 10_000, true)]
#[case(vec!["W", "a"], vec![], 1, true)]
#[case(vec!["W"], vec!["Ctrl"], 100, false)]
#[case(vec!["W"], vec!["Alt"], 100, false)]
#[case(vec!["W"], vec!["Win"], 100, false)]
#[case(vec!["W"], vec!["Shift", "shift"], 100, false)]
#[case(vec!["W"], vec!["Shift", "Control"], 100, false)]
#[case(vec!["W", "w"], vec!["Shift"], 100, false)]
#[case(vec!["W", " w "], vec!["Shift"], 100, false)]
#[case(vec!["W", "A", "S"], vec!["Shift"], 100, false)]
#[case(vec!["Enter"], vec!["Shift"], 100, false)]
#[case(vec!["Shift", "W"], vec![], 100, false)]
#[case(vec!["W"], vec!["Shift"], 0, false)]
#[case(vec!["W"], vec!["Shift"], 10_001, false)]
fn held_movement_contract(
    #[case] keys: Vec<&str>,
    #[case] modifiers: Vec<&str>,
    #[case] duration: u64,
    #[case] accepted: bool,
) {
    let action = ComputerUseAction {
        action: "keypress".into(),
        keys: keys.into_iter().map(str::to_owned).collect(),
        modifiers: modifiers.into_iter().map(str::to_owned).collect(),
        duration_ms: Some(duration),
        ..Default::default()
    };
    assert_eq!(validate_action_route(&action, true).is_ok(), accepted);
    if accepted {
        let combined = keyboard_shortcut_keys(&action);
        assert_eq!(combined.len(), action.keys.len() + action.modifiers.len());
        assert!(combined.starts_with(&action.modifiers));
    }
}

#[rstest]
#[case(false)]
#[case(true)]
fn held_shift_is_closed_to_the_exact_pixel_route(#[case] exact_pixels: bool) {
    let action = ComputerUseAction {
        action: "keypress".into(),
        keys: vec!["W".into()],
        modifiers: vec!["Shift".into()],
        duration_ms: Some(100),
        ..Default::default()
    };
    assert_eq!(
        validate_action_route(&action, exact_pixels).is_ok(),
        exact_pixels
    );
}
