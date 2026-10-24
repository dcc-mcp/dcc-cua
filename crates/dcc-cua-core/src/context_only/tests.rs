use std::cell::Cell;

use rstest::rstest;
use serde_json::json;

use super::{ContextProbe, collect, report};

struct MetadataProbe {
    desktop: Result<Option<String>, String>,
    receives: Result<bool, String>,
    birth: Result<String, String>,
    calls: Cell<usize>,
}

impl MetadataProbe {
    fn read<T>(&self, value: T) -> T {
        self.calls.set(self.calls.get() + 1);
        value
    }

    fn cursor_position(&self) -> ! {
        panic!("forbidden cursor position query")
    }
    fn cursor_info(&self) -> ! {
        panic!("forbidden cursor info query")
    }
    fn input_desktop(&self) -> ! {
        panic!("forbidden input desktop query")
    }
    fn foreground(&self) -> ! {
        panic!("forbidden foreground query")
    }
    fn sdk_or_host(&self) -> ! {
        panic!("forbidden SDK/Host construction")
    }
    fn capture_or_input(&self) -> ! {
        panic!("forbidden capture/input operation")
    }
}

impl ContextProbe for MetadataProbe {
    fn process_id(&self) -> u32 {
        self.read(7)
    }
    fn thread_id(&self) -> u32 {
        self.read(9)
    }
    fn process_birth(&self) -> Result<String, String> {
        self.read(self.birth.clone())
    }
    fn session_id(&self) -> Result<u32, String> {
        self.read(Ok(1))
    }
    fn window_station(&self) -> Result<Option<String>, String> {
        self.read(Ok(Some("WinSta0".into())))
    }
    fn thread_desktop(&self) -> Result<Option<String>, String> {
        self.read(self.desktop.clone())
    }
    fn receives_input(&self) -> Result<bool, String> {
        self.read(self.receives.clone())
    }
}

#[rstest]
#[case(Ok(Some("Default".into())), Ok(true), "collected")]
#[case(Ok(Some("IsolatedDesktop".into())), Ok(false), "collected")]
#[case(Ok(None), Ok(false), "partial")]
#[case(Err("metadata denied".into()), Err("UOI_IO denied".into()), "partial")]
fn context_only_metadata_never_attests_readiness(
    #[case] desktop: Result<Option<String>, String>,
    #[case] receives: Result<bool, String>,
    #[case] status: &str,
) {
    let probe = MetadataProbe {
        desktop,
        receives: receives.clone(),
        birth: Ok("134057601234567891".into()),
        calls: Cell::new(0),
    };
    let result = report(collect(&probe));
    assert_eq!(
        probe.calls.get(),
        7,
        "only the seven borrowed/identity metadata operations"
    );
    assert_eq!(result["checks"]["execution_context"]["status"], status);
    let context = &result["checks"]["execution_context"]["data"];
    assert_eq!(
        context["process_birth"]["windows_filetime_100ns"],
        "134057601234567891"
    );
    assert_eq!(
        context["thread_desktop"]["receives_input"],
        json!(receives.as_ref().ok())
    );
    assert_eq!(
        context["thread_desktop"]["receives_input_error"],
        json!(receives.as_ref().err())
    );
    assert!(context.get("input_desktop").is_none());
    assert_eq!(result["ready"], false);
    assert_eq!(result["input_ready"], false);
    assert_eq!(result["readiness_assessed"], false);
    assert_eq!(result["native_acceptance"], "not_run");
    assert!(result.get("retry_safe").is_none());
    for check in [
        "input_desktop",
        "input_surface",
        "foreground_window",
        "sdk_constructor",
        "capture",
        "recording",
    ] {
        assert_eq!(result["checks"][check]["status"], "not_run");
    }
}

#[rstest]
fn context_only_unknown_birth_and_launcher_evidence_remain_unknown() {
    let probe = MetadataProbe {
        desktop: Ok(Some("Default".into())),
        receives: Ok(true),
        birth: Err("birth unavailable".into()),
        calls: Cell::new(0),
    };
    let result = report(collect(&probe));
    assert_eq!(result["checks"]["execution_context"]["status"], "partial");
    assert!(
        result["checks"]["execution_context"]["data"]["process_birth"]["windows_filetime_100ns"]
            .is_null()
    );
    assert_eq!(
        result["checks"]["execution_context"]["data"]["process_birth"]["error"],
        "birth unavailable"
    );
    assert_eq!(result["agent_host_binding"]["status"], "not_established");
    assert!(
        !result["agent_host_binding"]["missing_evidence"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn context_only_native_capability_modules_exclude_input_surface_dependencies() {
    for source in [
        include_str!("windows.rs"),
        include_str!("../desktop_context.rs"),
    ] {
        for forbidden in [
            "GetCursorPos",
            "GetCursorInfo",
            "OpenInputDesktop",
            "GetForegroundWindow",
            "SetThreadDesktop",
            "SwitchDesktop",
            "OpenDesktop",
            "ComputerUseDriver",
        ] {
            assert!(
                !source.contains(forbidden),
                "context-only capability module includes {forbidden}"
            );
        }
    }
}

#[rstest]
fn context_only_fake_forbidden_operations_are_fail_fast_and_unavailable_to_collector() {
    let probe = MetadataProbe {
        desktop: Ok(Some("IsolatedDesktop".into())),
        receives: Ok(false),
        birth: Ok("134057601234567891".into()),
        calls: Cell::new(0),
    };
    // The collector receives only ContextProbe, which cannot express these operations.
    // Verify the full fake would fail immediately if any forbidden capability were used.
    let forbidden: [fn(&MetadataProbe) -> !; 6] = [
        MetadataProbe::cursor_position,
        MetadataProbe::cursor_info,
        MetadataProbe::input_desktop,
        MetadataProbe::foreground,
        MetadataProbe::sdk_or_host,
        MetadataProbe::capture_or_input,
    ];
    for operation in forbidden {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(&probe))).is_err()
        );
    }
    assert_eq!(probe.calls.get(), 0);
    let result = report(collect(&probe));
    assert_eq!(probe.calls.get(), 7);
    assert_eq!(result["readiness_assessed"], false);
}

#[cfg(not(windows))]
#[rstest]
fn context_only_unsupported_platform_does_not_claim_windows_metadata() {
    let result = super::context_only_diagnostics();
    assert_eq!(result["checks"]["execution_context"]["status"], "not_run");
    assert_eq!(result["readiness_assessed"], false);
}
