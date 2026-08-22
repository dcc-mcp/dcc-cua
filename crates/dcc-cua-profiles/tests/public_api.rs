use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use dcc_cua_profiles::{ContextSelection, ProfileContextRequest, ProfilePlatform, ProfileStore};
use rstest::rstest;
use serde_json::json;
use tempfile::tempdir;

fn write_package(root: &Path) {
    fs::create_dir_all(root.join("knowledge/documents")).expect("knowledge directory");
    fs::write(
        root.join("profile-package.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 2,
            "kind": "dcc-cua-profile-package",
            "id": "office",
            "version": "1.0.0",
            "display_name": "Office",
            "description": "Reusable profile package test",
            "license": "MIT",
            "artifacts": [
                {"type": "semantic_profile", "path": "profile.json"},
                {"type": "context_index", "path": "knowledge/index.json"},
                {"type": "context_document", "path": "knowledge/documents/workbook.json"}
            ],
            "requires": {"dcc_cua": ">=1.0.0"},
            "platforms": ["windows", "macos", "linux"]
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest");
    fs::write(
        root.join("profile.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 3,
            "id": "office",
            "profile_version": "1.0.0",
            "application": {"family": "office", "versions": []},
            "display_name": "Office",
            "selectors": [{"application_names": ["office.exe"]}],
            "surfaces": [{
                "id": "main",
                "label": "Main",
                "role": "document",
                "route": "accessibility",
                "targets": []
            }],
            "settings": {
                "dialog_style": "application_rendered",
                "preferred_route": "accessibility",
                "destructive_confirmation_required": true
            }
        }))
        .expect("profile JSON"),
    )
    .expect("profile");
    fs::write(
        root.join("knowledge/index.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 2,
            "profileId": "office",
            "documents": [{
                "id": "workbook",
                "path": "knowledge/documents/workbook.json",
                "identities": {"document": "sha256:ABC"},
                "selectors": {"kind": "Workbook"}
            }]
        }))
        .expect("context index JSON"),
    )
    .expect("context index");
    fs::write(
        root.join("knowledge/documents/workbook.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 2,
            "profileId": "office",
            "id": "workbook",
            "fences": {"document": "sha256:ABC"},
            "content": {"summary": "rules"}
        }))
        .expect("context document JSON"),
    )
    .expect("context document");
}

fn rewrite_package_identity(root: &Path, id: &str, display_name: &str) {
    let manifest_path = root.join("profile-package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest"))
            .expect("manifest JSON");
    manifest["id"] = json!(id);
    manifest["display_name"] = json!(display_name);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");

    let profile_path = root.join("profile.json");
    let mut profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&profile_path).expect("profile")).expect("profile JSON");
    profile["id"] = json!(id);
    profile["display_name"] = json!(display_name);
    fs::write(
        &profile_path,
        serde_json::to_vec_pretty(&profile).expect("profile JSON"),
    )
    .expect("profile");
}

fn add_cross_profile_fallback(root: &Path, profile_id: &str) {
    let profile_path = root.join("profile.json");
    let mut profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&profile_path).expect("profile")).expect("profile JSON");
    profile["surfaces"][0]["targets"] = json!([{
        "id": "open",
        "label": "Open",
        "role": "button",
        "fallback": {"profile_id": profile_id, "surface_id": "main"}
    }]);
    fs::write(
        &profile_path,
        serde_json::to_vec_pretty(&profile).expect("profile JSON"),
    )
    .expect("profile");
}

fn extend_profile(root: &Path, parent_id: &str) {
    let profile_path = root.join("profile.json");
    let mut profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&profile_path).expect("profile")).expect("profile JSON");
    profile["extends"] = json!({"id": parent_id, "version": ">=1.0.0"});
    fs::write(
        &profile_path,
        serde_json::to_vec_pretty(&profile).expect("profile JSON"),
    )
    .expect("profile");
}

#[rstest]
fn embedder_can_install_resolve_match_and_select_context_without_the_cli() {
    let workspace = tempdir().expect("workspace");
    let source = workspace.path().join("source");
    let store_root = workspace.path().join("store");
    write_package(&source);

    let mut store = ProfileStore::open(&store_root).expect("empty store");
    let installed = store.install(&source, false).expect("install package");
    assert_eq!(installed.manifest.id, "office");

    let profile = store
        .profile("office")
        .expect("profile lookup")
        .expect("installed profile");
    assert_eq!(profile.id, "office");

    let matched = store.catalog().match_window("office.exe", "Quarterly Plan");
    assert_eq!(matched.selected.as_deref(), Some("office"));
    assert!(!matched.ambiguous);

    let context = store
        .context(ProfileContextRequest {
            profile_id: "office".into(),
            identities: BTreeMap::from([("document".into(), "sha256:ABC".into())]),
            selectors: BTreeMap::from([("kind".into(), "Workbook".into())]),
            knowledge_root: None,
        })
        .expect("profile context");
    assert_eq!(context.selection, ContextSelection::Exact);
    assert_eq!(context.documents.len(), 1);
    assert_eq!(context.documents[0].id, "workbook");
}

