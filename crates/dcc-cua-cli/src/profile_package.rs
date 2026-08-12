use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dcc_cua_semantic_profiles::{SemanticProfile, builtin_profile, parse_profile, resolve_profile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use super::{flag_value, has_flag};

const MANIFEST_FILE: &str = "profile-package.json";
const PACKAGE_KIND: &str = "dcc-cua-profile-package";
const PACKAGE_SCHEMA_VERSION: u32 = 1;
const MAX_PACKAGE_FILES: usize = 4096;
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfilePackageManifest {
    pub schema_version: u32,
    pub kind: String,
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub license: String,
    pub entry: String,
    pub contents: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ValidatedProfilePackage {
    pub manifest: ProfilePackageManifest,
    pub profile: SemanticProfile,
    pub root: PathBuf,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Error)]
pub(crate) enum ProfilePackageError {
    #[error("profile package I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid profile package: {0}")]
    Invalid(String),
    #[error("profile package {0} is already installed; pass --replace to replace it atomically")]
    AlreadyInstalled(String),
    #[error("profile package {0} is not installed")]
    NotInstalled(String),
}

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
    let store = flag_value(flags, "--profile-store").map(PathBuf::from);
    match command {
        "validate" => {
            let mut package = validate_package(Path::new(target))?;
            package.profile =
                resolve_profile_with_store(package.profile.clone(), store.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&package_summary(&package, "validated"))?
            );
        }
        "install" => {
            let package = install_package(
                Path::new(target),
                store.as_deref(),
                has_flag(flags, "--replace"),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&package_summary(&package, "installed"))?
            );
        }
        "uninstall" => {
            if !has_flag(flags, "--confirm") {
                return Err("profile uninstall requires --confirm".into());
            }
            let removed = uninstall_package(target, store.as_deref())?;
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

pub(crate) fn validate_package(
    root: &Path,
) -> Result<ValidatedProfilePackage, ProfilePackageError> {
    let root = canonical_directory(root)?;
    let manifest_path = root.join(MANIFEST_FILE);
    ensure_regular_file(&manifest_path)?;
    let manifest =
        serde_json::from_str::<ProfilePackageManifest>(&read_bounded(&manifest_path, 64 * 1024)?)
            .map_err(|error| ProfilePackageError::Invalid(format!("{MANIFEST_FILE}: {error}")))?;
    validate_manifest(&manifest)?;

    let mut declared = BTreeSet::new();
    let mut file_count = 1;
    let mut total_bytes = fs::metadata(&manifest_path)
        .map_err(|source| io_error(&manifest_path, source))?
        .len();
    for content in &manifest.contents {
        let relative = validated_relative_path(content)?;
        if !declared.insert(relative.clone()) {
            return Err(ProfilePackageError::Invalid(format!(
                "contents contains duplicate path {content}"
            )));
        }
        if relative == Path::new(MANIFEST_FILE) {
            return Err(ProfilePackageError::Invalid(format!(
                "{MANIFEST_FILE} is included automatically and must not appear in contents"
            )));
        }
        inspect_tree(&root.join(&relative), &mut file_count, &mut total_bytes)?;
    }
    if !declared.contains(Path::new(&manifest.entry)) {
        return Err(ProfilePackageError::Invalid(
            "entry must be declared in contents".into(),
        ));
    }
    if !declared.contains(Path::new("SKILL.md")) {
        return Err(ProfilePackageError::Invalid(
            "contents must include SKILL.md so agents receive the package policy".into(),
        ));
    }
    let entry_path = root.join(&manifest.entry);
    ensure_regular_file(&entry_path)?;
    let profile = parse_profile(&read_bounded(&entry_path, 1024 * 1024)?)
        .map_err(|error| ProfilePackageError::Invalid(format!("profile.json: {error}")))?;
    if profile.id != manifest.id {
        return Err(ProfilePackageError::Invalid(format!(
            "manifest id {} does not match profile id {}",
            manifest.id, profile.id
        )));
    }
    if profile.display_name != manifest.display_name {
        return Err(ProfilePackageError::Invalid(format!(
            "manifest display_name {:?} does not match profile display_name {:?}",
            manifest.display_name, profile.display_name
        )));
    }
    if profile.profile_version != manifest.version {
        return Err(ProfilePackageError::Invalid(format!(
            "manifest version {} does not match profile_version {}",
            manifest.version, profile.profile_version
        )));
    }
    Ok(ValidatedProfilePackage {
        manifest,
        profile,
        root,
        file_count,
        total_bytes,
    })
}

pub(crate) fn install_package(
    source: &Path,
    store: Option<&Path>,
    replace: bool,
) -> Result<ValidatedProfilePackage, ProfilePackageError> {
    let package = validate_package(source)?;
    let store = profile_store(store)?;
    fs::create_dir_all(&store).map_err(|source| io_error(&store, source))?;
    resolve_profile_with_store(package.profile.clone(), Some(&store))?;
    let destination = store.join(&package.manifest.id);
    if destination.exists() && !replace {
        return Err(ProfilePackageError::AlreadyInstalled(package.manifest.id));
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = store.join(format!(
        ".{}.install-{}-{nonce}",
        package.manifest.id,
        std::process::id()
    ));
    fs::create_dir(&staging).map_err(|source| io_error(&staging, source))?;
    let staged_result = (|| {
        copy_file(
            &package.root.join(MANIFEST_FILE),
            &staging.join(MANIFEST_FILE),
        )?;
        for content in &package.manifest.contents {
            let relative = validated_relative_path(content)?;
            copy_tree(&package.root.join(&relative), &staging.join(&relative))?;
        }
        validate_package(&staging)
    })();
    let staged = match staged_result {
        Ok(staged) => staged,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    let backup = store.join(format!(
        ".{}.backup-{}-{nonce}",
        package.manifest.id,
        std::process::id()
    ));
    if destination.exists() {
        fs::rename(&destination, &backup).map_err(|source| io_error(&destination, source))?;
    }
    if let Err(source) = fs::rename(&staging, &destination) {
        if backup.exists() {
            let _ = fs::rename(&backup, &destination);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(io_error(&destination, source));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|source| io_error(&backup, source))?;
    }
    validate_package(&destination).map(|installed| ValidatedProfilePackage {
        file_count: staged.file_count,
        total_bytes: staged.total_bytes,
        ..installed
    })
}

pub(crate) fn uninstall_package(
    id: &str,
    store: Option<&Path>,
) -> Result<PathBuf, ProfilePackageError> {
    validate_package_id(id)?;
    let store = profile_store(store)?;
    let target = store.join(id);
    if !target.is_dir() {
        return Err(ProfilePackageError::NotInstalled(id.into()));
    }
    validate_package(&target)?;
    fs::remove_dir_all(&target).map_err(|source| io_error(&target, source))?;
    Ok(target)
}

pub(crate) fn load_installed_profile(
    id: &str,
    store: Option<&Path>,
) -> Result<Option<SemanticProfile>, ProfilePackageError> {
    validate_package_id(id)?;
    let path = profile_store(store)?.join(id);
    if !path.is_dir() {
        return Ok(None);
    }
    let package = validate_package(&path)?;
    resolve_profile_with_store(package.profile, Some(&profile_store(store)?)).map(Some)
}

pub(crate) fn resolve_profile_with_store(
    profile: SemanticProfile,
    store: Option<&Path>,
) -> Result<SemanticProfile, ProfilePackageError> {
    let store = profile_store(store)?;
    resolve_profile_document(profile, &store, &mut BTreeSet::new())
}

fn resolve_profile_document(
    profile: SemanticProfile,
    store: &Path,
    visiting: &mut BTreeSet<String>,
) -> Result<SemanticProfile, ProfilePackageError> {
    let Some(parent_reference) = profile.extends.clone() else {
        return Ok(profile);
    };
    if !visiting.insert(profile.id.clone()) {
        return Err(ProfilePackageError::Invalid(format!(
            "profile inheritance cycle includes {}",
            profile.id
        )));
    }
    let parent = if let Some(parent) = builtin_profile(&parent_reference.id) {
        parent.clone()
    } else {
        let parent_path = store.join(&parent_reference.id);
        if !parent_path.is_dir() {
            return Err(ProfilePackageError::Invalid(format!(
                "profile {} requires missing parent {} {}",
                profile.id, parent_reference.id, parent_reference.version
            )));
        }
        validate_package(&parent_path)?.profile
    };
    let parent = resolve_profile_document(parent, store, visiting)?;
    visiting.remove(&profile.id);
    resolve_profile(&parent, &profile)
        .map_err(|error| ProfilePackageError::Invalid(error.to_string()))
}

pub(crate) fn installed_profile_summaries(
    store: Option<&Path>,
) -> Result<Vec<Value>, ProfilePackageError> {
    let store = profile_store(store)?;
    if !store.is_dir() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    for entry in fs::read_dir(&store).map_err(|source| io_error(&store, source))? {
        let entry = entry.map_err(|source| io_error(&store, source))?;
        let file_name = entry.file_name();
        let Some(id) = file_name.to_str() else {
            continue;
        };
        if id.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        match validate_package(&entry.path()).and_then(|mut package| {
            package.profile = resolve_profile_with_store(package.profile.clone(), Some(&store))?;
            Ok(package)
        }) {
            Ok(package) => summaries.push(package_summary(&package, "ready")),
            Err(error) => summaries.push(json!({
                "id": id,
                "source": "user",
                "status": "invalid",
                "path": entry.path(),
                "error": error.to_string(),
            })),
        }
    }
    Ok(summaries)
}

/// Return only profiles that are ready for runtime selection.
///
/// An unrelated broken package must not prevent matching a healthy Profile.
/// `installed_profile_summaries` remains the diagnostic surface that exposes
/// every invalid package and its error.
pub(crate) fn installed_ready_profiles(
    store: Option<&Path>,
) -> Result<Vec<SemanticProfile>, ProfilePackageError> {
    let store = profile_store(store)?;
    if !store.is_dir() {
        return Ok(Vec::new());
    }
    let mut profiles = Vec::new();
    for entry in fs::read_dir(&store).map_err(|source| io_error(&store, source))? {
        let entry = entry.map_err(|source| io_error(&store, source))?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if id.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        if let Ok(package) = validate_package(&entry.path())
            && let Ok(profile) = resolve_profile_with_store(package.profile.clone(), Some(&store))
        {
            profiles.push(profile);
        }
    }
    Ok(profiles)
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
        "capabilities": package.manifest.capabilities,
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

pub(crate) fn installed_package(
    id: &str,
    store: Option<&Path>,
) -> Result<Option<ValidatedProfilePackage>, ProfilePackageError> {
    validate_package_id(id)?;
    let path = profile_store(store)?.join(id);
    if !path.is_dir() {
        return Ok(None);
    }
    validate_package(&path).map(Some)
}

pub(crate) fn profile_store(override_path: Option<&Path>) -> Result<PathBuf, ProfilePackageError> {
    if let Some(path) = override_path {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env::var_os("DCC_CUA_HOME") {
        return Ok(PathBuf::from(path).join("profiles"));
    }
    let home =
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok_or_else(|| {
            ProfilePackageError::Invalid("cannot resolve the user home directory".into())
        })?;
    Ok(PathBuf::from(home).join(".dcc-cua").join("profiles"))
}

fn validate_manifest(manifest: &ProfilePackageManifest) -> Result<(), ProfilePackageError> {
    if manifest.schema_version != PACKAGE_SCHEMA_VERSION {
        return Err(ProfilePackageError::Invalid(format!(
            "unsupported package schema_version {}; expected {}",
            manifest.schema_version, PACKAGE_SCHEMA_VERSION
        )));
    }
    if manifest.kind != PACKAGE_KIND {
        return Err(ProfilePackageError::Invalid(format!(
            "kind must be {PACKAGE_KIND}"
        )));
    }
    validate_package_id(&manifest.id)?;
    if manifest.display_name.trim().is_empty()
        || manifest.description.trim().is_empty()
        || manifest.license.trim().is_empty()
    {
        return Err(ProfilePackageError::Invalid(
            "display_name, description, and license are required".into(),
        ));
    }
    if !is_semver(&manifest.version) {
        return Err(ProfilePackageError::Invalid(
            "version must be SemVer in major.minor.patch form".into(),
        ));
    }
    if manifest.entry != "profile.json" {
        return Err(ProfilePackageError::Invalid(
            "entry must be profile.json in package schema 1".into(),
        ));
    }
    if manifest.contents.is_empty() {
        return Err(ProfilePackageError::Invalid(
            "contents cannot be empty".into(),
        ));
    }
    Ok(())
}

fn validate_package_id(id: &str) -> Result<(), ProfilePackageError> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ProfilePackageError::Invalid(format!(
            "package id {id:?} must use 1-80 ASCII letters, numbers, '.', '_' or '-'"
        )));
    }
    Ok(())
}

fn is_semver(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts = core.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn validated_relative_path(value: &str) -> Result<PathBuf, ProfilePackageError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProfilePackageError::Invalid(format!(
            "content path {value:?} must be a normalized relative path"
        )));
    }
    Ok(path.to_path_buf())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ProfilePackageError> {
    let root = fs::canonicalize(path).map_err(|source| io_error(path, source))?;
    let metadata = fs::symlink_metadata(&root).map_err(|source| io_error(&root, source))?;
    if !metadata.is_dir() {
        return Err(ProfilePackageError::Invalid(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    Ok(root)
}

fn ensure_regular_file(path: &Path) -> Result<(), ProfilePackageError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ProfilePackageError::Invalid(format!(
            "{} must be a regular file",
            path.display()
        )));
    }
    Ok(())
}

fn inspect_tree(
    path: &Path,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), ProfilePackageError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.file_type().is_symlink() {
        return Err(ProfilePackageError::Invalid(format!(
            "symbolic links are not allowed in profile packages: {}",
            path.display()
        )));
    }
    if metadata.is_file() {
        *file_count += 1;
        *total_bytes = total_bytes.saturating_add(metadata.len());
        if *file_count > MAX_PACKAGE_FILES || *total_bytes > MAX_PACKAGE_BYTES {
            return Err(ProfilePackageError::Invalid(format!(
                "package exceeds {MAX_PACKAGE_FILES} files or {MAX_PACKAGE_BYTES} bytes"
            )));
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(ProfilePackageError::Invalid(format!(
            "unsupported package entry {}",
            path.display()
        )));
    }
    for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
        let entry = entry.map_err(|source| io_error(path, source))?;
        inspect_tree(&entry.path(), file_count, total_bytes)?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), ProfilePackageError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| io_error(source, error))?;
    if metadata.file_type().is_symlink() {
        return Err(ProfilePackageError::Invalid(format!(
            "symbolic links are not allowed in profile packages: {}",
            source.display()
        )));
    }
    if metadata.is_file() {
        return copy_file(source, destination);
    }
    fs::create_dir_all(destination).map_err(|error| io_error(destination, error))?;
    for entry in fs::read_dir(source).map_err(|error| io_error(source, error))? {
        let entry = entry.map_err(|error| io_error(source, error))?;
        copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), ProfilePackageError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    fs::copy(source, destination).map_err(|error| io_error(destination, error))?;
    Ok(())
}

fn read_bounded(path: &Path, maximum: u64) -> Result<String, ProfilePackageError> {
    ensure_regular_file(path)?;
    let metadata = fs::metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.len() > maximum {
        return Err(ProfilePackageError::Invalid(format!(
            "{} exceeds the {maximum}-byte limit",
            path.display()
        )));
    }
    fs::read_to_string(path).map_err(|source| io_error(path, source))
}

fn io_error(path: &Path, source: std::io::Error) -> ProfilePackageError {
    ProfilePackageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests;
