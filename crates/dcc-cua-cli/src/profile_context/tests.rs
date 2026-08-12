use std::fs;

use serde_json::json;
use tempfile::tempdir;

use super::{select_playbook, validate_owned_document};

#[test]
fn exact_catalog_and_hero_select_user_playbook() {
    let package = tempdir().unwrap();
    let user = tempdir().unwrap();
    fs::create_dir_all(user.path().join("playbooks")).unwrap();
    fs::write(
        user.path().join("playbooks/index.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 1, "profileId": "the-bazaar", "entries": [{
                "seasonId": "current", "hero": "Pygmalien", "path": "playbooks/current.json",
                "catalogContentIds": ["sha256:abc"]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        user.path().join("playbooks/current.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 1, "profileId": "the-bazaar", "seasonId": "current",
            "hero": "Pygmalien", "catalogFence": {"contentId": "sha256:abc"}
        }))
        .unwrap(),
    )
    .unwrap();
    let result = select_playbook(
        "the-bazaar",
        "sha256:abc",
        Some("pygmalien"),
        None,
        &user.path().join("playbooks/index.json"),
        &package.path().join("knowledge/playbooks/index.seed.json"),
        package.path(),
        user.path(),
    )
    .unwrap();
    assert_eq!(result.kind, "fresh_exact");
    assert!(result.playbook.is_some());
}

#[test]
fn catalog_mismatch_fails_closed() {
    let package = tempdir().unwrap();
    let user = tempdir().unwrap();
    let result = select_playbook(
        "the-bazaar",
        "sha256:new",
        Some("Pygmalien"),
        None,
        &user.path().join("playbooks/index.json"),
        &package.path().join("knowledge/playbooks/index.seed.json"),
        package.path(),
        user.path(),
    )
    .unwrap();
    assert_eq!(result.kind, "none");
    assert!(result.playbook.is_none());
}

#[test]
fn owned_document_rejects_wrong_profile() {
    let error = validate_owned_document(
        &json!({"schemaVersion": 1, "profileId": "other"}),
        "the-bazaar",
        "test",
    )
    .unwrap_err();
    assert!(error.to_string().contains("profileId"));
}
