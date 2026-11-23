use super::WindowTarget;

pub(crate) fn resolved_application_name(configured: &str, target: &WindowTarget) -> String {
    if !configured.trim().eq_ignore_ascii_case("application") && !configured.trim().is_empty() {
        return configured.trim().to_owned();
    }
    let title = target.title.trim();
    if !title.is_empty() {
        return title.to_owned();
    }
    let process = target.app_name.trim();
    process
        .strip_suffix(".exe")
        .or_else(|| process.strip_suffix(".EXE"))
        .unwrap_or(process)
        .to_owned()
}
