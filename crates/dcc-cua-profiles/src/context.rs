use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::package::{
    ProfilePackageArtifactType, ProfilePackageError, ProfileStore, ensure_regular_file, io_error,
    read_bounded, validated_relative_path,
};

const USER_INDEX: &str = "index.json";
const MAX_CONTEXT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileContextRequest {
    pub profile_id: String,
    pub identities: BTreeMap<String, String>,
    pub selectors: BTreeMap<String, String>,
    pub knowledge_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSelection {
    None,
    Exact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProvenance {
    pub index: PathBuf,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDocument {
    pub id: String,
    pub document: Value,
    pub provenance: ContextProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileContext {
    pub schema_version: u32,
    pub profile_id: String,
    pub identities: BTreeMap<String, String>,
    pub selectors: BTreeMap<String, String>,
    pub documents: Vec<ContextDocument>,
    pub selection: ContextSelection,
    pub requires_refresh: bool,
    pub warnings: Vec<String>,
}

pub(crate) fn select_context(
    store: &ProfileStore,
    request: ProfileContextRequest,
) -> Result<ProfileContext, ProfilePackageError> {
    validate_pairs(&request.identities, "identity")?;
    validate_pairs(&request.selectors, "selector")?;
    let package = store
        .package(&request.profile_id)?
        .ok_or_else(|| ProfilePackageError::NotInstalled(request.profile_id.clone()))?;

    let mut indexes = Vec::new();
    if let Some(knowledge_root) = &request.knowledge_root {
        let user_index = knowledge_root.join(USER_INDEX);
        if user_index.is_file() {
            indexes.push((user_index, knowledge_root.clone()));
        }
    }
    indexes.extend(
        package
            .manifest
            .artifacts
            .iter()
            .filter(|artifact| artifact.artifact_type == ProfilePackageArtifactType::ContextIndex)
            .map(|artifact| (package.root.join(&artifact.path), package.root.clone())),
    );

    let documents = select_documents(
        &request.profile_id,
        &request.identities,
        &request.selectors,
        &indexes,
    )?;
    let selection = if documents.is_empty() {
        ContextSelection::None
    } else {
        ContextSelection::Exact
    };
    let context = ProfileContext {
        schema_version: 2,
        profile_id: request.profile_id,
        identities: request.identities,
        selectors: request.selectors,
        documents,
        selection,
        requires_refresh: selection == ContextSelection::None,
        warnings: if selection == ContextSelection::None {
            vec!["no document matched every identity fence and selector".into()]
        } else {
            Vec::new()
        },
    };
    if serde_json::to_vec(&context)
        .map_err(|error| ProfilePackageError::Invalid(error.to_string()))?
        .len() as u64
        > MAX_CONTEXT_BYTES
    {
        return Err(ProfilePackageError::Invalid(
            "profile context exceeds the 2 MiB output limit".into(),
        ));
    }
    Ok(context)
}

fn select_documents(
    profile_id: &str,
    identities: &BTreeMap<String, String>,
    selectors: &BTreeMap<String, String>,
    indexes: &[(PathBuf, PathBuf)],
) -> Result<Vec<ContextDocument>, ProfilePackageError> {
    let mut selected = Vec::new();
    let mut document_ids = BTreeSet::new();
    for (index_path, root) in indexes {
        let index = read_json_file(index_path)?;
        validate_owned_document(&index, profile_id, &index_path.display().to_string())?;
        let entries = index
            .get("documents")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProfilePackageError::Invalid("context index documents must be an array".into())
            })?;
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
                return Err(ProfilePackageError::Invalid(format!(
                    "conflicting context document id: {entry_id}"
                )));
            }
            let relative = required_string(entry, "path", "context index document")?;
            let path = safe_child(root, relative)?;
            let document = read_json_file(&path)?;
            validate_owned_document(&document, profile_id, relative)?;
            if document.get("id").and_then(Value::as_str) != Some(entry_id) {
                return Err(ProfilePackageError::Invalid(format!(
                    "{relative} id does not match index entry {entry_id}"
                )));
            }
            let fences = object_strings(document.get("fences"), "fences")?;
            if fences != entry_identities || !map_matches(&fences, identities) {
                return Err(ProfilePackageError::Invalid(format!(
                    "{relative} fences do not exactly match its index identities"
                )));
            }
            selected.push(ContextDocument {
                id: entry_id.to_owned(),
                document,
                provenance: ContextProvenance {
                    index: index_path.clone(),
                    path,
                },
            });
        }
    }
    Ok(selected)
}

fn validate_pairs(
    pairs: &BTreeMap<String, String>,
    label: &str,
) -> Result<(), ProfilePackageError> {
    if pairs.iter().any(|(key, value)| {
        key.is_empty()
            || value.is_empty()
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
    }) {
        return Err(ProfilePackageError::Invalid(format!(
            "{label} must use a non-empty portable NAMESPACE=VALUE"
        )));
    }
    Ok(())
}

fn map_matches(required: &BTreeMap<String, String>, actual: &BTreeMap<String, String>) -> bool {
    required
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

fn object_strings(
    value: Option<&Value>,
    label: &str,
) -> Result<BTreeMap<String, String>, ProfilePackageError> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| ProfilePackageError::Invalid(format!("{label} must be an object")))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.to_owned()))
                .ok_or_else(|| {
                    ProfilePackageError::Invalid(format!("{label}.{key} must be a string"))
                })
        })
        .collect()
}

fn required_string<'a>(
    value: &'a Value,
    key: &str,
    label: &str,
) -> Result<&'a str, ProfilePackageError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ProfilePackageError::Invalid(format!("{label} requires {key}")))
}

fn validate_owned_document(
    value: &Value,
    profile_id: &str,
    label: &str,
) -> Result<(), ProfilePackageError> {
    if value.get("schemaVersion").and_then(Value::as_u64) != Some(2) {
        return Err(ProfilePackageError::Invalid(format!(
            "{label} requires schemaVersion 2"
        )));
    }
    if value.get("profileId").and_then(Value::as_str) != Some(profile_id) {
        return Err(ProfilePackageError::Invalid(format!(
            "{label} profileId does not match {profile_id}"
        )));
    }
    Ok(())
}

fn safe_child(root: &Path, relative: &str) -> Result<PathBuf, ProfilePackageError> {
    let relative = validated_relative_path(relative)?;
    let root = fs::canonicalize(root).map_err(|source| io_error(root, source))?;
    let candidate = root.join(relative);
    ensure_regular_file(&candidate)?;
    let path = fs::canonicalize(&candidate).map_err(|source| io_error(&candidate, source))?;
    if !path.starts_with(&root) {
        return Err(ProfilePackageError::Invalid(
            "knowledge path escapes its owned root".into(),
        ));
    }
    Ok(path)
}

fn read_json_file(path: &Path) -> Result<Value, ProfilePackageError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_CONTEXT_BYTES
    {
        return Err(ProfilePackageError::Invalid(format!(
            "context file is not a bounded regular file: {}",
            path.display()
        )));
    }
    serde_json::from_slice(&read_bounded(path, MAX_CONTEXT_BYTES)?)
        .map_err(|error| ProfilePackageError::Invalid(error.to_string()))
}
