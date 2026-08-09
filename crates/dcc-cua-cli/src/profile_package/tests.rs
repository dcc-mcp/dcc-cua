use rstest::rstest;

use std::fs;

use tempfile::TempDir;

use super::*;

fn write_package(root: &Path, id: &str, version: &str) {
    fs::create_dir_all(root.join("fixtures")).expect("fixture directory");
    fs::write(
        root.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "kind": "dcc-cua-profile-package",
            "id": id,
            "version": version,
            "display_name": "Test Profile",
            "description": "Deterministic test profile",
            "license": "MIT",
            "entry": "profile.json",
            "contents": ["profile.json", "SKILL.md", "fixtures"],
            "capabilities": ["semantic_profile", "agent_skill"],
            "platforms": ["windows", "macos", "linux"]
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest");
    fs::write(
        root.join("profile.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 3,
            "id": id,
            "profile_version": version,
            "application": {"family": "test-application", "versions": []},
            "display_name": "Test Profile",
            "selectors": [{"application_names": ["test-app"]}],
            "surfaces": [{
                "id": "main",
                "label": "Main",
                "role": "document",
                "route": "accessibility",
                "targets": [{
                    "id": "open",
                    "label": "Open",
                    "role": "button",
                    "supported_actions": ["invoke"]
                }]
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
    fs::write(root.join("SKILL.md"), "# Test profile\n").expect("skill");
    fs::write(root.join("fixtures").join("state.json"), "{}\n").expect("fixture");
}

#[rstest]
fn validates_installs_resolves_and_replaces_a_package_atomically() {
    let workspace = TempDir::new().expect("workspace");
    let package = workspace.path().join("package");
    let store = workspace.path().join("store");
    write_package(&package, "test-profile", "1.0.0");
    fs::create_dir_all(package.join("target")).expect("build cache directory");
    fs::write(package.join("target").join("build-cache.bin"), [0_u8; 32]).expect("build cache");

    let validated = validate_package(&package).expect("valid package");
    assert_eq!(validated.file_count, 4);
    assert_eq!(validated.profile.id, "test-profile");

    let installed = install_package(&package, Some(&store), false).expect("install");
    assert_eq!(installed.manifest.version, "1.0.0");
    assert!(!store.join("test-profile").join("target").exists());
    assert!(
        load_installed_profile("test-profile", Some(&store))
            .expect("load")
            .is_some()
    );
    assert!(matches!(
        install_package(&package, Some(&store), false),
        Err(ProfilePackageError::AlreadyInstalled(_))
    ));

    write_package(&package, "test-profile", "1.1.0");
    let replaced = install_package(&package, Some(&store), true).expect("replace");
    assert_eq!(replaced.manifest.version, "1.1.0");
    assert!(!store.join(".test-profile.backup").exists());
}

#[rstest]
fn rejects_traversal_and_identity_mismatch() {
    let workspace = TempDir::new().expect("workspace");
    let package = workspace.path().join("package");
    write_package(&package, "test-profile", "1.0.0");
    let manifest_path = package.join(MANIFEST_FILE);
    let mut manifest =
        serde_json::from_slice::<Value>(&fs::read(&manifest_path).expect("read manifest"))
            .expect("manifest JSON");
    manifest["contents"] = json!(["profile.json", "SKILL.md", "../escape"]);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");
    assert!(matches!(
        validate_package(&package),
        Err(ProfilePackageError::Invalid(_))
    ));
}

#[rstest]
fn installed_child_resolves_a_versioned_parent_without_copying_common_surfaces() {
    let workspace = TempDir::new().expect("workspace");
    let parent = workspace.path().join("parent");
    let child = workspace.path().join("child");
    let store = workspace.path().join("store");
    write_package(&parent, "test-common", "1.2.0");
    install_package(&parent, Some(&store), false).expect("install parent");

    write_package(&child, "test-2024", "1.0.0");
    let profile_path = child.join("profile.json");
    let mut profile =
        serde_json::from_slice::<Value>(&fs::read(&profile_path).unwrap()).expect("profile JSON");
    profile["extends"] = json!({"id": "test-common", "version": "^1.0"});
    profile["application"]["versions"] = json!(["2024"]);
    profile["selectors"] = json!([]);
    profile["surfaces"] = json!([]);
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    install_package(&child, Some(&store), false).expect("install child");
    let resolved = load_installed_profile("test-2024", Some(&store))
        .expect("load child")
        .expect("installed child");
    assert_eq!(resolved.application.versions, ["2024"]);
    assert_eq!(resolved.selectors[0].application_names, ["test-app"]);
    assert!(resolved.surface("main").is_some());
    assert_eq!(
        resolved.extends.as_ref().map(|parent| parent.id.as_str()),
        Some("test-common")
    );
}
