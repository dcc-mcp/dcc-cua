use super::*;
use rstest::rstest;

#[rstest]
fn point_only_overlay_move_preserves_coordinates_without_inventing_target_bounds() {
    let command = upstream_overlay_command(WindowsOverlayCommand::MoveTo {
        x: -20.5,
        y: 42.0,
        heading: 0.75,
    });
    let cursor_overlay::OverlayCommand::MoveTo {
        x,
        y,
        end_heading_radians,
        target,
    } = command
    else {
        panic!("point-only movement must remain a MoveTo command");
    };
    assert_eq!((x, y, end_heading_radians), (-20.5, 42.0, 0.75));
    assert_eq!(target, None);
}

#[rstest]
fn nonmovement_overlay_commands_keep_their_payloads() {
    assert!(matches!(
        upstream_overlay_command(WindowsOverlayCommand::PinAbove(123)),
        cursor_overlay::OverlayCommand::PinAbove(123)
    ));
    let command = upstream_overlay_command(WindowsOverlayCommand::ClickPulse { x: -4.0, y: 8.0 });
    let cursor_overlay::OverlayCommand::ClickPulse { x, y } = command else {
        panic!("click pulse must remain a ClickPulse command");
    };
    assert_eq!((x, y), (-4.0, 8.0));
}
