use std::fs;
use std::path::{Path, PathBuf};

use rstest::rstest;
use serde_json::{Value, json};

const PACKAGE_SCHEMA: &str = include_str!("../../../docs/schemas/profile-package-v2.schema.json");
const PROFILE_SCHEMA: &str = include_str!("../../../docs/schemas/semantic-profile-v3.schema.json");

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn collect_named_files(root: &Path, name: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, name, files);
        } else if path.file_name().is_some_and(|candidate| candidate == name) {
            files.push(path);
        }
    }
}

#[rstest]
fn published_profile_schemas_accept_every_repository_example() {
    let package_schema: Value = serde_json::from_str(PACKAGE_SCHEMA).expect("package schema");
    let package_validator = jsonschema::validator_for(&package_schema).expect("package validator");
    let profile_schema: Value = serde_json::from_str(PROFILE_SCHEMA).expect("profile schema");
    let profile_validator = jsonschema::validator_for(&profile_schema).expect("profile validator");

    let root = workspace_root();
    let mut manifests = Vec::new();
    collect_named_files(
        &root.join("examples/profiles"),
        "profile-package.json",
        &mut manifests,
    );
    assert!(!manifests.is_empty(), "repository package examples");
    for path in manifests {
        let value: Value =
            serde_json::from_slice(&fs::read(&path).expect("manifest")).expect("manifest JSON");
        assert!(
            package_validator.is_valid(&value),
            "{} failed package schema: {:?}",
            path.display(),
            package_validator.iter_errors(&value).collect::<Vec<_>>()
        );
    }

    let mut profiles = Vec::new();
    collect_named_files(
        &root.join("examples/profiles"),
        "profile.json",
        &mut profiles,
    );
    collect_named_files(
        &root.join("crates/dcc-cua-semantic-profiles/profiles"),
        "ue.json",
        &mut profiles,
    );
    for name in ["maya.json", "maya-2024.json", "fab.json"] {
        profiles.push(
            root.join("crates/dcc-cua-semantic-profiles/profiles")
                .join(name),
        );
    }
    assert!(!profiles.is_empty(), "repository Profile examples");
    for path in profiles {
        let value: Value =
            serde_json::from_slice(&fs::read(&path).expect("profile")).expect("profile JSON");
        assert!(
            profile_validator.is_valid(&value),
            "{} failed Profile schema: {:?}",
            path.display(),
            profile_validator.iter_errors(&value).collect::<Vec<_>>()
        );
    }
}

#[rstest]
fn package_schema_rejects_values_that_the_runtime_rejects() {
    let schema: Value = serde_json::from_str(PACKAGE_SCHEMA).expect("package schema");
    let validator = jsonschema::validator_for(&schema).expect("package validator");
    let base = json!({
        "schema_version": 2,
        "kind": "dcc-cua-profile-package",
        "id": "valid-profile",
        "version": "1.0.0",
        "display_name": "Valid",
        "description": "Valid package",
        "license": "MIT",
        "artifacts": [{"type": "semantic_profile", "path": "profile.json"}],
        "requires": {"dcc_cua": ">=1.0.0"},
        "platforms": ["windows"]
    });
    assert!(validator.is_valid(&base));

    for (pointer, invalid) in [
        ("/id", json!("Ambiguous-ID")),
        ("/version", json!("not-semver")),
        ("/display_name", json!("   ")),
        ("/requires/dcc_cua", json!("^1.0")),
        ("/platforms", json!(["plan9"])),
        ("/artifacts/0/path", json!("../profile.json")),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).expect("test pointer") = invalid;
        assert!(
            !validator.is_valid(&value),
            "{pointer} accepted invalid data"
        );
    }

    let mut duplicate_semantic_profile = base;
    duplicate_semantic_profile["artifacts"] = json!([
        {"type": "semantic_profile", "path": "profile.json"},
        {"type": "semantic_profile", "path": "other-profile.json"}
    ]);
    assert!(
        !validator.is_valid(&duplicate_semantic_profile),
        "schema accepted multiple semantic_profile artifacts"
    );
}
