use dcc_cua_core::{ComputerUseAction, ComputerUseDriver};
use dcc_cua_semantic_profiles::{SemanticProfile, SemanticRoute};
use serde_json::json;

use super::{
    action_result_value, bounded_u32, flag_value, has_flag, load_semantic_profile, maybe_escalate,
    select_scope, semantic_post_snapshot_value,
};

pub(crate) async fn execute(
    driver: &ComputerUseDriver,
    flags: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let profile = load_semantic_profile(flags)?;
    let surface_id = flag_value(flags, "--surface").unwrap_or_else(|| {
        profile
            .surfaces
            .first()
            .map_or_else(|| "".into(), |surface| surface.id.clone())
    });
    let query = flag_value(flags, "--query").unwrap_or_else(|| {
        profile
            .surface(&surface_id)
            .and_then(|surface| surface.targets.first())
            .map_or_else(|| "".into(), |target| target.id.clone())
    });
    let surface = profile
        .surface(&surface_id)
        .ok_or_else(|| format!("profile {:?} has no surface {surface_id:?}", profile.id))?;
    let target = profile.resolve_target(&surface_id, &query).ok_or_else(|| {
        format!(
            "profile {:?} has no target {query:?} on {surface_id:?}",
            profile.id
        )
    })?;
    let action_name = flag_value(flags, "--action");
    if action_name.is_some() && surface.route != SemanticRoute::Accessibility {
        return Err(format!(
            "profile surface {surface_id:?} uses {:?}; profile CLI actions currently require the accessibility route",
            surface.route
        )
        .into());
    }
    let scope = select_scope(driver, flags).await?;
    let app = flag_value(flags, "--app").unwrap_or_else(|| profile.display_name.clone());
    let session_id =
        flag_value(flags, "--session").unwrap_or_else(|| format!("dcc-cua-profile-{}", profile.id));
    let max_elements = bounded_u32(flags, "--max-elements", 5_000, 5_000)?;
    let max_depth = bounded_u32(flags, "--max-depth", 64, 64)?;
    let mut session = driver.session(scope, app, session_id)?;
    session.start().await?;
    let result = async {
        maybe_escalate(&mut session, flags).await?;
        let activation = if has_flag(flags, "--activate") {
            Some(session.activate().await?)
        } else {
            None
        };
        let root = session
            .accessibility_snapshot(max_elements, max_depth)
            .await?;
        let observation = session.latest_observation().cloned().ok_or_else(|| {
            dcc_cua_core::ComputerUseError::new(
                dcc_cua_core::ComputerUseErrorCode::CaptureFailed,
                "semantic snapshot returned no observation metadata",
            )
        })?;
        let exact_target = session.target();
        if !target_matches_profile(&profile, exact_target.as_ref()) {
            return Err(dcc_cua_core::ComputerUseError::new(
                dcc_cua_core::ComputerUseErrorCode::InvalidTarget,
                format!(
                    "live target does not match semantic profile {:?} selectors",
                    profile.id
                ),
            ));
        }
        let matches = profile
            .find_elements(&surface_id, &root, &query)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let action_result = if let Some(action_name) = action_name.as_deref() {
            if !target.supports_action(action_name) {
                return Err(dcc_cua_core::ComputerUseError::new(
                    dcc_cua_core::ComputerUseErrorCode::InvalidAction,
                    format!("profile target {query:?} does not support action {action_name:?}"),
                ));
            }
            if matches.len() != 1 {
                return Err(dcc_cua_core::ComputerUseError::new(
                    dcc_cua_core::ComputerUseErrorCode::InvalidAction,
                    format!(
                        "profile target {query:?} matched {} live elements",
                        matches.len()
                    ),
                ));
            }
            let element = &matches[0];
            let action = ComputerUseAction {
                action: action_name.to_owned(),
                element_index: element["element_index"].as_u64().map(|index| index as u32),
                element_token: element["element_token"].as_str().map(str::to_owned),
                observation_id: Some(observation.observation_id.clone()),
                ..ComputerUseAction::default()
            };
            if action.element_index.is_none() && action.element_token.is_none() {
                return Err(dcc_cua_core::ComputerUseError::new(
                    dcc_cua_core::ComputerUseErrorCode::InvalidAction,
                    "profile match has no live element locator",
                ));
            }
            Some(action_result_value(session.perform_action(&action).await?))
        } else {
            None
        };
        let post_snapshot = if action_result.is_some() {
            Some(semantic_post_snapshot_value(
                session
                    .accessibility_snapshot(max_elements, max_depth)
                    .await,
                None,
            ))
        } else {
            None
        };
        Ok::<_, dcc_cua_core::ComputerUseError>((
            activation,
            root,
            observation,
            exact_target,
            matches,
            action_result,
            post_snapshot,
        ))
    }
    .await;
    let stop_result = session.stop().await;
    let (activation, root, observation, exact_target, matches, action_result, post_snapshot) =
        result?;
    stop_result?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "success": true,
            "profile": profile,
            "surface": surface,
            "target": target,
            "matched_elements": matches,
            "match_count": matches.len(),
            "observation_id": observation.observation_id,
            "exact_target": exact_target,
            "activation": activation,
            "executed_action": action_name,
            "action": action_result,
            "post_snapshot": post_snapshot,
            "semantic_snapshot": {
                "node_count": root["elements"].as_array().map_or(0, Vec::len),
                "max_elements": max_elements,
                "max_depth": max_depth,
            },
        }))?
    );
    Ok(())
}

pub(crate) fn target_matches_profile(
    profile: &SemanticProfile,
    target: Option<&serde_json::Value>,
) -> bool {
    let Some(target) = target else {
        return false;
    };
    profile.matches_window(
        target["app_name"].as_str().unwrap_or_default(),
        target["title"].as_str().unwrap_or_default(),
    ) || target["url"]
        .as_str()
        .is_some_and(|url| profile.matches_url(url))
}
