//! Command-line argument helpers and stable help text.

use dcc_cua_core::{COMPUTER_USE_ESCALATION_REASONS, MAX_ESCALATION_DETAIL_CHARS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnapshotMode {
    AccessibilityPreferred,
    PixelsOnly,
}

pub(super) fn snapshot_mode(flags: &[String]) -> Result<SnapshotMode, Box<dyn std::error::Error>> {
    if !has_flag(flags, "--pixels-only") {
        return Ok(SnapshotMode::AccessibilityPreferred);
    }
    if flag_value(flags, "--pid").is_none() || flag_value(flags, "--window-id").is_none() {
        return Err(
            "snapshot --pixels-only requires an exact --pid PID --window-id ID pair".into(),
        );
    }
    if has_flag(flags, "--activate") || has_flag(flags, "--escalate") {
        return Err(
            "snapshot --pixels-only is read-only and cannot be combined with --activate or --escalate"
                .into(),
        );
    }
    Ok(SnapshotMode::PixelsOnly)
}

const KNOWN_FLAG_NAMES: &[&str] = &[
    "--action",
    "--action-json",
    "--activate",
    "--agent-name",
    "--app",
    "--arg",
    "--aumid",
    "--bundle-id",
    "--button",
    "--browser",
    "--by",
    "--check",
    "--confirm",
    "--cdp-state",
    "--delay-ms",
    "--delivery-mode",
    "--duration-ms",
    "--element-index",
    "--element-token",
    "--endpoint",
    "--escalate",
    "--escalation-detail",
    "--escalation-reason",
    "--etag",
    "--extension-id",
    "--expect-json",
    "--file-path",
    "--focused",
    "--from-x",
    "--from-y",
    "--generation",
    "--grant",
    "--height",
    "--help",
    "--id",
    "--identity",
    "--image-path",
    "--include-text",
    "--json",
    "--json-file",
    "--key",
    "--knowledge-root",
    "--launch-path",
    "--max-depth",
    "--max-elements",
    "--max-updates",
    "--menu",
    "--method",
    "--metrics-output",
    "--modifier",
    "--name",
    "--new-instance",
    "--observation-height",
    "--observation-width",
    "--on-screen",
    "--output",
    "--output-dir",
    "--parallel-discovery",
    "--path",
    "--pid",
    "--pixels-only",
    "--poll-ms",
    "--profile-file",
    "--profile-store",
    "--query",
    "--replace",
    "--response-format",
    "--route",
    "--scroll-x",
    "--scroll-y",
    "--selector",
    "--session",
    "--showcase",
    "--showcase-dir",
    "--snapshot-transport",
    "--source",
    "--spawn",
    "--stable-samples",
    "--start-minimized",
    "--state",
    "--stdio",
    "--steps",
    "--surface",
    "--text",
    "--timeout-ms",
    "--title",
    "--tool",
    "--to-x",
    "--to-y",
    "--url",
    "--value",
    "--watch",
    "--width",
    "--window-id",
    "--x",
    "--x1",
    "--x2",
    "--y",
    "--y1",
    "--y2",
    "-h",
];

pub(super) fn reject_unknown_flags(flags: &[String]) -> Result<(), String> {
    for argument in flags {
        let Some(name) = (argument.starts_with("--") || argument == "-h").then(|| {
            argument
                .split_once('=')
                .map_or(argument.as_str(), |(name, _)| name)
        }) else {
            continue;
        };
        if !KNOWN_FLAG_NAMES.contains(&name) {
            return Err("unknown option; use `help` to list supported options".into());
        }
    }
    Ok(())
}

pub(super) fn flag_value(flags: &[String], name: &str) -> Option<String> {
    flags.iter().enumerate().find_map(|(index, flag)| {
        if flag == name {
            flags.get(index + 1).cloned()
        } else {
            inline_flag_value(flag, name).map(str::to_owned)
        }
    })
}

pub(super) fn application_label(flags: &[String]) -> String {
    flag_value(flags, "--app")
        .or_else(|| flag_value(flags, "--title"))
        .unwrap_or_else(|| "Application".into())
}

pub(super) fn flag_values(flags: &[String], name: &str) -> Vec<String> {
    flags
        .iter()
        .enumerate()
        .filter_map(|(index, flag)| {
            if flag == name {
                flags.get(index + 1).cloned()
            } else {
                inline_flag_value(flag, name).map(str::to_owned)
            }
        })
        .collect()
}

pub(super) fn checked_flag_values(flags: &[String], name: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    for (index, flag) in flags.iter().enumerate() {
        let value = if flag == name {
            let value = flags
                .get(index + 1)
                .ok_or_else(|| format!("{name} requires a value"))?;
            if is_known_flag(value) {
                return Err(format!("{name} requires a value"));
            }
            value.as_str()
        } else if let Some(value) = inline_flag_value(flag, name) {
            value
        } else {
            continue;
        };
        if value.is_empty() {
            return Err(format!("{name} requires a value"));
        }
        values.push(value.to_owned());
    }
    Ok(values)
}

fn is_known_flag(argument: &str) -> bool {
    let name = argument.split_once('=').map_or(argument, |(name, _)| name);
    KNOWN_FLAG_NAMES.contains(&name)
}

fn inline_flag_value<'a>(argument: &'a str, name: &str) -> Option<&'a str> {
    argument
        .split_once('=')
        .and_then(|(candidate, value)| (candidate == name).then_some(value))
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
    flags
        .iter()
        .any(|flag| flag == name || inline_flag_value(flag, name).is_some())
}

