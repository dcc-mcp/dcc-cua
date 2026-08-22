//! Reusable profile package, catalog, and knowledge-context domain services.
//!
//! The command-line binary is one adapter over these services. Embedders use
//! the same typed API and therefore do not need to spawn or parse the CLI.

#![forbid(unsafe_code)]

mod catalog;
mod context;
mod package;

pub use catalog::{
    ProfileCatalog, ProfileCatalogError, ProfileMatch, ProfileMatchCandidate, ProfileSource,
};
pub use context::{
    ContextDocument, ContextProvenance, ContextSelection, ProfileContext, ProfileContextRequest,
};
pub use package::{
    ProfilePackageArtifact, ProfilePackageArtifactType, ProfilePackageError,
    ProfilePackageManifest, ProfilePackageRequirements, ProfilePlatform, ProfileStore,
    ProfileStoreEntry, ValidatedProfilePackage, default_profile_store, validate_package,
};
