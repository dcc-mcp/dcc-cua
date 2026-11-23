use rstest::rstest;

use super::*;

#[rstest]
fn profile_key_binding_becomes_a_fenced_foreground_action() {
    let action = key_binding_action(&["SPACE".into()], "observation-1");
    assert_eq!(action.action, "keypress");
    assert_eq!(action.keys, ["SPACE"]);
    assert_eq!(action.delivery_mode.as_deref(), Some("foreground"));
    assert_eq!(action.observation_id.as_deref(), Some("observation-1"));
}

#[rstest]
fn target_matching_requires_exact_binding_evidence_for_versioned_dcc_profiles() {
    let profile =
        dcc_cua_semantic_profiles::builtin_profile("maya-2024").expect("Maya 2024 profile");
    let exact = json!({
        "app_name": "maya.exe",
        "title": "Autodesk Maya 2024: scene.ma",
        "process_id": 42,
        "window_handle": 77,
        "version": "2024"
    });
    assert!(target_matches_profile(profile, Some(&exact)));
    assert!(!target_matches_profile(
        profile,
        Some(&json!({
            "app_name": "maya.exe",
            "title": "Autodesk Maya 2024: scene.ma",
            "process_id": 42,
            "window_handle": 77
        }))
    ));

    for mut weakened in [exact.clone(), exact.clone(), exact.clone(), exact.clone()] {
        weakened.as_object_mut().unwrap().remove("process_id");
        assert!(!target_matches_profile(profile, Some(&weakened)));
        weakened = exact.clone();
        weakened.as_object_mut().unwrap().remove("window_handle");
        assert!(!target_matches_profile(profile, Some(&weakened)));
        weakened = exact.clone();
        weakened.as_object_mut().unwrap().remove("version");
        assert!(!target_matches_profile(profile, Some(&weakened)));
        weakened = exact.clone();
        weakened["version"] = json!("2025");
        assert!(!target_matches_profile(profile, Some(&weakened)));
    }
}