pub(super) fn is_help_request(command: &str, flags: &[String]) -> bool {
    matches!(command, "help" | "--help" | "-h")
        || has_flag(flags, "--help")
        || has_flag(flags, "-h")
}

pub(super) fn print_help() -> std::io::Result<()> {
    stdoutln!(
        r#"dcc-cua

  version | --version | -V
  list [--app APP] [--pid PID] [--window-id ID] [--title TITLE] [--on-screen]
  wait-window --app APP|--pid PID|--window-id ID|--title TITLE [--on-screen] [--timeout-ms N] [--poll-ms N]
  apps
  tools
  call --tool NAME [--json JSON|--json-file PATH] [--app APP|--pid PID --window-id ID] [--output FILE]
  ping [--endpoint PATH|--spawn BINARY] [--agent-name NAME]
  interrupt-all [--endpoint PATH]
  host-call --method NAME [--json JSON|--json-file PATH] [--endpoint PATH|--spawn BINARY] [--agent-name NAME] [--snapshot-transport binary_frame|shared_memory] [--output FILE]
  host-batch --json JSON_ARRAY [--endpoint PATH|--spawn BINARY] [--agent-name NAME] [--snapshot-transport binary_frame|shared_memory] [--output-dir DIR]
  host-jsonl [--endpoint PATH|--spawn BINARY] [--agent-name NAME] [--parallel-discovery] [--showcase] [--showcase-dir DIR] [--snapshot-transport binary_frame|shared_memory] [--response-format host|mcp] [--output-dir DIR] [--metrics-output FILE]
  mcp-server                       # stdio MCP Apps bridge with one pre-task user authorization card
  host-ensure [--endpoint PATH] [--grant existing-profile]
  browser-extension plan|status|install-native-host --browser chrome|edge|firefox --extension-id PUBLISHED_ID [--cdp-state available|unavailable]
  manifest
  profiles [--state valid|invalid|all] [--profile-store DIR]
                                   # default: list only usable built-in and installed profiles; invalid shows package diagnostics
  profile validate PACKAGE_DIR [--profile-store DIR]
  profile install PACKAGE_DIR [--replace] [--profile-store DIR]
  profile uninstall ID --confirm [--profile-store DIR]
  profile match --app APP [--title TITLE]
  profile context --id PROFILE [--identity NAMESPACE=VALUE]... [--selector KEY=VALUE]... [--profile-store DIR] [--knowledge-root DIR]
  profile --id ue|maya|fab|... [--profile-file PATH] [--app APP] [--surface ID] [--query TARGET] [--action ACTION] [--activate] [--max-elements N] [--max-depth N]
  profile-state --id PROFILE|--profile-file PATH [--source ID] [--etag ETAG] [--watch] [--poll-ms N] [--max-updates N]
  update [--check]
  desktop-snapshot [--output FILE]
  screen-size
  cursor-position
  launch --name NAME|--bundle-id ID|--aumid ID|--path PATH|--launch-path PATH [--url URL] [--arg ARG] [--new-instance] [--start-minimized]
  terminate --app APP --confirm
  snapshot --app APP|--pid PID|--window-id ID|--title TITLE [--pixels-only] [--activate] [--escalate --escalation-reason REASON] [--escalation-detail NOTE] [--output FILE]
  restore-activate --pid PID --window-id ID
  set-window-frame --app APP|--pid PID --window-id ID --x N --y N --width N --height N
  invoke-menu --app APP|--pid PID --window-id ID --menu TOP [--menu CHILD ...]
  act --app APP|--pid PID|--window-id ID|--title TITLE --action-json JSON [--observation-width W --observation-height H] [--output FILE]
  verify --app APP|--pid PID|--window-id ID|--title TITLE --expect-json JSON [--timeout-ms N] [--stable-samples N]
  desktop-act --action-json JSON [--session ID] [--output FILE]
  clipboard-read --app APP|--pid PID|--window-id ID|--title TITLE [--include-text]
  clipboard-write --app APP|--pid PID|--window-id ID|--title TITLE --text TEXT|--image-path FILE|--file-path FILE
  doctor [--route full|visual|semantic] [--endpoint PATH|--spawn BINARY]
  host [--stdio|--endpoint PATH] [--grant existing-profile]

Host uses versioned big-endian JSON frames. Hello version 1 negotiates binary-frame or shared-memory snapshots and supports request_id correlation."#
    );
    stdoutln!(
        "Profiles are built-in, installed from ~/.dcc-cua/profiles, or loaded explicitly from JSON; package installation copies declarative content only and never launches bundled code. snapshot --pid PID --window-id ID --pixels-only skips accessibility and captures only that exact native window; it never falls back to a whole-desktop screenshot. Window snapshots/actions accept --escalate --escalation-reason REASON for legacy visual fallback routes; --activate keeps custom-rendered foreground capture and actions in one session."
    );
    stdoutln!("{}", escalation_reason_help());
    stdoutln!("Zoom: zoom --app APP --x1 N --y1 N --x2 N --y2 N [--output FILE].");
    stdoutln!(
        "Friendly window actions accept --delivery-mode background|foreground. x/y and drag paths are non-negative coordinates in the latest exact-window screenshot (not UIA virtual-desktop bounds). Coordinate actions require --observation-width and --observation-height from snapshot.coordinate_space. Desktop actions use signed virtual-desktop coordinates. Actions: click [--x X --y Y|--element-index N|--element-token TOKEN] [--button left|middle|right --duration-ms N], double-click/right-click/toggle [--x X --y Y|--element-index N|--element-token TOKEN] [--button left|middle|right], drag --from-x X --from-y Y --to-x X --to-y Y [--button B --modifier M --duration-ms N --steps N], type [--text TEXT] [--focused|--x X --y Y|--element-index N], set-text/set-value, press [--key K] [--modifier M] [--x X --y Y|--element-index N], hotkey [--key K ...] [--x X --y Y], scroll [--scroll-x N|--scroll-y N] [--by line|page] [--x X --y Y|--element-index N], move."
    );
    stdoutln!(
        "Coordinate mapping: snapshot.coordinate_space width/height are the encoded PNG pixel dimensions. screen_x = bounds.x + x * bounds.width / observation_width; screen_y = bounds.y + y * bounds.height / observation_height. window-state bounds are already device pixels. Do not apply screen-size scale_factor to them."
    );
    stdoutln!(
        "Snapshot activation: when snapshot --activate receives foreground_activation_refused with background_delivery_viable=true, it preserves the exact PID/HWND session, captures without foreground activation, and reports activation.status=refused_fallback_background. Every other activation error remains fatal."
    );
    stdoutln!(
        "Semantic tree: accessibility --app APP [--max-elements N] [--max-depth N]. no_accessibility_provider is permanent for that window class: do not retry accessibility; use snapshot --pixels-only plus OCR or another perception layer. Window: window-state|activate|restore-activate|set-window-frame|invoke-menu. restore-activate requires an exact --pid/--window-id pair."
    );
    Ok(())
}

pub(super) fn escalation_reason_help() -> String {
    let values = COMPUTER_USE_ESCALATION_REASONS
        .iter()
        .map(|reason| format!("{} ({})", reason.value, reason.meaning))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Escalation reasons: {values}. Use --escalation-detail NOTE for an optional audit note of at most {MAX_ESCALATION_DETAIL_CHARS} characters."
    )
}
