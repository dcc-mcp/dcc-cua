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
fn typed_routes_map_to_registered_dcc_tools() {
    assert_eq!(
        SemanticRoute::UnrealTypedApi.typed_tool_name(),
        Some("unreal_remote_call")
    );
    assert_eq!(
        SemanticRoute::MayaTypedApi.typed_tool_name(),
        Some("maya_command")
    );
    for route in [
        SemanticRoute::Accessibility,
        SemanticRoute::BrowserDom,
        SemanticRoute::OsNativeDialog,
        SemanticRoute::VisualFallback,
    ] {
        assert_eq!(route.typed_tool_name(), None);
    }
}

#[rstest]
fn denied_word_exemptions_match_case_insensitively_and_default_empty() {
    for profile in builtin_profiles() {
        assert!(profile.settings.denied_word_exemptions.is_empty());
        assert!(!profile.settings.exempts_denied_word("login"));
    }
    let input = r#"{
        "schema_version": 1,
        "id": "licensed",
        "display_name": "Licensed App",
        "selectors": [{"application_names": ["licensed.exe"]}],
        "settings": {
            "dialog_style": "host_owned",
            "preferred_route": "accessibility",
            "denied_word_exemptions": ["Sign in"]
        }
    }"#;
    let profile = parse_profile(input).expect("exemption profile parses");
    assert!(profile.settings.exempts_denied_word("sign in"));
    assert!(!profile.settings.exempts_denied_word("password"));
}

fn write_profiles_dir(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let directory = std::env::temp_dir()
        .join("dcc-cua-profile-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create profile dir");
    for (file, contents) in files {
        std::fs::write(directory.join(file), contents).expect("write profile");
    }
    directory
}

const EXTERNAL_PROFILE: &str = r#"{
    "schema_version": 1,
    "id": "houdini",
    "display_name": "SideFX Houdini",
    "selectors": [{"application_names": ["houdini.exe"]}],
    "settings": {"dialog_style": "os_native", "preferred_route": "accessibility"}
}"#;

#[rstest]
fn external_profile_directory_loads_valid_profiles_in_order() {
    let directory = write_profiles_dir("valid", &[("houdini.json", EXTERNAL_PROFILE)]);
    let profiles = load_profiles_from_dir(&directory).expect("external profiles load");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].id, "houdini");
    assert!(profiles[0].matches_window("houdini.exe", "untitled.hip"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[rstest]
fn external_profile_directory_fails_closed_on_invalid_or_shadowing_profiles() {
    let invalid = write_profiles_dir("invalid", &[("broken.json", "{not json")]);
    assert!(matches!(
        load_profiles_from_dir(&invalid),
        Err(ProfileError::InvalidJson(_))
    ));
    let _ = std::fs::remove_dir_all(&invalid);

    let shadowing = write_profiles_dir(
        "shadow",
        &[(
            "maya.json",
            r#"{
                "schema_version": 1,
                "id": "maya",
                "display_name": "Shadow Maya",
                "selectors": [{"application_names": ["maya.exe"]}],
                "settings": {"dialog_style": "os_native", "preferred_route": "accessibility"}
            }"#,
        )],
    );
    assert_eq!(
        load_profiles_from_dir(&shadowing),
        Err(ProfileError::DuplicateProfile("maya".into()))
    );
    let _ = std::fs::remove_dir_all(&shadowing);
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
