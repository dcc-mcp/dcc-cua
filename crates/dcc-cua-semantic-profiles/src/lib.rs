//! Application-specific semantic profiles kept outside the generic CUA host.
//!
//! The host owns exact-window scope, observations, input fencing, and safety.
//! This crate owns only stable application vocabulary and routing hints so an
//! Unreal, Maya, or Fab adapter can opt into deeper semantics without adding
//! product-specific branches to the host protocol.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticProfile {
    pub schema_version: u32,
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub selectors: Vec<ProfileSelector>,
    #[serde(default)]
    pub surfaces: Vec<SemanticSurface>,
    pub settings: ProfileSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSelector {
    #[serde(default)]
    pub application_names: Vec<String>,
    #[serde(default)]
    pub window_title_contains: Vec<String>,
    #[serde(default)]
    pub url_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticSurface {
    pub id: String,
    pub label: String,
    pub role: String,
    pub route: SemanticRoute,
    #[serde(default)]
    pub targets: Vec<SemanticTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticTarget {
    pub id: String,
    pub label: String,
    pub role: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub automation_ids: Vec<String>,
    #[serde(default)]
    pub supported_actions: Vec<String>,
}

impl SemanticTarget {
    #[must_use]
    pub fn supports_action(&self, action: &str) -> bool {
        self.supported_actions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(action))
    }

    #[must_use]
    pub fn matches_element(&self, element: &Value) -> bool {
        let values = ["name", "text", "value", "automation_id"]
            .into_iter()
            .filter_map(|field| element[field].as_str())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let role = element["role"].as_str().unwrap_or_default();
        let automation_id = element["automation_id"].as_str().unwrap_or_default();
        let role_matches = self.role.eq_ignore_ascii_case(role);
        let name_matches = self.names.iter().any(|name| {
            values
                .iter()
                .any(|value| value == &name.to_ascii_lowercase())
        });
        let automation_matches = self
            .automation_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(automation_id));
        if self.names.is_empty() && self.automation_ids.is_empty() {
            role_matches
        } else {
            role_matches && (name_matches || automation_matches)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRoute {
    UnrealTypedApi,
    MayaTypedApi,
    Accessibility,
    BrowserDom,
    OsNativeDialog,
    VisualFallback,
}

impl SemanticRoute {
    /// Whether this route's execution belongs to a typed DCC API owned by the
    /// DCC-MCP gateway. CUA never executes typed routes itself: the upstream
    /// driver has no host risk-classification extension point (ADR 0002,
    /// revised), so profiles only declare the routing intent and the host
    /// keeps exact-window UI observation/action as the fallback.
    #[must_use]
    pub const fn is_gateway_typed_api(self) -> bool {
        matches!(self, Self::UnrealTypedApi | Self::MayaTypedApi)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSettings {
    pub dialog_style: DialogStyle,
    pub preferred_route: SemanticRoute,
    #[serde(default)]
    pub destructive_confirmation_required: bool,
    /// Host denied-word markers this application may bypass (for example a
    /// license "Sign in" window). The host only honors an exemption when the
    /// caller also passes an explicit grant for the same profile; the profile
    /// alone never widens policy.
    #[serde(default)]
    pub denied_word_exemptions: Vec<String>,
}

impl ProfileSettings {
    /// True when this profile declares an exemption for `marker` (case
    /// insensitive). Callers must still require an explicit grant.
    #[must_use]
    pub fn exempts_denied_word(&self, marker: &str) -> bool {
        self.denied_word_exemptions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(marker))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogStyle {
    OsNative,
    HostOwned,
    ApplicationRendered,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProfileError {
    #[error("profile JSON is invalid: {0}")]
    InvalidJson(String),
    #[error("profile {0:?} has unsupported schema version {1}")]
    UnsupportedSchema(String, u32),
    #[error("profile id is required")]
    MissingId,
    #[error("profile {0:?} must define at least one selector or URL host")]
    MissingSelector(String),
    #[error("profile {0:?} contains duplicate surface id {1:?}")]
    DuplicateSurface(String, String),
    #[error("profile {0:?} surface {1:?} contains duplicate target id {2:?}")]
    DuplicateTarget(String, String, String),
    #[error("profile id {0:?} is already defined by a built-in or earlier external profile")]
    DuplicateProfile(String),
}

impl SemanticProfile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchema(
                self.id.clone(),
                self.schema_version,
            ));
        }
        if self.id.trim().is_empty() {
            return Err(ProfileError::MissingId);
        }
        let has_selector = self.selectors.iter().any(|selector| {
            !selector.application_names.is_empty()
                || !selector.window_title_contains.is_empty()
                || !selector.url_hosts.is_empty()
        });
        if !has_selector {
            return Err(ProfileError::MissingSelector(self.id.clone()));
        }

        let mut surface_ids = HashSet::new();
        for surface in &self.surfaces {
            if !surface_ids.insert(surface.id.as_str()) {
                return Err(ProfileError::DuplicateSurface(
                    self.id.clone(),
                    surface.id.clone(),
                ));
            }
            let mut target_ids = HashSet::new();
            for target in &surface.targets {
                if !target_ids.insert(target.id.as_str()) {
                    return Err(ProfileError::DuplicateTarget(
                        self.id.clone(),
                        surface.id.clone(),
                        target.id.clone(),
                    ));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn matches_window(&self, application_name: &str, window_title: &str) -> bool {
        self.selectors.iter().any(|selector| {
            let app_matches = selector.application_names.is_empty()
                || selector
                    .application_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(application_name));
            let title_matches = selector.window_title_contains.is_empty()
                || selector.window_title_contains.iter().any(|value| {
                    window_title
                        .to_ascii_lowercase()
                        .contains(&value.to_ascii_lowercase())
                });
            app_matches && title_matches
        })
    }

    #[must_use]
    pub fn matches_url(&self, url: &str) -> bool {
        let host = url_host(url);
        !host.is_empty()
            && self.selectors.iter().any(|selector| {
                !selector.url_hosts.is_empty()
                    && selector.url_hosts.iter().any(|candidate| {
                        let candidate = normalize_host(candidate);
                        host == candidate || host.ends_with(&format!(".{candidate}"))
                    })
            })
    }

    #[must_use]
    pub fn surface(&self, id: &str) -> Option<&SemanticSurface> {
        self.surfaces.iter().find(|surface| surface.id == id)
    }

    #[must_use]
    pub fn resolve_target(&self, surface_id: &str, query: &str) -> Option<&SemanticTarget> {
        let query = query.to_ascii_lowercase();
        self.surface(surface_id)?.targets.iter().find(|target| {
            target.id.eq_ignore_ascii_case(&query)
                || target.label.to_ascii_lowercase().contains(&query)
                || target
                    .names
                    .iter()
                    .any(|name| name.to_ascii_lowercase() == query)
                || target
                    .automation_ids
                    .iter()
                    .any(|automation_id| automation_id.eq_ignore_ascii_case(&query))
        })
    }

    /// Resolve a profile target and filter a host accessibility snapshot.
    ///
    /// The returned elements retain the host-provided element indexes/tokens;
    /// the profile only narrows semantic intent and never creates a new input
    /// path around the Host observation fence.
    #[must_use]
    pub fn find_elements<'a>(
        &'a self,
        surface_id: &str,
        root: &'a Value,
        query: &str,
    ) -> Vec<&'a Value> {
        let Some(target) = self.resolve_target(surface_id, query) else {
            return Vec::new();
        };
        root["elements"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|element| target.matches_element(element))
            .collect()
    }
}

fn normalize_host(value: &str) -> String {
    value.trim().trim_start_matches("www.").to_ascii_lowercase()
}

fn url_host(url: &str) -> String {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest);
    normalize_host(
        authority
            .split('/')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default(),
    )
}

pub fn parse_profile(input: &str) -> Result<SemanticProfile, ProfileError> {
    let profile = serde_json::from_str::<SemanticProfile>(input)
        .map_err(|error| ProfileError::InvalidJson(error.to_string()))?;
    profile.validate()?;
    Ok(profile)
}

pub fn builtin_profiles() -> &'static [SemanticProfile] {
    static PROFILES: OnceLock<Vec<SemanticProfile>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            [
                include_str!("../profiles/ue.json"),
                include_str!("../profiles/maya.json"),
                include_str!("../profiles/fab.json"),
            ]
            .into_iter()
            .map(|profile| parse_profile(profile).expect("built-in semantic profile is valid"))
            .collect()
        })
        .as_slice()
}

