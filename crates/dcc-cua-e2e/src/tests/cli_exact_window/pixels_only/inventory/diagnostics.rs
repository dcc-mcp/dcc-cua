//! Pure reporting for the opt-in owned-window read boundary.
use super::Window;
use rstest::rstest;
use serde_json::{Value, json};

pub(super) fn inventory_report(stage: &str, pid: u32, aborted: bool, windows: &[Window]) -> String {
    let prefix = windows
        .iter()
        .filter(|w| w.identity.pid == pid)
        .take(64)
        .map(|w| {
            json!({
                "hwnd":w.identity.hwnd, "pid":w.identity.pid, "thread":w.identity.thread,
                "class":w.identity.class, "title":w.identity.title, "visible":w.visible
            })
        })
        .collect::<Vec<_>>();
    format!(
        "owned-resource inventory {}",
        json!({
            "stage":stage, "pid":pid, "enumeration_aborted":aborted,
            "successfully_read_prefix":prefix
        })
    )
}

pub(super) const TITLE_CAPACITY: usize = 256;

#[derive(Clone)]
pub(super) struct TitleRead<'a> {
    pub hwnd: u64,
    pub initial_pid: u32,
    pub initial_thread: u32,
    pub class: &'a str,
    pub delivery: isize,
    pub length: usize,
    pub win32_error: u32,
    pub final_pid: u32,
    pub final_thread: u32,
}

impl TitleRead<'_> {
    pub fn failed(&self, expected_pid: u32) -> bool {
        self.delivery == 0
            || self.length >= TITLE_CAPACITY - 1
            || self.final_pid != expected_pid
            || self.final_thread != self.initial_thread
    }

    pub fn diagnostic(&self, stage: &str, expected_pid: u32) -> Option<Value> {
        if self.initial_pid != expected_pid || !self.failed(expected_pid) {
            return None;
        }
        let reasons = [
            (self.delivery == 0, "delivery_failure"),
            (self.length >= TITLE_CAPACITY - 1, "truncation"),
            (self.final_pid != expected_pid, "pid_drift"),
            (self.final_thread != self.initial_thread, "thread_drift"),
        ]
        .into_iter()
        .filter_map(|(failed, reason)| failed.then_some(reason))
        .collect::<Vec<_>>();
        Some(json!({
            "stage":stage, "hwnd":self.hwnd,
            "initial_pid":self.initial_pid, "initial_thread":self.initial_thread,
            "initial_class":self.class, "delivery":self.delivery, "length":self.length,
            "win32_error":self.win32_error, "final_pid":self.final_pid,
            "final_thread":self.final_thread, "reasons":reasons,
            "delivery_failure_kind": if self.delivery != 0 { None } else if self.win32_error == 0 {
                Some("generic_failure")
            } else { Some("win32_error") }
        }))
    }
}

#[rstest]
fn pixels_only_read_diagnostic_retains_failure_signals() {
    // da0's real log lost these fields. Values here are a synthetic boundary
    // control, NOT a claim about its unknown failing HWND or failure branch.
    let read = TitleRead {
        hwnd: 77,
        initial_pid: 42,
        initial_thread: 10,
        class: "OwnedFixture",
        delivery: 0,
        length: 0,
        win32_error: 0,
        final_pid: 42,
        final_thread: 10,
    };
    let report = read.diagnostic("success-session-active", 42).unwrap();
    assert_eq!(
        report["hwnd"], 77,
        "actual report boundary discarded the failed owned identity"
    );
    assert_eq!(report["initial_pid"], 42);
    assert_eq!(report["delivery"], 0);
    assert_eq!(report["win32_error"], 0);
}

#[rstest]
fn pixels_only_read_diagnostic_marks_aborted_prefix() {
    // Replay da0's empty successful prefix: this is not a complete empty inventory.
    let report = inventory_report("success-session-active", 7188, true, &[]);
    assert!(report.contains("\"enumeration_aborted\":true"), "{report}");
    assert!(report.contains("\"successfully_read_prefix\":[]"));
}

#[rstest]
fn pixels_only_read_diagnostic_preserves_four_branch_decisions_and_combinations() {
    for mask in 0..16 {
        for win32_error in [0, 1460] {
            let read = TitleRead {
                hwnd: 77,
                initial_pid: 42,
                initial_thread: 10,
                class: "OwnedFixture",
                delivery: if mask & 1 != 0 { 0 } else { 1 },
                length: if mask & 2 != 0 { 255 } else { 254 },
                win32_error,
                final_pid: if mask & 4 != 0 { 43 } else { 42 },
                final_thread: if mask & 8 != 0 { 11 } else { 10 },
            };
            assert_eq!(
                read.failed(42),
                mask != 0,
                "diagnostics must not change rejection"
            );
            let report = read.diagnostic("active", 42);
            assert_eq!(report.is_some(), mask != 0);
            if let Some(report) = report {
                let expected = [
                    "delivery_failure",
                    "truncation",
                    "pid_drift",
                    "thread_drift",
                ]
                .into_iter()
                .enumerate()
                .filter_map(|(bit, reason)| (mask & (1 << bit) != 0).then_some(reason))
                .collect::<Vec<_>>();
                assert_eq!(report["reasons"], json!(expected));
                assert_eq!(report["stage"], "active");
                assert_eq!(report["initial_thread"], 10);
                assert_eq!(report["initial_class"], "OwnedFixture");
                assert_eq!(report["delivery"], read.delivery);
                assert_eq!(report["length"], read.length);
                assert_eq!(report["win32_error"], win32_error);
                assert_eq!(report["final_pid"], read.final_pid);
                assert_eq!(report["final_thread"], read.final_thread);
                assert_eq!(
                    report["delivery_failure_kind"],
                    if mask & 1 == 0 {
                        Value::Null
                    } else if win32_error == 0 {
                        json!("generic_failure")
                    } else {
                        json!("win32_error")
                    }
                );
                assert!(!report.to_string().contains("timeout"));
            }
        }
    }
}

#[rstest]
fn pixels_only_read_diagnostic_excludes_foreign_details_and_bounds_prefix() {
    // Native inspect filters owner PID BEFORE class/title reads; the reporting
    // layer also refuses foreign input rather than printing its identity.
    let read = TitleRead {
        hwnd: 999,
        initial_pid: 99,
        initial_thread: 100,
        class: "foreign-secret-class",
        delivery: 0,
        length: 0,
        win32_error: 0,
        final_pid: 99,
        final_thread: 100,
    };
    assert!(read.diagnostic("active", 42).is_none());
    let own = Window {
        identity: super::Identity {
            hwnd: 77,
            pid: 42,
            thread: 10,
            class: "OwnedFixture".into(),
            title: "owned title".into(),
        },
        visible: true,
    };
    let mut foreign = own.clone();
    foreign.identity.pid = 99;
    foreign.identity.title = "foreign-secret-title".into();
    let prefix = [vec![foreign], vec![own; 65]].concat();
    for aborted in [true, false] {
        let text = inventory_report("active", 42, aborted, &prefix);
        assert!(!text.contains("foreign-secret"));
        let report: Value =
            serde_json::from_str(text.strip_prefix("owned-resource inventory ").unwrap()).unwrap();
        assert_eq!(report["enumeration_aborted"], aborted);
        assert_eq!(
            report["successfully_read_prefix"].as_array().unwrap().len(),
            64
        );
        assert_eq!(report["successfully_read_prefix"][0]["hwnd"], 77);
    }
}
