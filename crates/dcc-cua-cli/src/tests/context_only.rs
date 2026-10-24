use rstest::rstest;

#[rstest]
fn context_only_short_circuits_before_platform_startup() {
    let result = crate::run_with_arguments(vec!["doctor".into(), "--context-only".into()], |_| {
        panic!("context-only entered forbidden platform/SDK/Host startup")
    });
    assert!(result.is_ok());
}

#[rstest]
#[case(vec!["doctor", "--context-only", "--diagnostics-only"])]
#[case(vec!["doctor", "--context-only", "--pid", "42"])]
#[case(vec!["doctor", "--context-only=false"])]
#[case(vec!["host", "--context-only"])]
fn context_only_invalid_arguments_never_enter_platform_startup(#[case] arguments: Vec<&str>) {
    let result =
        crate::run_with_arguments(arguments.into_iter().map(str::to_owned).collect(), |_| {
            panic!("invalid context-only arguments entered platform startup")
        });
    assert!(result.is_err());
}
