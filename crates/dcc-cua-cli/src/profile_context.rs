use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::cli_args::{flag_value, flag_values};
use super::profile_package::{ProfilePackageArtifactType, ProfilePackageError, installed_package};

const USER_INDEX: &str = "index.json";
const MAX_CONTEXT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
struct SelectedDocument {
    id: String,
    document: Value,
    index: PathBuf,
    path: PathBuf,
}

pub(crate) fn execute(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let id = flag_value(flags, "--id").ok_or("profile context requires --id PROFILE")?;
    let identities = parse_pairs(&flag_values(flags, "--identity"), "identity")?;
    let selectors = parse_pairs(&flag_values(flags, "--selector"), "selector")?;
    let store = flag_value(flags, "--profile-store").map(PathBuf::from);
    let package = installed_package(&id, store.as_deref())?
        .ok_or_else(|| ProfilePackageError::NotInstalled(id.clone()))?;

    let seed_indexes = package
        .manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.artifact_type == ProfilePackageArtifactType::ContextIndex)
        .map(|artifact| {
            (
                package.root.join(&artifact.path),
                package.root.clone(),
                "seed",
            )
        })
        .collect::<Vec<_>>();
    let knowledge_root = flag_value(flags, "--knowledge-root")
        .map(PathBuf::from)
        .unwrap_or(resolve_knowledge_root()?.join(&id));
    let mut indexes = Vec::new();
    let user_index = knowledge_root.join(USER_INDEX);
    if user_index.is_file() {
        indexes.push((user_index, knowledge_root, "user"));
    }
    indexes.extend(seed_indexes);

    let selected = select_documents(&id, &identities, &selectors, &indexes)?;
    let documents = selected
        .iter()
        .map(|item| {
            json!({
                "id": item.id,
                "document": item.document,
                "provenance": {"index": item.index, "path": item.path},
            })
        })
        .collect::<Vec<_>>();
    let output = json!({
        "schemaVersion": 2,
        "profileId": id,
        "identities": identities,
        "selectors": selectors,
        "documents": documents,
        "selection": if selected.is_empty() { "none" } else { "exact" },
        "requiresRefresh": selected.is_empty(),
        "warnings": if selected.is_empty() {
            vec!["no document matched every identity fence and selector"]
        } else {
            Vec::<&str>::new()
        },
    });
    let encoded = serde_json::to_vec(&output)?;
    if encoded.len() as u64 > MAX_CONTEXT_BYTES {
        return Err("profile context exceeds the 2 MiB output limit".into());
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn select_documents(
    profile_id: &str,
    identities: &BTreeMap<String, String>,
    selectors: &BTreeMap<String, String>,
    indexes: &[(PathBuf, PathBuf, &str)],
) -> Result<Vec<SelectedDocument>, Box<dyn std::error::Error>> {
    let mut selected = Vec::new();
    let mut document_ids = BTreeSet::new();
    for (index_path, root, _source) in indexes {
        let index = read_json_file(index_path)?;
        validate_owned_document(&index, profile_id, &index_path.display().to_string())?;
        let entries = index
            .get("documents")
            .and_then(Value::as_array)
            .ok_or("context index documents must be an array")?;
        for entry in entries {
            let entry_id = required_string(entry, "id", "context index document")?;
            let entry_identities = object_strings(entry.get("identities"), "identities")?;
            let entry_selectors = object_strings(entry.get("selectors"), "selectors")?;
            if !map_matches(&entry_identities, identities)
                || !map_matches(&entry_selectors, selectors)
            {
                continue;
            }
            if !document_ids.insert(entry_id.to_owned()) {
                return Err(format!("conflicting context document id: {entry_id}").into());
            }
            let relative = required_string(entry, "path", "context index document")?;
            let path = safe_child(root, relative)?;
            let document = read_json_file(&path)?;
            validate_owned_document(&document, profile_id, relative)?;
            if document.get("id").and_then(Value::as_str) != Some(entry_id) {
                return Err(format!("{relative} id does not match index entry {entry_id}").into());
            }
            let fences = object_strings(document.get("fences"), "fences")?;
            if fences != entry_identities || !map_matches(&fences, identities) {
                return Err(
                    format!("{relative} fences do not exactly match its index identities").into(),
                );
            }
            selected.push(SelectedDocument {
                id: entry_id.to_owned(),
                document,
                index: index_path.clone(),
                path,
            });
        }
    }
    Ok(selected)
}

fn map_matches(required: &BTreeMap<String, String>, actual: &BTreeMap<String, String>) -> bool {
    required
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

fn parse_pairs(
    values: &[String],
    label: &str,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut pairs = BTreeMap::new();
    for value in values {
        let (key, selected) = value
            .split_once('=')
            .ok_or_else(|| format!("{label} must be NAMESPACE=VALUE"))?;
        if key.is_empty()
            || selected.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!("{label} must use a non-empty portable NAMESPACE=VALUE").into());
        }
        if pairs.insert(key.to_owned(), selected.to_owned()).is_some() {
            return Err(format!("duplicate {label}: {key}").into());
        }
    }
    Ok(pairs)
}

fn object_strings(
    value: Option<&Value>,
    label: &str,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let value = value.ok_or_else(|| format!("{label} is required"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} must be an object"))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_owned()))
                .ok_or_else(|| format!("{label}.{key} must be a string").into())
        })
        .collect()
}

fn required_string<'a>(
    value: &'a Value,
    key: &str,
    label: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label} requires {key}").into())
}

fn validate_owned_document(
    value: &Value,
    profile_id: &str,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(2) {
        return Err(format!("{label} requires schemaVersion 2").into());
    }
    if value.get("profileId").and_then(Value::as_str) != Some(profile_id) {
        return Err(format!("{label} profileId does not match {profile_id}").into());
    }
    Ok(())
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
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_CONTEXT_BYTES
    {
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
