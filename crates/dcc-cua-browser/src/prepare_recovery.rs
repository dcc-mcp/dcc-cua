//! Human recovery guidance is descriptive and never grants setup or tab cleanup.

use serde_json::{Value, json};

pub(super) fn annotate(value: &mut Value, existing_profile: bool, target: Option<&Value>) {
    let structured = &value["structuredContent"];
    if !existing_profile
        || structured["status"] != "refused"
        || structured["prepared"] == true
        || structured["refusal"]["code"] != "browser_wrong_target_refused"
    {
        return;
    }
    let effects = &structured["refusal"]["detail"]["setup_side_effects"];
    let (Some(opened), Some(closed), Some(_enabled)) = (
        effects["opened_setup_page"].as_bool(),
        effects["closed_setup_page"].as_bool(),
        effects["enabled_remote_debugging"].as_bool(),
    ) else {
        return;
    };
    if closed && !opened {
        return;
    }
    let Some(target) = target else {
        return;
    };
    let (Some(pid), Some(window_id)) = (target["pid"].as_u64(), target["window_id"].as_u64())
    else {
        return;
    };
    if pid == 0 || pid > u64::from(u32::MAX) || window_id == 0 {
        return;
    }

    let mut setup = json!({
        "setting_state": "unverified",
        "instruction": "In the exact browser window, manually verify its remote-debugging setting before retrying preparation."
    });
    // These are static human-facing hints from the last validated native app
    // name. They do not classify a live tab or authorize opening or closing it.
    if let Some(url) = setup_url(target["app_name"].as_str().unwrap_or_default()) {
        setup["url"] = json!(url);
        setup["checkbox_label"] = json!("Allow remote debugging for this browser instance");
    }
    let cleanup_status = if opened && !closed {
        "manual_cleanup_required"
    } else if closed {
        "closed_by_driver"
    } else {
        "not_opened"
    };
    value["structuredContent"]["recovery"] = json!({
        "kind": "existing_profile_setup",
        "status": "manual_action_required",
        "target": {"pid": pid, "window_id": window_id},
        "setup": setup,
        "cleanup": {
            "status": cleanup_status,
            "instruction": "Manually review any setup tab left in this exact window; close only a tab you identify as belonging to this preparation attempt."
        },
        "automatic_cleanup": false,
        "automatic_retry": false,
        "fresh_observation_required": true,
        "retry_after": "manual_setup_and_cleanup"
    });
}

fn setup_url(app_name: &str) -> Option<&'static str> {
    match app_name
        .rsplit(['/', '\\'])
        .next()?
        .to_ascii_lowercase()
        .as_str()
    {
        "msedge.exe" | "msedge" | "microsoft edge" => Some("edge://inspect/#remote-debugging"),
        "chrome.exe" | "chrome" | "google chrome" | "chromium.exe" | "chromium" => {
            Some("chrome://inspect/#remote-debugging")
        }
        _ => None,
    }
}
