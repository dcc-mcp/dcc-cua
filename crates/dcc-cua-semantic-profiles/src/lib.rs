//! Application-specific semantic profiles kept outside the generic CUA host.
//!
//! The host owns exact-window scope, observations, input fencing, and safety.
//! This crate owns only stable application vocabulary and routing hints so an
//! Unreal, Maya, or Fab adapter can opt into deeper semantics without adding
//! product-specific branches to the host protocol.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::OnceLock;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

pub const PROFILE_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticProfile {
    pub schema_version: u32,
    pub id: String,
    pub profile_version: String,
    pub application: ProfileApplication,
    #[serde(default)]
    pub extends: Option<ProfileReference>,
    pub display_name: String,
    #[serde(default)]
    pub selectors: Vec<ProfileSelector>,
    #[serde(default)]
    pub surfaces: Vec<SemanticSurface>,
    #[serde(default)]
    pub state_sources: Vec<StateSource>,
    #[serde(default)]
    pub binding: ProfileBinding,
    #[serde(default)]
    pub capability_probes: Vec<CapabilityProbe>,
    #[serde(default)]
    pub flows: Vec<ProfileFlow>,
    pub settings: ProfileSettings,
}

/// Exact-window and version fencing requirements shared by every application
/// profile.  These are declarative policy; the Host remains responsible for
/// supplying the actual PID/HWND and version evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBinding {
    #[serde(default = "default_true")]
    pub require_exact_pid: bool,
    #[serde(default = "default_true")]
    pub require_exact_window_handle: bool,
    #[serde(default = "default_true")]
    pub require_window_version_match: bool,
    #[serde(default = "default_true")]
    pub fail_closed_on_ambiguity: bool,
}

