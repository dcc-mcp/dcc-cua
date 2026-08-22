use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dcc_cua_semantic_profiles::{SemanticProfile, builtin_profile, resolve_profile};
use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ProfileCatalog, ProfileContext, ProfileContextRequest};

pub(crate) const MANIFEST_FILE: &str = "profile-package.json";
const PACKAGE_KIND: &str = "dcc-cua-profile-package";
const PACKAGE_SCHEMA_VERSION: u32 = 2;
const MAX_PACKAGE_FILES: usize = 4096;
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePackageManifest {
    pub schema_version: u32,
    pub kind: String,
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub license: String,
    pub artifacts: Vec<ProfilePackageArtifact>,
    pub requires: ProfilePackageRequirements,
    #[serde(default)]
    pub platforms: Vec<ProfilePlatform>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePackageArtifact {
    #[serde(rename = "type")]
    pub artifact_type: ProfilePackageArtifactType,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfilePackageArtifactType {
    SemanticProfile,
    ContextIndex,
    ContextDocument,
    AgentSkill,
    Documentation,
    Fixtures,
    CompanionSource,
    License,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfilePlatform {
    Windows,
    Macos,
    Linux,
}

impl ProfilePlatform {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePackageRequirements {
    pub dcc_cua: String,
}

#[derive(Clone, Debug)]
pub struct ValidatedProfilePackage {
    pub manifest: ProfilePackageManifest,
    pub profile: SemanticProfile,
    pub root: PathBuf,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Clone, Debug)]
pub enum ProfileStoreEntry {
    Ready(Box<ValidatedProfilePackage>),
    Invalid {
        id: String,
        path: PathBuf,
        error: String,
    },
}

impl ProfileStoreEntry {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Ready(package) => &package.manifest.id,
            Self::Invalid { id, .. } => id,
        }
    }

    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[derive(Debug, Error)]
pub enum ProfilePackageError {
    #[error("profile package I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid profile package: {0}")]
    Invalid(String),
    #[error("profile package {0} is already installed; replace must be explicitly enabled")]
    AlreadyInstalled(String),
    #[error("profile package {0} is not installed")]
    NotInstalled(String),
}

#[derive(Debug)]
pub struct ProfileStore {
    root: PathBuf,
    entries: Vec<ProfileStoreEntry>,
}

impl ProfileStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProfilePackageError> {
        let mut store = Self {
            root: root.as_ref().to_path_buf(),
            entries: Vec::new(),
        };
        store.refresh()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self, ProfilePackageError> {
        Self::open(default_profile_store()?)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn entries(&self) -> &[ProfileStoreEntry] {
        &self.entries
    }

    pub fn refresh(&mut self) -> Result<(), ProfilePackageError> {
        self.entries.clear();
        if !self.root.is_dir() {
            return Ok(());
        }

        let mut paths = fs::read_dir(&self.root)
            .map_err(|source| io_error(&self.root, source))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| io_error(&self.root, source))?;
        paths.sort();

        let mut packages = BTreeMap::new();
        let mut invalid = Vec::new();
        for path in paths {
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<non-utf8>")
                .to_owned();
            if id.starts_with('.') || !path.is_dir() {
                continue;
            }
            match validate_package(&path) {
                Ok(package) => {
                    if packages
                        .insert(package.manifest.id.clone(), package)
                        .is_some()
                    {
                        invalid.push(ProfileStoreEntry::Invalid {
                            id,
                            path,
                            error: "duplicate package identity".into(),
                        });
                    }
                }
                Err(error) => invalid.push(ProfileStoreEntry::Invalid {
                    id,
                    path,
                    error: error.to_string(),
                }),
            }
        }

        let mut candidates = packages;
        let unsupported = candidates
            .iter()
            .filter(|(_, package)| !package_supports_current_platform(package))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in unsupported {
            let package = candidates.remove(&id).expect("candidate exists");
            invalid.push(ProfileStoreEntry::Invalid {
                id,
                path: package.root,
                error: format!(
                    "package does not support the current {:?} platform",
                    ProfilePlatform::current()
                ),
            });
        }
        let resolved = loop {
            let mut resolved = BTreeMap::new();
            let failures = candidates
                .iter()
                .filter_map(|(id, package)| {
                    match resolve_profile_from_packages(
                        package.profile.clone(),
                        &candidates,
                        &mut BTreeSet::new(),
                    ) {
                        Ok(profile) => {
                            let mut package = package.clone();
                            package.profile = profile;
                            resolved.insert(id.clone(), package);
                            None
                        }
                        Err(error) => Some((id.clone(), error.to_string())),
                    }
                })
                .collect::<Vec<_>>();
            if failures.is_empty() {
                break resolved;
            }
            for (id, error) in failures {
                let package = candidates.remove(&id).expect("candidate exists");
                invalid.push(ProfileStoreEntry::Invalid {
                    id,
                    path: package.root,
                    error,
                });
            }
        };
        self.entries.extend(
            resolved
                .into_values()
                .map(Box::new)
                .map(ProfileStoreEntry::Ready),
        );
        self.entries.extend(invalid);
        self.entries
            .sort_by(|left, right| left.id().cmp(right.id()));
        loop {
            let catalog = ProfileCatalog::from_store(self);
            let failures = self
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    ProfileStoreEntry::Ready(package) => catalog
                        .validate_profile(&package.profile)
                        .err()
                        .map(|error| (package.manifest.id.clone(), error.to_string())),
                    ProfileStoreEntry::Invalid { .. } => None,
                })
                .collect::<BTreeMap<_, _>>();
            if failures.is_empty() {
                break;
            }
            self.entries = self
                .entries
                .drain(..)
                .map(|entry| match entry {
                    ProfileStoreEntry::Ready(package) => {
                        if let Some(error) = failures.get(&package.manifest.id) {
                            ProfileStoreEntry::Invalid {
                                id: package.manifest.id.clone(),
                                path: package.root.clone(),
                                error: error.clone(),
                            }
                        } else {
                            ProfileStoreEntry::Ready(package)
                        }
                    }
                    invalid @ ProfileStoreEntry::Invalid { .. } => invalid,
                })
                .collect();
        }
        Ok(())
    }

    pub fn profile(&self, id: &str) -> Result<Option<&SemanticProfile>, ProfilePackageError> {
        Ok(self.package(id)?.map(|package| &package.profile))
    }

    pub fn package(
        &self,
        id: &str,
    ) -> Result<Option<&ValidatedProfilePackage>, ProfilePackageError> {
        validate_package_id(id)?;
        match self.entries.iter().find(|entry| entry.id() == id) {
            Some(ProfileStoreEntry::Ready(package)) => Ok(Some(package)),
            Some(ProfileStoreEntry::Invalid { error, .. }) => {
                Err(ProfilePackageError::Invalid(error.clone()))
            }
            None => Ok(None),
        }
    }

    #[must_use]
    pub fn ready_profiles(&self) -> Vec<&SemanticProfile> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                ProfileStoreEntry::Ready(package) => Some(&package.profile),
                ProfileStoreEntry::Invalid { .. } => None,
            })
            .collect()
    }

    #[must_use]
    pub fn catalog(&self) -> ProfileCatalog {
        ProfileCatalog::from_store(self)
    }

    pub fn resolve(
        &self,
        profile: SemanticProfile,
    ) -> Result<SemanticProfile, ProfilePackageError> {
        resolve_profile_against_store(profile, self)
    }

    pub fn context(
        &self,
        request: ProfileContextRequest,
    ) -> Result<ProfileContext, ProfilePackageError> {
        crate::context::select_context(self, request)
    }

    pub fn install(
        &mut self,
        source: &Path,
        replace: bool,
    ) -> Result<ValidatedProfilePackage, ProfilePackageError> {
        let package = validate_package(source)?;
        if !package_supports_current_platform(&package) {
            return Err(ProfilePackageError::Invalid(format!(
                "package does not support the current {:?} platform",
                ProfilePlatform::current()
            )));
        }
        resolve_profile_against_store(package.profile.clone(), self)?;
        fs::create_dir_all(&self.root).map_err(|source| io_error(&self.root, source))?;
        let destination = self.root.join(&package.manifest.id);
        if destination.exists() && !replace {
            return Err(ProfilePackageError::AlreadyInstalled(package.manifest.id));
        }

        let nonce = unique_nonce();
        let staging = self.root.join(format!(
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
            for artifact in &package.manifest.artifacts {
                let relative = validated_relative_path(&artifact.path)?;
                copy_tree(&package.root.join(&relative), &staging.join(&relative))?;
            }
            validate_package(&staging)
        })();
        if let Err(error) = staged_result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let backup = self.root.join(format!(
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

        let refresh_result = self.refresh().and_then(|()| {
            self.package(&package.manifest.id)?
                .cloned()
                .ok_or_else(|| ProfilePackageError::NotInstalled(package.manifest.id.clone()))
        });
        match refresh_result {
            Ok(installed) => {
                if backup.exists() {
                    fs::remove_dir_all(&backup).map_err(|source| io_error(&backup, source))?;
                }
                Ok(installed)
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&destination);
                if backup.exists() {
                    let _ = fs::rename(&backup, &destination);
                }
                let _ = self.refresh();
                Err(error)
            }
        }
    }

    pub fn uninstall(&mut self, id: &str) -> Result<PathBuf, ProfilePackageError> {
        validate_package_id(id)?;
        self.package(id)?
            .ok_or_else(|| ProfilePackageError::NotInstalled(id.into()))?;
        let target = self.root.join(id);
        let ready_before = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                ProfileStoreEntry::Ready(package) if package.manifest.id != id => {
                    Some(package.manifest.id.clone())
                }
                ProfileStoreEntry::Ready(_) | ProfileStoreEntry::Invalid { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        let tombstone = self.root.join(format!(
            ".{id}.uninstall-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::rename(&target, &tombstone).map_err(|source| io_error(&target, source))?;
        let refresh_result = self.refresh();
        let ready_after = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                ProfileStoreEntry::Ready(package) => Some(package.manifest.id.clone()),
                ProfileStoreEntry::Invalid { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        let broken = ready_before
            .difference(&ready_after)
            .cloned()
            .collect::<Vec<_>>();
        if let Err(error) = refresh_result {
            let _ = fs::rename(&tombstone, &target);
            let _ = self.refresh();
            return Err(error);
        }
        if !broken.is_empty() {
            fs::rename(&tombstone, &target).map_err(|source| io_error(&target, source))?;
            self.refresh()?;
            return Err(ProfilePackageError::Invalid(format!(
                "cannot uninstall {id}; installed profiles depend on it: {}",
                broken.join(", ")
            )));
        }
        fs::remove_dir_all(&tombstone).map_err(|source| io_error(&tombstone, source))?;
        Ok(target)
    }
}

pub fn default_profile_store() -> Result<PathBuf, ProfilePackageError> {
    if let Some(path) = env::var_os("DCC_CUA_HOME") {
        return Ok(PathBuf::from(path).join("profiles"));
    }
    let home =
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok_or_else(|| {
            ProfilePackageError::Invalid("cannot resolve the user home directory".into())
        })?;
    Ok(PathBuf::from(home).join(".dcc-cua").join("profiles"))
}

pub fn validate_package(root: &Path) -> Result<ValidatedProfilePackage, ProfilePackageError> {
    let root = canonical_directory(root)?;
    let manifest_path = root.join(MANIFEST_FILE);
    ensure_regular_file(&manifest_path)?;
    let manifest =
        serde_json::from_slice::<ProfilePackageManifest>(&read_bounded(&manifest_path, 64 * 1024)?)
            .map_err(|error| ProfilePackageError::Invalid(format!("{MANIFEST_FILE}: {error}")))?;
    validate_manifest(&manifest)?;

    let mut declared = BTreeSet::new();
    let mut file_count = 1;
    let mut total_bytes = fs::metadata(&manifest_path)
        .map_err(|source| io_error(&manifest_path, source))?
        .len();
    for artifact in &manifest.artifacts {
        let relative = validated_relative_path(&artifact.path)?;
        if !declared.insert(relative.clone()) {
            return Err(ProfilePackageError::Invalid(format!(
                "artifacts contains duplicate path {}",
                artifact.path
            )));
        }
        if relative == Path::new(MANIFEST_FILE) {
            return Err(ProfilePackageError::Invalid(format!(
                "{MANIFEST_FILE} is included automatically and must not be an artifact"
            )));
        }
        inspect_tree(&root.join(&relative), &mut file_count, &mut total_bytes)?;
    }
    let entry = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.artifact_type == ProfilePackageArtifactType::SemanticProfile)
        .expect("validated semantic profile artifact");
    let entry_path = root.join(&entry.path);
    ensure_regular_file(&entry_path)?;
    let profile_json = String::from_utf8(read_bounded(&entry_path, 1024 * 1024)?)
        .map_err(|error| ProfilePackageError::Invalid(format!("profile.json: {error}")))?;
    let profile = dcc_cua_semantic_profiles::parse_profile(&profile_json)
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

fn validate_manifest(manifest: &ProfilePackageManifest) -> Result<(), ProfilePackageError> {
    if manifest.schema_version != PACKAGE_SCHEMA_VERSION {
        return Err(ProfilePackageError::Invalid(format!(
            "unsupported package schema_version {}; expected {PACKAGE_SCHEMA_VERSION}",
            manifest.schema_version
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
    Version::parse(&manifest.version)
        .map_err(|_| ProfilePackageError::Invalid("version must be valid SemVer".into()))?;
    if manifest.artifacts.is_empty() {
        return Err(ProfilePackageError::Invalid(
            "artifacts cannot be empty".into(),
        ));
    }
    let minimum = manifest
        .requires
        .dcc_cua
        .strip_prefix(">=")
        .ok_or_else(|| {
            ProfilePackageError::Invalid(
                "requires.dcc_cua must use the >=major.minor.patch form".into(),
            )
        })?;
    let required = Version::parse(minimum).map_err(|_| {
        ProfilePackageError::Invalid("requires.dcc_cua contains invalid SemVer".into())
    })?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is SemVer");
    if current < required {
        return Err(ProfilePackageError::Invalid(format!(
            "profile requires dcc-cua {}, but this runtime is {current}",
            manifest.requires.dcc_cua
        )));
    }
    let semantic_profiles = manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.artifact_type == ProfilePackageArtifactType::SemanticProfile)
        .count();
    if semantic_profiles != 1 {
        return Err(ProfilePackageError::Invalid(
            "artifacts must declare exactly one semantic_profile".into(),
        ));
    }
    Ok(())
}

fn package_supports_current_platform(package: &ValidatedProfilePackage) -> bool {
    package.manifest.platforms.is_empty()
        || package
            .manifest
            .platforms
            .contains(&ProfilePlatform::current())
}

fn resolve_profile_against_store(
    profile: SemanticProfile,
    store: &ProfileStore,
) -> Result<SemanticProfile, ProfilePackageError> {
    let Some(parent_reference) = profile.extends.clone() else {
        return Ok(profile);
    };
    let parent = if let Some(parent) = builtin_profile(&parent_reference.id) {
        parent
    } else {
        store.profile(&parent_reference.id)?.ok_or_else(|| {
            ProfilePackageError::Invalid(format!(
                "profile {} requires missing parent {} {}",
                profile.id, parent_reference.id, parent_reference.version
            ))
        })?
    };
    resolve_profile(parent, &profile)
        .map_err(|error| ProfilePackageError::Invalid(error.to_string()))
}

fn resolve_profile_from_packages(
    profile: SemanticProfile,
    packages: &BTreeMap<String, ValidatedProfilePackage>,
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
        packages
            .get(&parent_reference.id)
            .map(|package| package.profile.clone())
            .ok_or_else(|| {
                ProfilePackageError::Invalid(format!(
                    "profile {} requires missing parent {} {}",
                    profile.id, parent_reference.id, parent_reference.version
                ))
            })?
    };
    let parent = resolve_profile_from_packages(parent, packages, visiting)?;
    visiting.remove(&profile.id);
    resolve_profile(&parent, &profile)
        .map_err(|error| ProfilePackageError::Invalid(error.to_string()))
}

fn validate_package_id(id: &str) -> Result<(), ProfilePackageError> {
    if id.is_empty()
        || id.len() > 80
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(ProfilePackageError::Invalid(format!(
            "package id {id:?} must use 1-80 lowercase ASCII letters, numbers, '.', '_' or '-'"
        )));
    }
    Ok(())
}

pub(crate) fn validated_relative_path(value: &str) -> Result<PathBuf, ProfilePackageError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProfilePackageError::Invalid(format!(
            "artifact path {value:?} must be a normalized relative path"
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

pub(crate) fn ensure_regular_file(path: &Path) -> Result<(), ProfilePackageError> {
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

pub(crate) fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, ProfilePackageError> {
    ensure_regular_file(path)?;
    let metadata = fs::metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.len() > maximum {
        return Err(ProfilePackageError::Invalid(format!(
            "{} exceeds the {maximum}-byte limit",
            path.display()
        )));
    }
    fs::read(path).map_err(|source| io_error(path, source))
}

pub(crate) fn io_error(path: &Path, source: std::io::Error) -> ProfilePackageError {
    ProfilePackageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
