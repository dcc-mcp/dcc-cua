use std::collections::BTreeMap;
use std::fs;

use rstest::rstest;
use serde_json::json;
use tempfile::tempdir;

use super::{parse_pairs, select_documents};

fn write_context(root: &std::path::Path, document_id: &str, identity: &str, selector: &str) {
    fs::create_dir_all(root.join("documents")).unwrap();
    fs::write(
        root.join("index.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 2, "profileId": "office", "documents": [{
                "id": document_id, "path": format!("documents/{document_id}.json"),
                "identities": {"document": identity}, "selectors": {"kind": selector}
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(format!("documents/{document_id}.json")),
        serde_json::to_vec(&json!({
            "schemaVersion": 2, "profileId": "office", "id": document_id,
            "fences": {"document": identity}, "content": {"summary": "rules"}
        }))
        .unwrap(),
    )
    .unwrap();
}

#[rstest]
fn exact_case_sensitive_identity_and_selector_load_all_matching_documents() {
    let root = tempdir().unwrap();
    write_context(root.path(), "workbook-rules", "sha256:ABC", "Workbook");
    let selected = select_documents(
        "office",
        &BTreeMap::from([("document".into(), "sha256:ABC".into())]),
        &BTreeMap::from([("kind".into(), "Workbook".into())]),
        &[(root.path().join("index.json"), root.path().into(), "user")],
    )
    .unwrap();
    assert_eq!(selected.len(), 1);

    let wrong_case = select_documents(
        "office",
        &BTreeMap::from([("document".into(), "sha256:abc".into())]),
        &BTreeMap::from([("kind".into(), "workbook".into())]),
        &[(root.path().join("index.json"), root.path().into(), "user")],
    )
    .unwrap();
    assert!(wrong_case.is_empty());
}

#[rstest]
fn fence_mismatch_fails_closed() {
    let root = tempdir().unwrap();
    write_context(root.path(), "rules", "v1", "deck");
    let path = root.path().join("documents/rules.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    document["fences"]["document"] = json!("v2");
    fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
    let error = select_documents(
        "office",
        &BTreeMap::from([("document".into(), "v1".into())]),
        &BTreeMap::from([("kind".into(), "deck".into())]),
        &[(root.path().join("index.json"), root.path().into(), "user")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("fences"));
}

#[rstest]
fn duplicate_document_identity_conflicts_fail_closed() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    write_context(first.path(), "rules", "v1", "deck");
    write_context(second.path(), "rules", "v1", "deck");
    let error = select_documents(
        "office",
        &BTreeMap::from([("document".into(), "v1".into())]),
        &BTreeMap::from([("kind".into(), "deck".into())]),
        &[
            (first.path().join("index.json"), first.path().into(), "user"),
            (
                second.path().join("index.json"),
                second.path().into(),
                "seed",
            ),
        ],
    )
    .unwrap_err();
    assert!(error.to_string().contains("conflicting"));
}

#[rstest]
fn duplicate_pair_is_rejected() {
    let error =
        parse_pairs(&["app=excel".into(), "app=powerpoint".into()], "identity").unwrap_err();
    assert!(error.to_string().contains("duplicate"));
}