impl Default for ProfileBinding {
    fn default() -> Self {
        Self {
            require_exact_pid: true,
            require_exact_window_handle: true,
            require_window_version_match: true,
            fail_closed_on_ambiguity: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityProbe {
    pub id: String,
    pub route: SemanticRoute,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFlow {
    pub id: String,
    pub surface_id: String,
    pub action_target_id: String,
    pub verify_target_id: String,
    pub route: SemanticRoute,
    #[serde(default = "default_true")]
    pub requires_fresh_snapshot: bool,
    #[serde(default = "default_true")]
    pub requires_post_action_verification: bool,
    #[serde(default = "default_true")]
    pub prohibit_coordinates: bool,
    #[serde(default = "default_true")]
    pub prohibit_keyboard_shortcuts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileApplication {
    pub family: String,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileReference {
    pub id: String,
    pub version: String,
}

/// A bounded, read-only application state source declared by a profile.
///
/// Profiles describe how to reach state that is already exposed by a trusted
/// local application companion. They never declare a process to launch,
/// credentials, arbitrary headers, or an action endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSource {
    pub id: String,
    #[serde(rename = "type")]
    pub source_type: StateSourceType,
    pub mode: StateSourceMode,
    pub url: String,
    pub expected_schema_version: String,
    pub schema_version_pointer: String,
    pub tick_pointer: String,
    #[serde(default)]
    pub use_etag: bool,
    pub timeout_ms: u64,
    pub max_response_bytes: u64,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateSourceType {
    LoopbackHttpJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateSourceMode {
    ReadOnly,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSelector {
    #[serde(default)]
    pub application_names: Vec<String>,
    #[serde(default)]
    pub window_title_contains: Vec<String>,
    #[serde(default)]
    pub localized_window_title_contains: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub url_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSurface {
    pub id: String,
    pub label: String,
    pub role: String,
    pub route: SemanticRoute,
    #[serde(default)]
    pub targets: Vec<SemanticTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticTarget {
    pub id: String,
    pub label: String,
    pub role: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub localized_names: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub automation_ids: Vec<String>,
    #[serde(default)]
    pub supported_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub key_bindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub fallback: Option<SemanticFallback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticFallback {
    pub profile_id: String,
    pub surface_id: String,
}

impl SemanticTarget {
    #[must_use]
    pub fn supports_action(&self, action: &str) -> bool {
        self.supported_actions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(action))
    }

    #[must_use]
    pub fn key_binding(&self, action: &str) -> Option<&[String]> {
        self.key_bindings
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(action))
            .map(|(_, keys)| keys.as_slice())
    }

    #[must_use]
    pub fn matches_element(&self, element: &Value) -> bool {
        let values = ["name", "text", "value", "automation_id"]
            .into_iter()
            .filter_map(|field| element[field].as_str())
            .map(normalize_text)
            .collect::<Vec<_>>();
        let role = element["role"].as_str().unwrap_or_default();
        let automation_id = element["automation_id"].as_str().unwrap_or_default();
        let role_matches = normalize_text(&self.role) == normalize_text(role);
        let has_names = !self.names.is_empty()
            || self
                .localized_names
                .values()
                .any(|aliases| !aliases.is_empty());
        let name_matches = self
            .names
            .iter()
            .chain(self.localized_names.values().flatten())
            .any(|name| {
                let name = normalize_text(name);
                values.iter().any(|value| value == &name)
            });
        let automation_matches = self
            .automation_ids
            .iter()
            .any(|id| normalize_text(id) == normalize_text(automation_id));
        if !has_names && self.automation_ids.is_empty() {
            role_matches
        } else {
            role_matches && (name_matches || automation_matches)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRoute {
    UnrealTypedApi,
    Accessibility,
    BrowserDom,
    OsNativeDialog,
    VisualFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSettings {
    pub dialog_style: DialogStyle,
    pub preferred_route: SemanticRoute,
    #[serde(default)]
    pub default_locale: Option<String>,
    #[serde(default = "default_true")]
    pub destructive_confirmation_required: bool,
}

const fn default_true() -> bool {
    true
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
    #[error("profile {0:?} contains non-canonical identifier {1:?}: {2:?}")]
    InvalidIdentifier(String, String, String),
    #[error("profile {0:?} has invalid profile_version {1:?}")]
    InvalidProfileVersion(String, String),
    #[error("profile {0:?} has invalid application compatibility")]
    InvalidApplication(String),
    #[error("profile {0:?} has invalid parent reference")]
    InvalidParent(String),
    #[error("profile {0:?} requires parent {1:?} matching {2:?}")]
    ParentVersionMismatch(String, String, String),
    #[error("profile {0:?} cannot inherit application family {1:?}")]
    ParentApplicationMismatch(String, String),
    #[error("profile {0:?} must define at least one selector or URL host")]
    MissingSelector(String),
    #[error("profile {0:?} contains duplicate surface id {1:?}")]
    DuplicateSurface(String, String),
    #[error("profile {0:?} surface {1:?} contains duplicate target id {2:?}")]
    DuplicateTarget(String, String, String),
    #[error("profile {0:?} target {1:?} contains an empty fallback")]
    InvalidFallback(String, String),
    #[error("profile {0:?} target {1:?} contains invalid key binding for {2:?}")]
    InvalidKeyBinding(String, String, String),
    #[error("profile {0:?} contains invalid locale tag {1:?}")]
    InvalidLocale(String, String),
    #[error("profile {0:?} locale {1:?} must contain non-empty aliases")]
    InvalidLocalizedAliases(String, String),
    #[error("profile {0:?} contains duplicate state source id {1:?}")]
    DuplicateStateSource(String, String),
    #[error("profile {0:?} state source {1:?} must use a literal HTTP loopback URL")]
    InvalidStateSourceUrl(String, String),
    #[error("profile {0:?} state source {1:?} has an invalid state contract")]
    InvalidStateContract(String, String),
    #[error("profile {0:?} state source {1:?} exceeds runtime bounds")]
    InvalidStateSourceBounds(String, String),
    #[error("profile {0:?} contains duplicate capability probe id {1:?}")]
    DuplicateCapabilityProbe(String, String),
    #[error("profile {0:?} capability probe {1:?} must declare at least one capability")]
    EmptyCapabilityProbe(String, String),
    #[error("profile {0:?} capability probe {1:?} uses an uncontrolled route")]
    InvalidCapabilityProbeRoute(String, String),
    #[error("profile {0:?} contains duplicate flow id {1:?}")]
    DuplicateFlow(String, String),
    #[error("profile {0:?} flow {1:?} references unknown surface or target")]
    InvalidFlowReference(String, String),
    #[error("profile {0:?} flow {1:?} route does not match its surface")]
    FlowRouteMismatch(String, String),
    #[error("profile {0:?} flow {1:?} does not declare click/verify targets")]
    InvalidFlowActions(String, String),
    #[error(
        "profile {0:?} flow {1:?} weakens the snapshot, verification, or coordinate safety policy"
    )]
    UnsafeFlowPolicy(String, String),
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
        validate_identifier(&self.id, &self.id, "id")?;
        if Version::parse(&self.profile_version).is_err() {
            return Err(ProfileError::InvalidProfileVersion(
                self.id.clone(),
                self.profile_version.clone(),
            ));
        }
        if self.application.family.trim().is_empty()
            || self.application.family.chars().count() > 80
            || self
                .application
                .versions
                .iter()
                .any(|version| version.trim().is_empty() || version.chars().count() > 32)
        {
            return Err(ProfileError::InvalidApplication(self.id.clone()));
        }
        if let Some(parent) = &self.extends
            && (parent.id.trim().is_empty()
                || parent.id == self.id
                || VersionReq::parse(&parent.version).is_err())
        {
            return Err(ProfileError::InvalidParent(self.id.clone()));
        }
        let has_selector = self.selectors.iter().any(|selector| {
            !selector.application_names.is_empty()
                || !selector.window_title_contains.is_empty()
                || selector
                    .localized_window_title_contains
                    .values()
                    .any(|aliases| !aliases.is_empty())
                || !selector.url_hosts.is_empty()
        });
        // A child may inherit selectors from its parent. The resolved profile
        // is validated again by `resolve_profile`, so an inheritance chain can
        // never produce an executable profile without a selector.
        if !has_selector && self.extends.is_none() {
            return Err(ProfileError::MissingSelector(self.id.clone()));
        }
        if let Some(locale) = &self.settings.default_locale
            && !valid_locale_tag(locale)
        {
            return Err(ProfileError::InvalidLocale(self.id.clone(), locale.clone()));
        }
        for selector in &self.selectors {
            validate_localized_aliases(&self.id, &selector.localized_window_title_contains)?;
        }

        let mut state_source_ids = HashSet::new();
        for source in &self.state_sources {
            validate_identifier(&self.id, &source.id, "state_sources[].id")?;
            if !state_source_ids.insert(source.id.as_str()) {
                return Err(ProfileError::DuplicateStateSource(
                    self.id.clone(),
                    source.id.clone(),
                ));
            }
            source.validate(&self.id)?;
        }

        let mut surface_ids = HashSet::new();
        for surface in &self.surfaces {
            validate_identifier(&self.id, &surface.id, "surfaces[].id")?;
            if !surface_ids.insert(surface.id.as_str()) {
                return Err(ProfileError::DuplicateSurface(
                    self.id.clone(),
                    surface.id.clone(),
                ));
            }
            let mut target_ids = HashSet::new();
            for target in &surface.targets {
                validate_identifier(&self.id, &target.id, "surfaces[].targets[].id")?;
                if !target_ids.insert(target.id.as_str()) {
                    return Err(ProfileError::DuplicateTarget(
                        self.id.clone(),
                        surface.id.clone(),
                        target.id.clone(),
                    ));
                }
                if let Some(fallback) = &target.fallback {
                    validate_identifier(
                        &self.id,
                        &fallback.profile_id,
                        "surfaces[].targets[].fallback.profile_id",
                    )?;
                    validate_identifier(
                        &self.id,
                        &fallback.surface_id,
                        "surfaces[].targets[].fallback.surface_id",
                    )?;
                }
                for (action, keys) in &target.key_bindings {
                    if !target.supports_action(action)
                        || keys.is_empty()
                        || keys.len() > 4
                        || keys
                            .iter()
                            .any(|key| key.is_empty() || key.chars().count() > 32)
                    {
                        return Err(ProfileError::InvalidKeyBinding(
                            self.id.clone(),
                            target.id.clone(),
                            action.clone(),
                        ));
                    }
                }
                validate_localized_aliases(&self.id, &target.localized_names)?;
            }
        }
        let mut probe_ids = HashSet::new();
        for probe in &self.capability_probes {
            validate_identifier(&self.id, &probe.id, "capability_probes[].id")?;
            if !probe_ids.insert(probe.id.as_str()) {
                return Err(ProfileError::DuplicateCapabilityProbe(
                    self.id.clone(),
                    probe.id.clone(),
                ));
            }
            if probe.capabilities.is_empty()
                || probe
                    .capabilities
                    .iter()
                    .any(|capability| capability.trim().is_empty() || capability.len() > 128)
            {
                return Err(ProfileError::EmptyCapabilityProbe(
                    self.id.clone(),
                    probe.id.clone(),
                ));
            }
            if !matches!(
                probe.route,
                SemanticRoute::BrowserDom | SemanticRoute::Accessibility
            ) {
                return Err(ProfileError::InvalidCapabilityProbeRoute(
                    self.id.clone(),
                    probe.id.clone(),
                ));
            }
        }
        let mut flow_ids = HashSet::new();
        for flow in &self.flows {
            validate_identifier(&self.id, &flow.id, "flows[].id")?;
            if !flow_ids.insert(flow.id.as_str()) {
                return Err(ProfileError::DuplicateFlow(
                    self.id.clone(),
                    flow.id.clone(),
                ));
            }
            let Some(surface) = self.surface(&flow.surface_id) else {
                return Err(ProfileError::InvalidFlowReference(
                    self.id.clone(),
                    flow.id.clone(),
                ));
            };
            let action_exists = surface
                .targets
                .iter()
                .any(|target| target.id == flow.action_target_id);
            let verify_exists = surface
                .targets
                .iter()
                .any(|target| target.id == flow.verify_target_id);
            if !action_exists || !verify_exists {
                return Err(ProfileError::InvalidFlowReference(
                    self.id.clone(),
                    flow.id.clone(),
                ));
            }
            let action_target = surface
                .targets
                .iter()
                .find(|target| target.id == flow.action_target_id)
                .expect("action target checked above");
            let verify_target = surface
                .targets
                .iter()
                .find(|target| target.id == flow.verify_target_id)
                .expect("verify target checked above");
            if !action_target.supports_action("click") || !verify_target.supports_action("verify") {
                return Err(ProfileError::InvalidFlowActions(
                    self.id.clone(),
                    flow.id.clone(),
                ));
            }
            if surface.route != flow.route {
                return Err(ProfileError::FlowRouteMismatch(
                    self.id.clone(),
                    flow.id.clone(),
                ));
            }
            if !flow.requires_fresh_snapshot
                || !flow.requires_post_action_verification
                || !flow.prohibit_coordinates
                || !flow.prohibit_keyboard_shortcuts
            {
                return Err(ProfileError::UnsafeFlowPolicy(
                    self.id.clone(),
                    flow.id.clone(),
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn matches_window(&self, application_name: &str, window_title: &str) -> bool {
        let selector_matches = self.selectors.iter().any(|selector| {
            let has_titles = !selector.window_title_contains.is_empty()
                || selector
                    .localized_window_title_contains
                    .values()
                    .any(|aliases| !aliases.is_empty());
            if selector.application_names.is_empty() && !has_titles {
                return false;
            }
            let app_matches = selector.application_names.is_empty()
                || selector
                    .application_names
                    .iter()
                    .any(|name| normalize_text(name) == normalize_text(application_name));
            let normalized_title = normalize_text(window_title);
            let title_matches = !has_titles
                || selector
                    .window_title_contains
                    .iter()
                    .chain(selector.localized_window_title_contains.values().flatten())
                    .any(|value| normalized_title.contains(&normalize_text(value)));
            app_matches && title_matches
        });
        selector_matches
            && (self.application.versions.is_empty()
                || self.application.versions.iter().any(|version| {
                    contains_version_token(application_name, version)
                        || contains_version_token(window_title, version)
                }))
    }

    /// Match a window only when the Host has supplied all identity evidence
    /// requested by the profile.  The profile never invents or stores these
    /// values; they are ephemeral capabilities owned by the Host session.
    #[must_use]
    pub fn matches_bound_window(
        &self,
        application_name: &str,
        window_title: &str,
        process_id: Option<u32>,
        window_handle: Option<u64>,
        window_version: Option<&str>,
    ) -> bool {
        self.matches_window(application_name, window_title)
            && (!self.binding.require_exact_pid || process_id.is_some_and(|value| value != 0))
            && (!self.binding.require_exact_window_handle
                || window_handle.is_some_and(|value| value != 0))
            && (!self.binding.require_window_version_match
                || window_version.is_some_and(|value| !value.trim().is_empty()))
            && (self.application.versions.is_empty()
                || window_version.is_some_and(|version| {
                    self.application
                        .versions
                        .iter()
                        .any(|expected| contains_version_token(version, expected))
                }))
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
    pub fn state_source(&self, id: &str) -> Option<&StateSource> {
        self.state_sources
            .iter()
            .find(|source| source.id.eq_ignore_ascii_case(id))
    }

    #[must_use]
    pub fn resolve_target(&self, surface_id: &str, query: &str) -> Option<&SemanticTarget> {
        let query = normalize_text(query);
        self.surface(surface_id)?.targets.iter().find(|target| {
            target.id.eq_ignore_ascii_case(&query)
                || normalize_text(&target.label).contains(&query)
                || target
                    .names
                    .iter()
                    .chain(target.localized_names.values().flatten())
                    .any(|name| normalize_text(name) == query)
                || target
                    .automation_ids
                    .iter()
                    .any(|automation_id| normalize_text(automation_id) == query)
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

    /// Resolve exactly one current semantic element.  Ambiguous or missing
    /// controls intentionally return `None` so callers cannot fall back to a
    /// coordinate or guessed-label action.
    #[must_use]
    pub fn find_unique_element<'a>(
        &'a self,
        surface_id: &str,
        root: &'a Value,
        query: &str,
    ) -> Option<&'a Value> {
        let matches = self.find_elements(surface_id, root, query);
        (matches.len() == 1).then(|| matches[0])
    }

    #[must_use]
    pub fn supported_locales(&self) -> Vec<&str> {
        let mut locales = BTreeSet::new();
        locales.extend(self.settings.default_locale.as_deref());
        for selector in &self.selectors {
            locales.extend(
                selector
                    .localized_window_title_contains
                    .keys()
                    .map(String::as_str),
            );
        }
        for target in self.surfaces.iter().flat_map(|surface| &surface.targets) {
            locales.extend(target.localized_names.keys().map(String::as_str));
        }
        locales.into_iter().collect()
    }
}

fn validate_identifier(profile_id: &str, value: &str, field: &str) -> Result<(), ProfileError> {
    if value.is_empty()
        || value.len() > 80
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(ProfileError::InvalidIdentifier(
            profile_id.to_owned(),
            field.to_owned(),
            value.to_owned(),
        ));
    }
    Ok(())
}

impl StateSource {
    fn validate(&self, profile_id: &str) -> Result<(), ProfileError> {
        let url = Url::parse(&self.url).map_err(|_| {
            ProfileError::InvalidStateSourceUrl(profile_id.to_owned(), self.id.clone())
        })?;
        let literal_loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1"));
        if self.source_type != StateSourceType::LoopbackHttpJson
            || self.mode != StateSourceMode::ReadOnly
            || url.scheme() != "http"
            || !literal_loopback
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProfileError::InvalidStateSourceUrl(
                profile_id.to_owned(),
                self.id.clone(),
            ));
        }
        if self.expected_schema_version.trim().is_empty()
            || self.expected_schema_version.len() > 64
            || !valid_json_pointer(&self.schema_version_pointer)
            || !valid_json_pointer(&self.tick_pointer)
        {
            return Err(ProfileError::InvalidStateContract(
                profile_id.to_owned(),
                self.id.clone(),
            ));
        }
        if !(50..=10_000).contains(&self.timeout_ms)
            || !(1_024..=8 * 1_024 * 1_024).contains(&self.max_response_bytes)
        {
            return Err(ProfileError::InvalidStateSourceBounds(
                profile_id.to_owned(),
                self.id.clone(),
            ));
        }
        Ok(())
    }
}

fn valid_json_pointer(pointer: &str) -> bool {
    !pointer.is_empty()
        && pointer.len() <= 256
        && pointer.starts_with('/')
        && !pointer.chars().any(char::is_control)
}

fn normalize_text(value: &str) -> String {
    // ponytail: std Unicode lowercase is enough for observed UIs; add NFKC
    // case-folding only when a real alias demonstrates the need.
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn contains_version_token(value: &str, version: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '.')
        .any(|token| token.eq_ignore_ascii_case(version))
}

fn validate_localized_aliases(
    profile_id: &str,
    aliases: &BTreeMap<String, Vec<String>>,
) -> Result<(), ProfileError> {
    for (locale, values) in aliases {
        if !valid_locale_tag(locale) {
            return Err(ProfileError::InvalidLocale(
                profile_id.to_owned(),
                locale.clone(),
            ));
        }
        if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
            return Err(ProfileError::InvalidLocalizedAliases(
                profile_id.to_owned(),
                locale.clone(),
            ));
        }
    }
    Ok(())
}

fn valid_locale_tag(value: &str) -> bool {
    // ponytail: validate the common BCP-47 shape without a locale dependency;
    // use a standards parser if private-use or grandfathered tags are required.
    if value.is_empty() || value.len() > 35 || value.trim() != value {
        return false;
    }
    let mut parts = value.split('-');
    let Some(language) = parts.next() else {
        return false;
    };
    (2..=8).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts.all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
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

pub fn resolve_profile(
    parent: &SemanticProfile,
    child: &SemanticProfile,
) -> Result<SemanticProfile, ProfileError> {
    parent.validate()?;
    child.validate()?;
    let Some(reference) = &child.extends else {
        return Ok(child.clone());
    };
    let requirement = VersionReq::parse(&reference.version)
        .map_err(|_| ProfileError::InvalidParent(child.id.clone()))?;
    let parent_version = Version::parse(&parent.profile_version).map_err(|_| {
        ProfileError::InvalidProfileVersion(parent.id.clone(), parent.profile_version.clone())
    })?;
    if reference.id != parent.id || !requirement.matches(&parent_version) {
        return Err(ProfileError::ParentVersionMismatch(
            child.id.clone(),
            reference.id.clone(),
            reference.version.clone(),
        ));
    }
    if !child
        .application
        .family
        .eq_ignore_ascii_case(&parent.application.family)
    {
        return Err(ProfileError::ParentApplicationMismatch(
            child.id.clone(),
            parent.application.family.clone(),
        ));
    }

    let mut resolved = child.clone();
    if resolved.selectors.is_empty() {
        resolved.selectors.clone_from(&parent.selectors);
    }
    resolved.state_sources = merge_by_id(&parent.state_sources, &child.state_sources, |source| {
        source.id.as_str()
    });
    resolved.surfaces = merge_surfaces(&parent.surfaces, &child.surfaces);
    resolved.validate()?;
    Ok(resolved)
}

fn merge_by_id<T: Clone>(parent: &[T], child: &[T], id: impl Fn(&T) -> &str) -> Vec<T> {
    let mut merged = parent.to_vec();
    for value in child {
        if let Some(index) = merged
            .iter()
            .position(|candidate| id(candidate) == id(value))
        {
            merged[index] = value.clone();
        } else {
            merged.push(value.clone());
        }
    }
    merged
}

fn merge_surfaces(parent: &[SemanticSurface], child: &[SemanticSurface]) -> Vec<SemanticSurface> {
    let mut merged = parent.to_vec();
    for child_surface in child {
        if let Some(index) = merged
            .iter()
            .position(|surface| surface.id == child_surface.id)
        {
            let mut surface = child_surface.clone();
            surface.targets =
                merge_by_id(&merged[index].targets, &child_surface.targets, |target| {
                    target.id.as_str()
                });
            merged[index] = surface;
        } else {
            merged.push(child_surface.clone());
        }
    }
    merged
}

pub fn builtin_profiles() -> &'static [SemanticProfile] {
    static PROFILES: OnceLock<Vec<SemanticProfile>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            let mut profiles = [
                include_str!("../profiles/ue.json"),
                include_str!("../profiles/maya.json"),
                include_str!("../profiles/maya-2024.json"),
                include_str!("../profiles/fab.json"),
                include_str!("../profiles/steam-chromium.json"),
            ]
            .into_iter()
            .map(|profile| parse_profile(profile).expect("built-in semantic profile is valid"))
            .collect::<Vec<_>>();
            let child_index = profiles
                .iter()
                .position(|profile| profile.id == "maya-2024")
                .expect("Maya 2024 profile");
            let parent = profiles
                .iter()
                .find(|profile| profile.id == "maya")
                .expect("Maya base profile")
                .clone();
            profiles[child_index] = resolve_profile(&parent, &profiles[child_index])
                .expect("built-in Maya 2024 inheritance is valid");
            profiles
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

#[cfg(test)]
mod tests;
