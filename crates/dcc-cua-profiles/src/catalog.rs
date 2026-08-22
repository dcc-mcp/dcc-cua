use std::collections::BTreeMap;

use dcc_cua_semantic_profiles::{SemanticProfile, builtin_profiles};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ProfileStore;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    Builtin,
    User,
}

#[derive(Clone, Debug)]
struct CatalogProfile {
    source: ProfileSource,
    profile: SemanticProfile,
}

#[derive(Clone, Debug)]
pub struct ProfileCatalog {
    profiles: Vec<CatalogProfile>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ProfileCatalogError {
    #[error(
        "profile {profile_id:?} target {target_id:?} has dangling fallback to {fallback_profile_id:?}/{fallback_surface_id:?}"
    )]
    DanglingFallback {
        profile_id: String,
        target_id: String,
        fallback_profile_id: String,
        fallback_surface_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMatchCandidate {
    pub id: String,
    pub profile_version: String,
    pub application: dcc_cua_semantic_profiles::ProfileApplication,
    pub extends: Option<dcc_cua_semantic_profiles::ProfileReference>,
    pub source: ProfileSource,
    pub version_specific: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMatch {
    pub application_name: String,
    pub window_title: String,
    pub selected: Option<String>,
    pub ambiguous: bool,
    pub candidates: Vec<ProfileMatchCandidate>,
}

impl ProfileCatalog {
    pub(crate) fn from_store(store: &ProfileStore) -> Self {
        let mut profiles = builtin_profiles()
            .iter()
            .cloned()
            .map(|profile| {
                (
                    profile.id.clone(),
                    CatalogProfile {
                        source: ProfileSource::Builtin,
                        profile,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for profile in store.ready_profiles() {
            profiles.insert(
                profile.id.clone(),
                CatalogProfile {
                    source: ProfileSource::User,
                    profile: profile.clone(),
                },
            );
        }
        Self {
            profiles: profiles.into_values().collect(),
        }
    }

    #[must_use]
    pub fn profiles(&self) -> Vec<(&SemanticProfile, ProfileSource)> {
        self.profiles
            .iter()
            .map(|entry| (&entry.profile, entry.source))
            .collect()
    }

    #[must_use]
    pub fn profile(&self, id: &str) -> Option<(&SemanticProfile, ProfileSource)> {
        self.profiles
            .iter()
            .find(|entry| entry.profile.id == id)
            .map(|entry| (&entry.profile, entry.source))
    }

    pub fn validate(&self) -> Result<(), ProfileCatalogError> {
        for entry in &self.profiles {
            self.validate_profile(&entry.profile)?;
        }
        Ok(())
    }

    pub fn validate_profile(&self, profile: &SemanticProfile) -> Result<(), ProfileCatalogError> {
        for target in profile.surfaces.iter().flat_map(|surface| &surface.targets) {
            let Some(fallback) = &target.fallback else {
                continue;
            };
            let valid = self
                .profile(&fallback.profile_id)
                .is_some_and(|(fallback_profile, _)| {
                    fallback_profile.surface(&fallback.surface_id).is_some()
                });
            if !valid {
                return Err(ProfileCatalogError::DanglingFallback {
                    profile_id: profile.id.clone(),
                    target_id: target.id.clone(),
                    fallback_profile_id: fallback.profile_id.clone(),
                    fallback_surface_id: fallback.surface_id.clone(),
                });
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn match_window(&self, application_name: &str, window_title: &str) -> ProfileMatch {
        let mut candidates = self
            .profiles
            .iter()
            .filter(|entry| entry.profile.matches_window(application_name, window_title))
            .map(|entry| {
                let version_specific = !entry.profile.application.versions.is_empty();
                (entry, version_specific)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|(left, left_specific), (right, right_specific)| {
            right_specific
                .cmp(left_specific)
                .then_with(|| {
                    left.profile
                        .application
                        .versions
                        .len()
                        .cmp(&right.profile.application.versions.len())
                })
                .then_with(|| source_rank(right.source).cmp(&source_rank(left.source)))
                .then_with(|| left.profile.id.cmp(&right.profile.id))
        });

        let top_specificity = candidates
            .first()
            .map(|(entry, specific)| (*specific, entry.profile.application.versions.len()));
        let top_count = top_specificity.map_or(0, |specificity| {
            candidates
                .iter()
                .take_while(|(entry, specific)| {
                    (*specific, entry.profile.application.versions.len()) == specificity
                })
                .count()
        });
        let selected = (top_count == 1).then(|| candidates[0].0.profile.id.clone());
        ProfileMatch {
            application_name: application_name.into(),
            window_title: window_title.into(),
            selected,
            ambiguous: top_count > 1,
            candidates: candidates
                .into_iter()
                .map(|(entry, version_specific)| ProfileMatchCandidate {
                    id: entry.profile.id.clone(),
                    profile_version: entry.profile.profile_version.clone(),
                    application: entry.profile.application.clone(),
                    extends: entry.profile.extends.clone(),
                    source: entry.source,
                    version_specific,
                })
                .collect(),
        }
    }
}

const fn source_rank(source: ProfileSource) -> u8 {
    match source {
        ProfileSource::Builtin => 0,
        ProfileSource::User => 1,
    }
}
