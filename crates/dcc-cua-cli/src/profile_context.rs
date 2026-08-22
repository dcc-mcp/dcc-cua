use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

use dcc_cua_profiles::ProfileContextRequest;

use super::cli_args::{flag_value, flag_values};
use super::profile_package::open_store;

pub(crate) fn execute(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let id = flag_value(flags, "--id").ok_or("profile context requires --id PROFILE")?;
    let identities = parse_pairs(&flag_values(flags, "--identity"), "identity")?;
    let selectors = parse_pairs(&flag_values(flags, "--selector"), "selector")?;
    let store_path = flag_value(flags, "--profile-store").map(PathBuf::from);
    let knowledge_root = flag_value(flags, "--knowledge-root")
        .map(PathBuf::from)
        .unwrap_or(resolve_knowledge_root()?.join(&id));
    let store = open_store(store_path.as_deref())?;
    let context = store.context(ProfileContextRequest {
        profile_id: id,
        identities,
        selectors,
        knowledge_root: Some(knowledge_root),
    })?;
    println!("{}", serde_json::to_string_pretty(&context)?);
    Ok(())
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
            || !key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(format!("{label} must use a non-empty portable NAMESPACE=VALUE").into());
        }
        if pairs.insert(key.to_owned(), selected.to_owned()).is_some() {
            return Err(format!("duplicate {label}: {key}").into());
        }
    }
    Ok(pairs)
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
