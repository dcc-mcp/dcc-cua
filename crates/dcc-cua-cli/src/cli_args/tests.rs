use super::*;
use rstest::rstest;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[rstest]
fn only_bare_local_doctor_flag_is_accepted() {
    assert_eq!(
        diagnostics_only_requested("doctor", &strings(&["--diagnostics-only"])),
        Ok(true)
    );
    assert_eq!(
        diagnostics_only_requested("doctor", &strings(&["--diagnostics-only", "--help"])),
        Ok(true)
    );
    assert_eq!(diagnostics_only_requested("doctor", &[]), Ok(false));
    assert_eq!(diagnostics_only_requested("ping", &[]), Ok(false));
}

#[rstest]
fn non_doctor_commands_and_command_position_refuse_before_driver_creation() {
    for command in [
        "ping",
        "host",
        "mcp-server",
        "__private-worker",
        "version",
        "chrome-extension://fixture",
        "--diagnostics-only",
        "--diagnostics-only=false",
    ] {
        assert_eq!(
            diagnostics_only_requested(command, &strings(&["--diagnostics-only"])).unwrap_err(),
            "--diagnostics-only is supported only by local doctor"
        );
    }
    assert_eq!(
        diagnostics_only_requested("--diagnostics-only=false", &[]).unwrap_err(),
        "--diagnostics-only is supported only by local doctor"
    );
}

#[rstest]
fn values_duplicates_routes_and_extra_positionals_fail_closed() {
    for flags in [
        ["--diagnostics-only=true"].as_slice(),
        &["--diagnostics-only=false"],
        &["--diagnostics-only="],
        &["--diagnostics-only", "--diagnostics-only"],
    ] {
        assert_eq!(
            diagnostics_only_requested("doctor", &strings(flags)).unwrap_err(),
            "--diagnostics-only must appear once without a value"
        );
    }
    for flags in [
        ["--diagnostics-only", "--endpoint"].as_slice(),
        &["--diagnostics-only", "--endpoint=fixture"],
        &["--diagnostics-only", "--spawn"],
        &["--diagnostics-only", "--spawn=fixture"],
    ] {
        assert_eq!(
            diagnostics_only_requested("doctor", &strings(flags)).unwrap_err(),
            "--diagnostics-only cannot be combined with --endpoint or --spawn"
        );
    }
    for flags in [
        ["--diagnostics-only", "--route", "full"].as_slice(),
        &["--diagnostics-only", "extra"],
        &["--diagnostics-only", "--help=true"],
    ] {
        assert_eq!(
            diagnostics_only_requested("doctor", &strings(flags)).unwrap_err(),
            "--diagnostics-only accepts no additional probe or route arguments"
        );
    }
}
