//! Explicit metadata-only diagnostic; never opens a Host or UIA session.
use super::{positive_selector_u64, selector_value};

fn exact_selectors(flags: &[String]) -> Result<(u32, u64), Box<dyn std::error::Error>> {
    let mut index = 0;
    while index < flags.len() {
        let flag = &flags[index];
        let name = flag.split_once('=').map_or(flag.as_str(), |(name, _)| name);
        if !matches!(name, "--pid" | "--window-id") {
            return Err("capture-proof accepts only --pid and --window-id".into());
        }
        // Also reject stray positional values; selector_value validates missing/conflicting values.
        index += if flag.contains('=') { 1 } else { 2 };
    }
    for name in ["--pid", "--window-id"] {
        selector_value(flags, name)?;
    }
    let pid = positive_selector_u64(flags, "--pid")?.ok_or("capture-proof requires --pid")?;
    let hwnd =
        positive_selector_u64(flags, "--window-id")?.ok_or("capture-proof requires --window-id")?;
    Ok((u32::try_from(pid)?, hwnd))
}

pub(super) fn execute(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (pid, hwnd) = exact_selectors(flags)?;
    #[cfg(windows)]
    {
        let proof = dcc_cua_platform_windows::native_capture_proof(pid, hwnd);
        let result = serde_json::json!({
            "type": "native_capture_proof", "provider": "dcc-cua",
            "runtime_version": env!("CARGO_PKG_VERSION"), "diagnostic": proof,
        });
        stdoutln!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, hwnd);
        Err("capture-proof is available only on Windows".into())
    }
}

#[cfg(test)]
mod tests {
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
}