#[must_use]
pub fn builtin_profile(id: &str) -> Option<&'static SemanticProfile> {
    let normalized = id.trim().to_ascii_lowercase();
    builtin_profiles().iter().find(|profile| {
        profile.id == normalized
            || (normalized == "unreal" && profile.id == "ue")
            || (normalized == "unreal-engine" && profile.id == "ue")
    })
}

/// Environment variable naming a directory of external `*.json` profiles.
pub const PROFILE_DIR_ENV: &str = "DCC_CUA_PROFILE_DIR";

/// Load and validate every `*.json` profile in `directory`.
///
/// Fails closed: one invalid profile rejects the whole directory so a typo
/// cannot silently drop an application's routing or safety settings. External
/// profiles may not reuse a built-in id — built-ins always win.
pub fn load_profiles_from_dir(
    directory: &std::path::Path,
) -> Result<Vec<SemanticProfile>, ProfileError> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| ProfileError::InvalidJson(format!("{}: {error}", directory.display())))?;
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    let mut profiles = Vec::new();
    for path in paths {
        let input = std::fs::read_to_string(&path)
            .map_err(|error| ProfileError::InvalidJson(format!("{}: {error}", path.display())))?;
        let profile = parse_profile(&input)?;
        if builtin_profile(&profile.id).is_some()
            || profiles
                .iter()
                .any(|existing: &SemanticProfile| existing.id == profile.id)
        {
            return Err(ProfileError::DuplicateProfile(profile.id));
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

/// Built-in profiles plus any external directory named by
/// [`PROFILE_DIR_ENV`]. External load errors fail closed to built-ins only.
#[must_use]
pub fn all_profiles() -> Vec<SemanticProfile> {
    let mut profiles: Vec<SemanticProfile> = builtin_profiles().to_vec();
    if let Some(directory) = std::env::var_os(PROFILE_DIR_ENV)
        && let Ok(external) = load_profiles_from_dir(std::path::Path::new(&directory))
    {
        profiles.extend(external);
    }
    profiles
}

#[cfg(test)]
mod tests;