#[rstest]
fn store_snapshot_reuses_install_validation_until_explicit_refresh() {
    let workspace = tempdir().expect("workspace");
    let source = workspace.path().join("source");
    let store_root = workspace.path().join("store");
    write_package(&source);
    let mut store = ProfileStore::open(&store_root).expect("empty store");
    store.install(&source, false).expect("install package");

    fs::remove_file(store_root.join("office/profile.json")).expect("simulate external mutation");
    assert!(store.profile("office").expect("snapshot lookup").is_some());

    store.refresh().expect("explicit refresh");
    assert!(store.profile("office").is_err());
}

#[rstest]
fn install_rejects_a_dangling_cross_profile_fallback() {
    let workspace = tempdir().expect("workspace");
    let source = workspace.path().join("source");
    let store_root = workspace.path().join("store");
    write_package(&source);
    let profile_path = source.join("profile.json");
    let mut profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&profile_path).expect("profile")).expect("profile JSON");
    profile["surfaces"][0]["targets"] = json!([{
        "id": "open",
        "label": "Open",
        "role": "button",
        "fallback": {"profile_id": "missing-profile", "surface_id": "missing-surface"}
    }]);
    fs::write(
        &profile_path,
        serde_json::to_vec_pretty(&profile).expect("profile JSON"),
    )
    .expect("profile");

    let mut store = ProfileStore::open(&store_root).expect("empty store");
    let error = store
        .install(&source, false)
        .expect_err("dangling fallback must fail closed");
    assert!(error.to_string().contains("fallback"));
}

#[rstest]
fn install_enforces_the_declared_platforms() {
    let workspace = tempdir().expect("workspace");
    let source = workspace.path().join("source");
    let store_root = workspace.path().join("store");
    write_package(&source);
    let unsupported = match ProfilePlatform::current() {
        ProfilePlatform::Windows => "linux",
        ProfilePlatform::Macos | ProfilePlatform::Linux => "windows",
    };
    let manifest_path = source.join("profile-package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest"))
            .expect("manifest JSON");
    manifest["platforms"] = json!([unsupported]);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");

    let mut store = ProfileStore::open(&store_root).expect("empty store");
    assert!(store.install(&source, false).is_err());
}

#[rstest]
fn refresh_invalidates_fallback_dependents_transitively() {
    let workspace = tempdir().expect("workspace");
    let store_root = workspace.path().join("store");
    let office = store_root.join("office");
    let writer = store_root.join("writer");
    write_package(&office);
    write_package(&writer);
    rewrite_package_identity(&writer, "writer", "Writer");
    add_cross_profile_fallback(&office, "writer");
    add_cross_profile_fallback(&writer, "missing-profile");

    let store = ProfileStore::open(&store_root).expect("store snapshot");

    assert!(store.profile("writer").is_err());
    assert!(store.profile("office").is_err());
}

#[rstest]
fn uninstall_rolls_back_when_an_installed_profile_depends_on_the_target() {
    let workspace = tempdir().expect("workspace");
    let office_source = workspace.path().join("office-source");
    let writer_source = workspace.path().join("writer-source");
    let store_root = workspace.path().join("store");
    write_package(&office_source);
    write_package(&writer_source);
    rewrite_package_identity(&writer_source, "writer", "Writer");
    add_cross_profile_fallback(&writer_source, "office");
    let mut store = ProfileStore::open(&store_root).expect("empty store");
    store
        .install(&office_source, false)
        .expect("install office");
    store
        .install(&writer_source, false)
        .expect("install writer");

    let error = store
        .uninstall("office")
        .expect_err("dependent profile must keep uninstall atomic");

    assert!(error.to_string().contains("writer"));
    assert!(store.profile("office").expect("office lookup").is_some());
    assert!(store.profile("writer").expect("writer lookup").is_some());
}

#[rstest]
fn refresh_rejects_a_child_when_its_parent_is_not_ready() {
    let workspace = tempdir().expect("workspace");
    let store_root = workspace.path().join("store");
    let office = store_root.join("office");
    let writer = store_root.join("writer");
    write_package(&office);
    write_package(&writer);
    rewrite_package_identity(&writer, "writer", "Writer");
    extend_profile(&writer, "office");
    let unsupported = match ProfilePlatform::current() {
        ProfilePlatform::Windows => "linux",
        ProfilePlatform::Macos | ProfilePlatform::Linux => "windows",
    };
    let manifest_path = office.join("profile-package.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest"))
            .expect("manifest JSON");
    manifest["platforms"] = json!([unsupported]);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");

    let store = ProfileStore::open(&store_root).expect("store snapshot");

    assert!(store.profile("office").is_err());
    assert!(store.profile("writer").is_err());
}
