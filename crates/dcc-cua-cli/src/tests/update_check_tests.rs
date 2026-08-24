use rstest::rstest;

use crate::update_check::{command_allows_check, notification};

#[rstest]
fn reminder_is_actionable_only_for_a_newer_version() {
    let notice = notification("1.3.2", "1.3.3").unwrap();
    assert!(notice.contains("1.3.2 -> 1.3.3"));
    assert!(notice.contains("dcc-cua update"));
    assert!(notification("1.3.3", "1.3.3").is_none());
    assert!(notification("1.4.0", "1.3.3").is_none());
}

#[rstest]
#[case(&["list"], true)]
#[case(&["update"], false)]
#[case(&["--version"], false)]
#[case(&["host"], false)]
#[case(&["host-jsonl"], false)]
#[case(&["mcp-server"], false)]
#[case(&["browser-extension", "status"], false)]
#[case(&["__private-worker"], false)]
fn machine_protocols_and_update_commands_skip_reminders(
    #[case] arguments: &[&str],
    #[case] expected: bool,
) {
    assert_eq!(command_allows_check(arguments), expected);
}
