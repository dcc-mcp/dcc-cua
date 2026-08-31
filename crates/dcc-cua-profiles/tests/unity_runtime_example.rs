use std::fs;
use std::path::PathBuf;

use dcc_cua_profiles::{ProfilePackageArtifactType, validate_package};
use dcc_cua_semantic_profiles::{StateSourceMode, StateSourceType};
use rstest::rstest;
use serde_json::Value;

fn example_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/profiles/unity-runtime")
}

#[rstest]
fn unity_runtime_example_is_an_installable_read_only_profile_package() {
    let package = validate_package(&example_root()).expect("Unity runtime example package");

    assert_eq!(package.manifest.id, "unity-runtime");
    assert_eq!(package.profile.id, "unity-runtime");
    assert!(
        package
            .profile
            .matches_window("SampleUnityPlayer.exe", "DCC CUA Unity Runtime Sample")
    );

    let source = package
        .profile
        .state_source("unity-ui")
        .expect("optional Unity UI state source");
    assert_eq!(source.source_type, StateSourceType::LoopbackHttpJson);
    assert_eq!(source.mode, StateSourceMode::ReadOnly);
    assert_eq!(source.url, "http://127.0.0.1:47910/v1/ui");
    assert_eq!(source.expected_schema_version, "1.0.0");
    assert_eq!(source.schema_version_pointer, "/schemaVersion");
    assert_eq!(source.tick_pointer, "/tickId");
    assert!(source.use_etag);
    assert!(source.optional);

    assert!(package.manifest.artifacts.iter().any(|artifact| {
        artifact.artifact_type == ProfilePackageArtifactType::CompanionSource
            && artifact.path == "companion"
    }));
    assert!(package.manifest.artifacts.iter().any(|artifact| {
        artifact.artifact_type == ProfilePackageArtifactType::Fixtures
            && artifact.path == "fixtures"
    }));
}

#[rstest]
fn unity_runtime_fixture_matches_the_published_state_contract() {
    let root = example_root();
    let schema: Value = serde_json::from_slice(
        &fs::read(root.join("fixtures/ui-state-v1.schema.json")).expect("state schema"),
    )
    .expect("state schema JSON");
    let fixture: Value = serde_json::from_slice(
        &fs::read(root.join("fixtures/ui-state-v1.example.json")).expect("state fixture"),
    )
    .expect("state fixture JSON");
    let validator = jsonschema::validator_for(&schema).expect("state schema validator");

    assert!(
        validator.is_valid(&fixture),
        "Unity state fixture failed schema: {:?}",
        validator.iter_errors(&fixture).collect::<Vec<_>>()
    );
    assert_eq!(fixture["coordinateSpace"]["origin"], "top_left");
    assert_eq!(fixture["coordinateSpace"]["units"], "unity_render_pixels");
    assert!(fixture["widgets"][0]["label"].is_string());
    assert!(fixture["widgets"][0]["rect"]["width"].is_number());
    assert!(fixture["widgets"][0]["rect"]["height"].is_number());
}

#[rstest]
fn unity_runtime_companion_is_opt_in_loopback_and_read_only() {
    let companion = example_root().join("companion");
    let source = fs::read_dir(&companion)
        .expect("Unity companion directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "cs")
        })
        .map(|entry| fs::read_to_string(entry.path()).expect("Unity companion source"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(source.contains("private bool enableStateServer = false;"));
    assert!(source.contains("new TcpListener(IPAddress.Loopback, port)"));
    assert!(source.contains("private const string StatePath = \"/v1/ui\";"));
    assert!(!source.contains("\"POST\""));
    assert!(!source.contains("/action"));
}
