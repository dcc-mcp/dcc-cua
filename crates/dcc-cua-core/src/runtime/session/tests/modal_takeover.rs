use rstest::rstest;

fn owned_foreground_takeover_relation(
    target_window: u64,
    target_pid: u32,
    foreground_window: u64,
    foreground_pid: u32,
    owner_chain: &[u64],
) -> bool {
    foreground_window != 0
        && foreground_window != target_window
        && foreground_pid == target_pid
        && owner_chain.contains(&target_window)
}

#[rstest]
fn owned_same_process_foreground_requires_explicit_modal_rebind() {
    assert!(owned_foreground_takeover_relation(
        100,
        42,
        200,
        42,
        &[150, 100],
    ));
    assert!(!owned_foreground_takeover_relation(100, 42, 200, 7, &[100]));
    assert!(!owned_foreground_takeover_relation(
        100,
        42,
        200,
        42,
        &[300]
    ));
    assert!(!owned_foreground_takeover_relation(
        100,
        42,
        100,
        42,
        &[100]
    ));
}
