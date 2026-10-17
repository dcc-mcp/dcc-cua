use rstest::rstest;

fn strings(arguments: &[&str]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| (*argument).into())
        .collect()
}

#[rstest]
fn diagnostics_only_requires_one_bare_local_doctor_option() {
    assert_eq!(
        super::diagnostics_only_requested("doctor", &strings(&["--diagnostics-only"])),
        Ok(true)
    );
    for arguments in [
        &["doctor"][..],
        &["doctor", "--route", "visual"][..],
        &["host", "--stdio"][..],
    ] {
        assert_eq!(
            super::diagnostics_only_requested(arguments[0], &strings(&arguments[1..])),
            Ok(false)
        );
    }
}

#[rstest]
#[case(&["doctor", "--diagnostics-only", "--diagnostics-only"])]
#[case(&["doctor", "--diagnostics-only=true"])]
#[case(&["doctor", "--diagnostics-only="])]
#[case(&["doctor", "--diagnostics-only", "true"])]
#[case(&["doctor", "--diagnostics-only", "--endpoint", "endpoint"])]
#[case(&["doctor", "--diagnostics-only", "--spawn", "host"])]
#[case(&["doctor", "--diagnostics-only", "--route", "visual"])]
#[case(&["doctor", "--diagnostics-only", "--help"])]
#[case(&["doctor", "--diagnostics-only", "unexpected"])]
#[case(&["host", "--diagnostics-only"])]
#[case(&["version", "--diagnostics-only"])]
#[case(&["--diagnostics-only"])]
#[case(&["--diagnostics-only=true"])]
#[case(&["__capture-preparation-supervisor-v1", "--diagnostics-only"])]
fn diagnostics_only_rejects_arguments_before_platform_or_runtime_start(#[case] arguments: &[&str]) {
    let result = crate::run_with_arguments(strings(arguments), |_| {
        panic!("invalid diagnostics-only arguments reached platform or SDK startup")
    });

    assert!(result.is_err());
}

#[rstest]
fn diagnostics_only_finishes_with_no_platform_or_sdk_constructor_available() {
    crate::run_with_arguments(strings(&["doctor", "--diagnostics-only"]), |_| {
        panic!("diagnostics-only reached platform or SDK startup")
    })
    .unwrap();
}
