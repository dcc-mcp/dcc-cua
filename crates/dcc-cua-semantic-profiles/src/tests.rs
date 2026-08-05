use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
fn builtins_are_valid_and_have_independent_ids() {
    let profiles = builtin_profiles();
    assert_eq!(profiles.len(), 3);
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        ["ue", "maya", "fab"]
    );
}

#[rstest]
fn unreal_profile_prefers_typed_semantics_and_matches_editor_windows() {
    let profile = builtin_profile("unreal-engine").expect("Unreal profile");
    assert_eq!(
        profile.settings.preferred_route,
        SemanticRoute::UnrealTypedApi
    );
    assert!(profile.matches_window("UnrealEditor.exe", "PCG Fab Showcase"));
    assert!(profile.matches_window("UE4Editor.exe", "UE426LookdevTest - 虚幻编辑器"));
    assert_eq!(
        profile
            .surface("content_browser")
            .expect("content browser")
            .route,
        SemanticRoute::UnrealTypedApi
    );
    assert!(
        profile
            .resolve_target("content_browser", "search")
            .is_some()
    );
}

#[rstest]
fn maya_profile_uses_os_native_dialogs() {
    let profile = builtin_profile("maya").expect("Maya profile");
    assert_eq!(profile.settings.dialog_style, DialogStyle::OsNative);
    assert!(profile.resolve_target("home", "新建").is_some());
    assert_eq!(
        profile.surface("dialog").expect("dialog").route,
        SemanticRoute::OsNativeDialog
    );
    assert!(profile.matches_window("maya.exe", "Autodesk Maya - scene.ma"));
    let target = profile
        .resolve_target("dialog", "file_dialog")
        .expect("Maya file dialog target");
    assert!(target.supports_action("accept"));
    let root = json!({
        "elements": [
            {"role": "dialog", "name": "Open"},
            {"role": "Button", "name": "Cancel"}
        ]
    });
    let matches = profile.find_elements("dialog", &root, "file_dialog");
    assert_eq!(matches.len(), 1);
    let home_root = json!({
        "elements": [
            {"role": "Button", "name": "新建"},
            {"role": "Button", "name": "打开"}
        ]
    });
    let home_matches = profile.find_elements("home", &home_root, "new_scene");
    assert_eq!(home_matches.len(), 1);
    assert_eq!(home_matches[0]["name"], "新建");
}

#[rstest]
fn fab_profile_matches_browser_urls_without_matching_unrelated_hosts() {
    let profile = builtin_profile("fab").expect("Fab profile");
    assert!(profile.matches_url("https://www.fab.com/listings/asset"));
    assert!(profile.matches_url("https://store.epicgames.com/en-US/p/fab"));
    assert!(profile.matches_window("EpicGamesLauncher.exe", "Epic Games Launcher"));
    assert!(!profile.matches_url("https://example.com/fab"));
    assert!(profile.settings.destructive_confirmation_required);
}

#[rstest]
fn invalid_profile_rejects_duplicate_targets() {
    let input = r#"{
        "schema_version": 1,
        "id": "test",
        "display_name": "Test",
        "selectors": [{"application_names": ["test.exe"]}],
        "surfaces": [{
            "id": "surface",
            "label": "Surface",
            "role": "panel",
            "route": "accessibility",
            "targets": [
                {"id": "same", "label": "One", "role": "button"},
                {"id": "same", "label": "Two", "role": "button"}
            ]
        }],
        "settings": {"dialog_style": "host_owned", "preferred_route": "accessibility"}
    }"#;
    assert!(matches!(
        parse_profile(input),
        Err(ProfileError::DuplicateTarget(..))
    ));
}
