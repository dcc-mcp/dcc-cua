use super::exact_selectors;
use rstest::rstest;

#[rstest]
#[case(&["--pid", "42", "--window-id", "123"], Some((42, 123)))]
#[case(&["--pid=42", "--window-id=123"], Some((42, 123)))]
#[case(&["--pid", "42", "--pid", "43", "--window-id", "123"], None)]
#[case(&["--pid", "0", "--window-id", "123"], None)]
#[case(&["--pid", "4294967296", "--window-id", "123"], None)]
#[case(&["--pid", "42", "--window-id", "0"], None)]
#[case(&["--pid", "42"], None)]
#[case(&["--pid", "42", "--window-id"], None)]
#[case(&["--pid", "42", "--window-id", "123", "--activate"], None)]
#[case(&["--pid", "42", "--window-id", "123", "--output", "some-path"], None)]
#[case(&["--pid", "42", "--window-id", "123", "--app", "other"], None)]
#[case(&["--pid", "42", "--window-id", "123", "stray"], None)]
fn capture_proof_requires_only_exact_numeric_binding(
    #[case] values: &[&str],
    #[case] expected: Option<(u32, u64)>,
) {
    let flags = values
        .iter()
        .map(|value| (*value).into())
        .collect::<Vec<_>>();
    assert_eq!(exact_selectors(&flags).ok(), expected);
}

#[rstest]
fn capture_proof_manifest_is_windows_only_and_never_authorizes_input() {
    let windows = crate::manifest::document_for_platform(true);
    let contract = &windows["runtime"]["native_capture_proof"];
    assert_eq!(
        contract["required_selectors"],
        serde_json::json!(["--pid", "--window-id"])
    );
    for field in [
        "pixels_read",
        "input_sent",
        "host_or_uia_started",
        "authorizes_capture_or_input",
    ] {
        assert_eq!(contract[field], false);
    }
    assert!(
        crate::manifest::document_for_platform(false)["runtime"]
            .get("native_capture_proof")
            .is_none()
    );
}
