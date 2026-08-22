use rstest::rstest;

use super::*;

#[rstest]
fn generation_change_only_interrupts_existing_sessions() {
    assert!(!interrupt_generation_changed(7, 7));
    assert!(interrupt_generation_changed(7, 8));
}

#[rstest]
fn broadcast_advances_the_process_generation() {
    let started = interrupt_generation();
    let current = broadcast_interrupt();
    assert!(interrupt_generation_changed(started, current));
}
