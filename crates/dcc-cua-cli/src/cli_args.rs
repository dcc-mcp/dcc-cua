//! Command-line argument helpers and stable help text.

pub(super) fn flag_value(flags: &[String], name: &str) -> Option<String> {
    flags
        .iter()
        .position(|flag| flag == name)
        .and_then(|index| flags.get(index + 1))
        .cloned()
}

pub(super) fn application_label(flags: &[String]) -> String {
    flag_value(flags, "--app")
        .or_else(|| flag_value(flags, "--title"))
        .unwrap_or_else(|| "Application".into())
}

pub(super) fn flag_values(flags: &[String], name: &str) -> Vec<String> {
    flags
        .windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect()
}

pub(super) fn bounded_u32(
    flags: &[String],
    name: &str,
    default: u32,
    maximum: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    let value = flag_value(flags, name)
        .map(|value| value.parse::<u32>())
        .transpose()?
        .unwrap_or(default);
    if !(1..=maximum).contains(&value) {
        return Err(format!("{name} must be between 1 and {maximum}").into());
    }
    Ok(value)
}

pub(super) fn has_flag(flags: &[String], name: &str) -> bool {
    flags.iter().any(|flag| flag == name)
}

pub(super) fn is_help_request(command: &str, flags: &[String]) -> bool {
    matches!(command, "help" | "--help" | "-h")
        || has_flag(flags, "--help")
        || has_flag(flags, "-h")
}

pub(super) fn print_help() {
    println!(
        r#"dcc-cua

  list [--app APP] [--pid PID] [--window-id ID] [--title TITLE] [--on-screen]
  wait-window --app APP|--pid PID|--window-id ID|--title TITLE [--on-screen] [--timeout-ms N] [--poll-ms N]
  apps
  tools
  call --tool NAME [--json JSON|--json-file PATH] [--app APP|--pid PID --window-id ID] [--output FILE]
  ping [--endpoint PATH|--spawn BINARY] [--agent-name NAME]
  interrupt-all [--endpoint PATH]
  host-call --method NAME [--json JSON|--json-file PATH] [--endpoint PATH|--spawn BINARY] [--agent-name NAME] [--snapshot-transport binary_frame|shared_memory] [--output FILE]
  host-batch --json JSON_ARRAY [--endpoint PATH|--spawn BINARY] [--agent-name NAME] [--snapshot-transport binary_frame|shared_memory] [--output-dir DIR]
  host-jsonl [--endpoint PATH|--spawn BINARY] [--agent-name NAME] [--parallel-discovery] [--showcase] [--showcase-dir DIR] [--snapshot-transport binary_frame|shared_memory] [--output-dir DIR] [--metrics-output FILE]
  host-ensure [--endpoint PATH] [--grant existing-profile]
  manifest
  profiles                         # list built-in and installed profile packages
  profile validate PACKAGE_DIR [--profile-store DIR]
  profile install PACKAGE_DIR [--replace] [--profile-store DIR]
  profile uninstall ID --confirm [--profile-store DIR]
  profile match --app APP [--title TITLE]
  profile --id ue|maya|fab|... [--profile-file PATH] [--app APP] [--surface ID] [--query TARGET] [--action ACTION] [--activate] [--max-elements N] [--max-depth N]
  profile-state --id PROFILE|--profile-file PATH [--source ID] [--etag ETAG] [--watch] [--poll-ms N] [--max-updates N]
  update [--check]
  desktop-snapshot [--output FILE]
  screen-size
  cursor-position
  launch --name NAME|--bundle-id ID|--aumid ID|--path PATH|--launch-path PATH [--url URL] [--arg ARG] [--new-instance] [--start-minimized]
  terminate --app APP --confirm
  snapshot --app APP|--pid PID|--window-id ID|--title TITLE [--activate] [--output FILE]
  set-window-frame --app APP|--pid PID --window-id ID --x N --y N --width N --height N
  invoke-menu --app APP|--pid PID --window-id ID --menu TOP [--menu CHILD ...]
  act --app APP --action-json JSON [--output FILE]
  verify --app APP --expect-json JSON [--timeout-ms N] [--stable-samples N]
  desktop-act --action-json JSON [--session ID] [--output FILE]
  clipboard-read --app APP [--include-text]
  clipboard-write --app APP --text TEXT|--image-path FILE|--file-path FILE
  doctor [--route full|visual|semantic] [--endpoint PATH|--spawn BINARY]
  host [--stdio|--endpoint PATH] [--grant existing-profile]

Host uses versioned big-endian JSON frames. Hello version 1 negotiates binary-frame or shared-memory snapshots and supports request_id correlation."#
    );
    println!(
        "Profiles are built-in, installed from ~/.dcc-cua/profiles, or loaded explicitly from JSON; package installation copies declarative content only and never launches bundled code. Window snapshots/actions accept --escalate --escalation-reason REASON when an explicit desktop visual fallback approval is required; --activate keeps custom-rendered foreground capture and actions in one session."
    );
    println!("Zoom: zoom --app APP --x1 N --y1 N --x2 N --y2 N [--output FILE].");
    println!(
        "Friendly actions: click [--x X --y Y|--element-index N|--element-token TOKEN] [--button left|middle|right --duration-ms N], double-click/right-click/toggle [--x X --y Y|--element-index N|--element-token TOKEN] [--button left|middle|right], drag --from-x X --from-y Y --to-x X --to-y Y [--button B --modifier M --duration-ms N --steps N], type [--text TEXT] [--focused|--x X --y Y|--element-index N], set-text/set-value, press [--key K] [--modifier M] [--x X --y Y|--element-index N], hotkey [--key K ...] [--x X --y Y], scroll [--scroll-x N|--scroll-y N] [--by line|page] [--x X --y Y|--element-index N], move."
    );
    println!(
        "Semantic tree: accessibility --app APP [--max-elements N] [--max-depth N]. Window: window-state|activate|set-window-frame|invoke-menu --app APP."
    );
}
