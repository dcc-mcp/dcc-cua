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
mod tests;
