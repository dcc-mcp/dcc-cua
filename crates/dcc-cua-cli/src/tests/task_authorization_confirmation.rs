#[cfg(windows)]
use rstest::rstest;

#[cfg(windows)]
#[rstest]
fn injected_keyboard_events_cannot_advance_physical_confirmation() {
    use windows::Win32::UI::WindowsAndMessaging::{LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED};

    use crate::task_authorization_confirmation::{
        PHYSICAL_CONFIRMATION_KEYS, physical_confirmation_transition,
    };

    assert_eq!(
        physical_confirmation_transition(0, PHYSICAL_CONFIRMATION_KEYS[0], LLKHF_INJECTED.0),
        (0, false)
    );
    assert_eq!(
        physical_confirmation_transition(0, PHYSICAL_CONFIRMATION_KEYS[0], 0),
        (1, true)
    );
    assert_eq!(
        physical_confirmation_transition(
            1,
            PHYSICAL_CONFIRMATION_KEYS[1],
            LLKHF_LOWER_IL_INJECTED.0,
        ),
        (1, false)
    );
    assert_eq!(
        physical_confirmation_transition(1, PHYSICAL_CONFIRMATION_KEYS[1], 0),
        (2, true)
    );
    assert_eq!(
        physical_confirmation_transition(2, PHYSICAL_CONFIRMATION_KEYS[2], 0),
        (3, true)
    );
    assert_eq!(physical_confirmation_transition(0, 0x41, 0), (0, false));
    assert_eq!(
        physical_confirmation_transition(2, PHYSICAL_CONFIRMATION_KEYS[0], 0),
        (1, true)
    );
}
