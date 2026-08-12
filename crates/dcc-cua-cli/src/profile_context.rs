use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::cli_args::flag_value;
use super::profile_package::{ProfilePackageError, installed_package};

const BASE_RULES: &str = "knowledge/base-rules.seed.json";
const SEED_INDEX: &str = "knowledge/playbooks/index.seed.json";
const MAX_CONTEXT_BYTES: u64 = 2 * 1024 * 1024;

struct ContextSelection {
    kind: String,
    playbook: Option<Value>,
    provenance: Value,
    warnings: Vec<String>,
}

pub(crate) fn execute(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let id = flag_value(flags, "--id").ok_or("profile context requires --id PROFILE")?;
    let content_id = flag_value(flags, "--catalog-content-id")
        .ok_or("profile context requires --catalog-content-id ID")?;
    let hero = flag_value(flags, "--hero");
    let season = flag_value(flags, "--season");
    let store = flag_value(flags, "--profile-store").map(PathBuf::from);
    let package = installed_package(&id, store.as_deref())?
        .ok_or_else(|| ProfilePackageError::NotInstalled(id.clone()))?;
    let supports_context = package
        .manifest
        .capabilities
        .iter()
        .any(|capability| capability == "startup_context");
    if !supports_context {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 1,
                "profileId": id,
                "selection": "none",
                "requiresRefresh": false,
                "baseRules": null,
                "uiAtlas": null,
                "selectedPlaybook": null,
                "warnings": ["installed profile does not declare startup_context"]
            }))?
        );
        return Ok(());
    }

    let base = read_context_json(&package.root, BASE_RULES)?;
    validate_owned_document(&base, &id, BASE_RULES)?;
    let knowledge_root = flag_value(flags, "--knowledge-root")
        .map(PathBuf::from)
        .unwrap_or(resolve_knowledge_root()?.join(&id));
    let user_index = knowledge_root.join("playbooks/index.json");
    let seed_index = package.root.join(SEED_INDEX);
    let mut selected = select_playbook(
        &id,
        &content_id,
        hero.as_deref(),
        season.as_deref(),
        &user_index,
        &seed_index,
        &package.root,
        &knowledge_root,
    )?;
    if selected.playbook.is_none() {
        selected
            .warnings
            .push("no exact catalog/hero playbook matched; loaded base rules only".into());
    }
    let output = json!({
        "schemaVersion": 1,
        "profileId": id,
        "catalogContentId": content_id,
        "hero": hero,
        "season": season,
        "selection": selected.kind,
        "requiresRefresh": selected.playbook.is_none(),
        "baseRules": base.get("rules").cloned().unwrap_or(Value::Null),
        "uiAtlas": base.get("uiAtlas").cloned().unwrap_or(Value::Null),
        "selectedPlaybook": selected.playbook,
        "provenance": selected.provenance,
        "sources": base.get("sources").cloned().unwrap_or_else(|| json!([])),
        "warnings": selected.warnings,
    });
    let encoded = serde_json::to_vec(&output)?;
    if encoded.len() as u64 > MAX_CONTEXT_BYTES {
        return Err("profile startup context exceeds the 2 MiB output limit".into());
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn select_playbook(
    profile_id: &str,
    content_id: &str,
    hero: Option<&str>,
    season: Option<&str>,
    user_index: &Path,
    seed_index: &Path,
    package_root: &Path,
    knowledge_root: &Path,
) -> Result<ContextSelection, Box<dyn std::error::Error>> {
    let candidates = [
        ("fresh_exact", user_index, knowledge_root),
        ("seed_exact", seed_index, package_root),
    ];
    for (selection, index_path, root) in candidates {
        if !index_path.is_file() {
            continue;
        }
        let index = read_json_file(index_path)?;
        validate_owned_document(&index, profile_id, &index_path.display().to_string())?;
        let entries = index
            .get("entries")
            .and_then(Value::as_array)
            .ok_or("playbook index entries must be an array")?;
        let matching = entries.iter().filter(|entry| {
            entry
                .get("catalogContentIds")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(content_id)))
                && hero.is_none_or(|expected| {
                    entry
                        .get("hero")
                        .and_then(Value::as_str)
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
                })
                && season.is_none_or(|expected| {
                    entry
                        .get("seasonId")
                        .and_then(Value::as_str)
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
                })
        });
        let matches = matching.collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err("multiple playbooks match the exact catalog/hero selection".into());
        }
        if let Some(entry) = matches.first() {
            let relative = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or("playbook index entry requires path")?;
            let path = safe_child(root, relative)?;
            let document = read_json_file(&path)?;
            validate_owned_document(&document, profile_id, relative)?;
            let fenced = document
                .pointer("/catalogFence/contentId")
                .and_then(Value::as_str)
                == Some(content_id);
            if !fenced {
                return Err("selected playbook catalog fence does not match its index".into());
            }
            return Ok(ContextSelection {
                kind: selection.into(),
                playbook: Some(document),
                provenance: json!({"index": index_path, "playbook": path}),
                warnings: Vec::new(),
            });
        }
    }
    Ok(ContextSelection {
        kind: "none".into(),
        playbook: None,
        provenance: json!({}),
        warnings: Vec::new(),
    })
}

fn validate_owned_document(
    value: &Value,
    profile_id: &str,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(format!("{label} requires schemaVersion 1").into());
    }
    if value.get("profileId").and_then(Value::as_str) != Some(profile_id) {
        return Err(format!("{label} profileId does not match {profile_id}").into());
    }
    Ok(())
}

fn read_context_json(root: &Path, relative: &str) -> Result<Value, Box<dyn std::error::Error>> {
    read_json_file(&safe_child(root, relative)?)
}

fn safe_child(root: &Path, relative: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("knowledge path must be a normalized relative path".into());
    }
    let root = fs::canonicalize(root)?;
    let path = fs::canonicalize(root.join(relative))?;
    if !path.starts_with(&root) {
        return Err("knowledge path escapes its owned root".into());
    }
    Ok(path)
}

fn read_json_file(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_CONTEXT_BYTES {
        return Err(format!(
            "context file is not a bounded regular file: {}",
            path.display()
        )
        .into());
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn resolve_knowledge_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = env::var_os("DCC_CUA_HOME") {
        return Ok(PathBuf::from(path).join("knowledge"));
    }
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or("cannot resolve the user home directory")?;
    Ok(PathBuf::from(home).join(".dcc-cua/knowledge"))
}

#[cfg(test)]
mod tests;
