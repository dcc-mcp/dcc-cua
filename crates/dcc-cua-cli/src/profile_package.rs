use std::path::{Path, PathBuf};

use dcc_cua_profiles::{
    ProfilePackageError, ProfileStore, ProfileStoreEntry, ValidatedProfilePackage, validate_package,
};
use serde_json::{Value, json};

use super::{flag_value, has_flag};

pub(crate) fn is_management_command(flags: &[String]) -> bool {
    matches!(
        flags.first().map(String::as_str),
        Some("validate" | "install" | "uninstall")
    )
}

pub(crate) fn execute(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let command = flags
        .first()
        .map(String::as_str)
        .ok_or("profile command is missing")?;
    let target = flags
        .get(1)
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("profile {command} requires a package directory or ID"))?;
    let store_path = flag_value(flags, "--profile-store").map(PathBuf::from);
    let mut store = open_store(store_path.as_deref())?;
    match command {
        "validate" => {
            let mut package = validate_package(Path::new(target))?;
            package.profile = store.resolve(package.profile.clone())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&package_summary(&package, "validated"))?
            );
        }
        "install" => {
            let package = store.install(Path::new(target), has_flag(flags, "--replace"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&package_summary(&package, "installed"))?
            );
        }
        "uninstall" => {
            if !has_flag(flags, "--confirm") {
                return Err("profile uninstall requires --confirm".into());
            }
            let removed = store.uninstall(target)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "id": target,
                    "status": "uninstalled",
                    "path": removed,
                }))?
            );
        }
        _ => return Err(format!("unknown profile management command: {command}").into()),
    }
    Ok(())
}

pub(crate) fn open_store(
    override_path: Option<&Path>,
) -> Result<ProfileStore, ProfilePackageError> {
    match override_path {
        Some(path) => ProfileStore::open(path),
        None => ProfileStore::open_default(),
    }
}

pub(crate) fn installed_profile_summaries(store: &ProfileStore) -> Vec<Value> {
    store
        .entries()
        .iter()
        .map(|entry| match entry {
            ProfileStoreEntry::Ready(package) => package_summary(package, "ready"),
            ProfileStoreEntry::Invalid { id, path, error } => json!({
                "id": id,
                "source": "user",
                "status": "invalid",
                "path": path,
                "error": error,
            }),
        })
        .collect()
}

fn package_summary(package: &ValidatedProfilePackage, status: &str) -> Value {
    json!({
        "id": package.manifest.id,
        "display_name": package.manifest.display_name,
        "version": package.manifest.version,
        "application": package.profile.application,
        "extends": package.profile.extends,
        "source": "user",
        "status": status,
        "path": package.root,
        "license": package.manifest.license,
        "artifacts": package.manifest.artifacts,
        "requires": package.manifest.requires,
        "platforms": package.manifest.platforms,
        "preferred_route": package.profile.settings.preferred_route,
        "dialog_style": package.profile.settings.dialog_style,
        "supported_locales": package.profile.supported_locales(),
        "surface_count": package.profile.surfaces.len(),
        "state_source_count": package.profile.state_sources.len(),
        "file_count": package.file_count,
        "total_bytes": package.total_bytes,
    })
}
