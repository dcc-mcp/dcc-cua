use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
fn builtins_are_valid_and_have_independent_ids() {
    let profiles = builtin_profiles();
    assert_eq!(profiles.len(), 5);
    assert_eq!(
        profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        ["ue", "maya", "maya-2024", "fab", "steam-chromium"]
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
    assert!(profile.matches_window("UnrealEditor.exe", "Showcase - 虛幻編輯器"));
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
    assert_eq!(profile.settings.default_locale.as_deref(), Some("en"));
    assert!(profile.resolve_target("home", "新建").is_some());
    assert!(profile.resolve_target("home", "新規シーン").is_some());
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
    let german_root = json!({
        "elements": [{"role": "Button", "name": "ÖFFNEN"}]
    });
    assert_eq!(
        profile
            .find_elements("home", &german_root, "open_scene")
            .len(),
        1
    );
    assert!(profile.supported_locales().contains(&"ja-JP"));
}

#[rstest]
fn maya_2024_inherits_common_surfaces_and_matches_only_its_host_version() {
    let profile = builtin_profile("maya-2024").expect("Maya 2024 profile");
    assert_eq!(profile.profile_version, "1.0.0");
    assert_eq!(profile.application.family, "autodesk-maya");
    assert_eq!(profile.application.versions, ["2024"]);
    assert_eq!(
        profile.extends.as_ref().map(|parent| parent.id.as_str()),
        Some("maya")
    );
    assert_eq!(
        profile.selectors,
        builtin_profile("maya").unwrap().selectors
    );
    assert!(profile.surface("home").is_some());
    assert!(profile.resolve_target("dialog", "file_dialog").is_some());
    assert!(profile.matches_window("maya.exe", "Autodesk Maya 2024: scene.ma"));
    assert!(!profile.matches_window("maya.exe", "Autodesk Maya 2025: scene.ma"));
}

#[rstest]
fn unresolved_child_may_omit_selectors_but_resolution_cannot() {
    let parent = builtin_profile("maya").expect("Maya parent");
    let mut child = builtin_profile("maya-2024").expect("Maya child").clone();
    child.selectors.clear();
    assert!(child.validate().is_ok());
    assert!(
        !resolve_profile(parent, &child)
            .expect("resolved child")
            .selectors
            .is_empty()
    );

    let mut unresolved_parent = parent.clone();
    unresolved_parent.selectors.clear();
    assert!(matches!(
        unresolved_parent.validate(),
        Err(ProfileError::MissingSelector(..))
    ));
}

#[rstest]
fn inheritance_rejects_an_incompatible_parent_version() {
    let parent = builtin_profile("maya").expect("Maya parent");
    let mut child = builtin_profile("maya-2024").expect("Maya child").clone();
    child.extends.as_mut().unwrap().version = ">=2.0".into();
    assert!(matches!(
        resolve_profile(parent, &child),
        Err(ProfileError::ParentVersionMismatch(..))
    ));
}

#[rstest]
fn fab_profile_matches_browser_urls_without_matching_unrelated_hosts() {
    let profile = builtin_profile("fab").expect("Fab profile");
    assert!(profile.matches_url("https://www.fab.com/listings/asset"));
    assert!(profile.matches_url("https://store.epicgames.com/en-US/p/fab"));
    assert!(profile.matches_window("EpicGamesLauncher.exe", "Epic Games Launcher"));
    assert!(profile.matches_window("EpicGamesLauncher.exe", "Epic Games 启动器"));
    assert!(!profile.matches_window("notepad.exe", "Untitled - Notepad"));
    assert!(!profile.matches_url("https://example.com/fab"));
    assert!(profile.settings.destructive_confirmation_required);
    let launcher = profile
        .surface("launcher_download")
        .expect("Launcher fallback surface");
    assert_eq!(launcher.route, SemanticRoute::VisualFallback);
    assert!(
        profile
            .resolve_target("launcher_download", "human_verification")
            .expect("human verification target")
            .supports_action("request_confirmation")
    );
    let ue_download = builtin_profile("ue")
        .expect("UE profile")
        .resolve_target("fab", "download")
        .expect("UE Fab download target");
    assert_eq!(
        ue_download
            .fallback
            .as_ref()
            .map(|fallback| (fallback.profile_id.as_str(), fallback.surface_id.as_str())),
        Some(("fab", "launcher_download"))
    );
}

#[rstest]
fn steam_profile_requires_exact_binding_and_prefers_controlled_browser_bridges() {
    let profile = builtin_profile("steam-chromium").expect("Steam profile");
    assert!(profile.binding.require_exact_pid);
    assert!(profile.binding.require_exact_window_handle);
    assert!(profile.binding.require_window_version_match);
    assert!(profile.binding.fail_closed_on_ambiguity);
    assert!(profile.matches_window("steam.exe", "Steam - Library"));
    assert!(profile.matches_window("steamwebhelper.exe", "Steam Store"));
    assert!(profile.matches_bound_window(
        "steam.exe",
        "Steam Store",
        Some(4242),
        Some(0x1234),
        Some("Steam 1.0")
    ));
    assert!(!profile.matches_bound_window(
        "steam.exe",
        "Steam Store",
        None,
        Some(0x1234),
        Some("Steam 1.0")
    ));
    assert!(profile.matches_url("https://store.steampowered.com/app/123"));
    assert!(!profile.matches_url("https://example.com/store.steampowered.com"));
    assert_eq!(profile.capability_probes.len(), 2);
    assert_eq!(
        profile.capability_probes[0].route,
        SemanticRoute::BrowserDom
    );
    assert!(profile.capability_probes[1].optional);
    let flow = profile
        .flows
        .iter()
        .find(|flow| flow.id == "install")
        .unwrap();
    assert!(flow.requires_fresh_snapshot);
    assert!(flow.requires_post_action_verification);
    assert!(flow.prohibit_coordinates);
    assert!(flow.prohibit_keyboard_shortcuts);
}

#[rstest]
fn steam_install_resolution_fails_closed_when_dom_is_missing_or_ambiguous() {
    let profile = builtin_profile("steam-chromium").expect("Steam profile");
    let missing = json!({"elements": [{"role": "status", "name": "Installed"}]});
    assert!(
        profile
            .find_unique_element("store", &missing, "install_button")
            .is_none()
    );
    let ambiguous = json!({"elements": [
        {"role": "button", "name": "Install", "automation_id": "steam-install-button"},
        {"role": "button", "name": "Install", "automation_id": "steam-install-button"}
    ]});
    assert!(
        profile
            .find_unique_element("store", &ambiguous, "install_button")
            .is_none()
    );
    let unique = json!({"elements": [
        {"role": "button", "name": "安装", "automation_id": "steam-install-button"}
    ]});
    assert!(
        profile
            .find_unique_element("store", &unique, "install_button")
            .is_some()
    );
}

#[rstest]
fn steam_profile_rejects_uncontrolled_probe_routes_and_unsafe_flow_edits() {
    let source = include_str!("../profiles/steam-chromium.json");
    let mut value: Value = serde_json::from_str(source).expect("Steam profile JSON");
    value["capability_probes"][0]["route"] = json!("visual_fallback");
    assert!(matches!(
        parse_profile(&value.to_string()),
        Err(ProfileError::InvalidCapabilityProbeRoute(..))
    ));

    let mut value: Value = serde_json::from_str(source).expect("Steam profile JSON");
    value["flows"][0]["prohibit_coordinates"] = json!(false);
    assert!(matches!(
        parse_profile(&value.to_string()),
        Err(ProfileError::UnsafeFlowPolicy(..))
    ));
}

#[rstest]
fn invalid_profile_rejects_duplicate_targets() {
    let input = r#"{
        "schema_version": 3,
        "id": "test",
        "profile_version": "1.0.0",
        "application": {"family": "test", "versions": []},
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

#[rstest]
fn invalid_profile_rejects_malformed_locale_tags() {
    let input = r#"{
        "schema_version": 3,
        "id": "test",
        "profile_version": "1.0.0",
        "application": {"family": "test", "versions": []},
        "display_name": "Test",
        "selectors": [{"application_names": ["test.exe"]}],
        "surfaces": [{
            "id": "surface",
            "label": "Surface",
            "role": "panel",
            "route": "accessibility",
            "targets": [{
                "id": "run",
                "label": "Run",
                "role": "button",
                "localized_names": {"not_a_locale": ["Run"]}
            }]
        }],
        "settings": {"dialog_style": "host_owned", "preferred_route": "accessibility"}
    }"#;
    assert!(matches!(
        parse_profile(input),
        Err(ProfileError::InvalidLocale(..))
    ));
    let empty_alias = input.replace("\"not_a_locale\": [\"Run\"]", "\"en\": [\"\"]");
    assert!(matches!(
        parse_profile(&empty_alias),
        Err(ProfileError::InvalidLocalizedAliases(..))
    ));
}

#[rstest]
fn key_bindings_are_bounded_and_require_a_supported_action() {
    let valid = r#"{
        "schema_version": 3,
        "id": "game",
        "profile_version": "1.0.0",
        "application": {"family": "game", "versions": []},
        "display_name": "Game",
        "selectors": [{"application_names": ["game.exe"]}],
        "surfaces": [{
            "id": "inventory",
            "label": "Inventory",
            "role": "inventory",
            "route": "visual_fallback",
            "targets": [{
                "id": "backpack",
                "label": "Backpack",
                "role": "shortcut",
                "supported_actions": ["toggle"],
                "key_bindings": {"toggle": ["SPACE"]}
            }]
        }],
        "settings": {"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}
    }"#;
    let profile = parse_profile(valid).unwrap();
    assert_eq!(
        profile
            .resolve_target("inventory", "backpack")
            .unwrap()
            .key_binding("TOGGLE"),
        Some(["SPACE".to_owned()].as_slice())
    );

    let invalid = valid.replace(
        "\"supported_actions\": [\"toggle\"]",
        "\"supported_actions\": [\"inspect\"]",
    );
    assert!(matches!(
        parse_profile(&invalid),
        Err(ProfileError::InvalidKeyBinding(..))
    ));
}

#[rstest]
fn profile_accepts_a_bounded_read_only_loopback_state_source() {
    let input = r#"{
        "schema_version": 3,
        "id": "the-bazaar",
        "profile_version": "1.0.0",
        "application": {"family": "the-bazaar", "versions": []},
        "display_name": "The Bazaar",
        "selectors": [{"application_names": ["TheBazaar.exe"]}],
        "surfaces": [],
        "state_sources": [{
            "id": "bazaar-agent",
            "type": "loopback_http_json",
            "mode": "read_only",
            "url": "http://127.0.0.1:47900/v1/context",
            "expected_schema_version": "2.2.0",
            "schema_version_pointer": "/schemaVersion",
            "tick_pointer": "/tickId",
            "use_etag": true,
            "timeout_ms": 1000,
            "max_response_bytes": 1048576,
            "optional": true
        }],
        "settings": {"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}
    }"#;

    let profile = parse_profile(input).expect("valid profile state source");
    let source = profile
        .state_source("bazaar-agent")
        .expect("state source by id");
    assert_eq!(source.mode, StateSourceMode::ReadOnly);
    assert_eq!(source.url, "http://127.0.0.1:47900/v1/context");
    assert_eq!(source.expected_schema_version, "2.2.0");
    assert!(source.use_etag);
    assert!(source.optional);
}

#[rstest]
#[case("http://localhost:47900/v1/context")]
#[case("https://127.0.0.1:47900/v1/context")]
#[case("http://127.0.0.1:47900/v1/context?token=secret")]
#[case("http://example.com:47900/v1/context")]
fn profile_rejects_state_sources_that_are_not_literal_http_loopback(#[case] url: &str) {
    let input = format!(
        r#"{{
            "schema_version": 3,
            "id": "unsafe",
            "profile_version": "1.0.0",
            "application": {{"family": "unsafe", "versions": []}},
            "display_name": "Unsafe",
            "selectors": [{{"application_names": ["unsafe.exe"]}}],
            "surfaces": [],
            "state_sources": [{{
                "id": "state",
                "type": "loopback_http_json",
                "mode": "read_only",
                "url": "{url}",
                "expected_schema_version": "1.0.0",
                "schema_version_pointer": "/schemaVersion",
                "tick_pointer": "/tickId",
                "timeout_ms": 1000,
                "max_response_bytes": 1048576
            }}],
            "settings": {{"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}}
        }}"#
    );

    assert!(matches!(
        parse_profile(&input),
        Err(ProfileError::InvalidStateSourceUrl(..))
    ));
}

#[rstest]
fn profile_rejects_undeclared_state_source_capabilities() {
    let input = r#"{
        "schema_version": 3,
        "id": "unsafe",
        "profile_version": "1.0.0",
        "application": {"family": "unsafe", "versions": []},
        "display_name": "Unsafe",
        "selectors": [{"application_names": ["unsafe.exe"]}],
        "surfaces": [],
        "state_sources": [{
            "id": "state",
            "type": "loopback_http_json",
            "mode": "read_only",
            "url": "http://127.0.0.1:47900/v1/context",
            "expected_schema_version": "1.0.0",
            "schema_version_pointer": "/schemaVersion",
            "tick_pointer": "/tickId",
            "timeout_ms": 1000,
            "max_response_bytes": 1048576,
            "action_url": "http://127.0.0.1:47900/v1/action"
        }],
        "settings": {"dialog_style": "application_rendered", "preferred_route": "visual_fallback"}
    }"#;

    assert!(matches!(
        parse_profile(input),
        Err(ProfileError::InvalidJson(_))
    ));
}

fn minimal_profile_value() -> Value {
    json!({
        "schema_version": 3,
        "id": "strict",
        "profile_version": "1.0.0",
        "application": {"family": "strict", "versions": []},
        "display_name": "Strict",
        "selectors": [{"application_names": ["strict.exe"]}],
        "surfaces": [{
            "id": "main",
            "label": "Main",
            "role": "window",
            "route": "accessibility",
            "targets": [{
                "id": "open",
                "label": "Open",
                "role": "button",
                "fallback": {"profile_id": "strict", "surface_id": "main"}
            }]
        }],
        "settings": {
            "dialog_style": "host_owned",
            "preferred_route": "accessibility"
        }
    })
}

#[rstest]
#[case("")]
#[case("/selectors/0")]
#[case("/surfaces/0")]
#[case("/surfaces/0/targets/0")]
#[case("/surfaces/0/targets/0/fallback")]
#[case("/settings")]
fn every_profile_object_rejects_unknown_fields(#[case] object_pointer: &str) {
    let mut profile = minimal_profile_value();
    profile
        .pointer_mut(object_pointer)
        .and_then(Value::as_object_mut)
        .expect("test object")
        .insert("future_or_misspelled_field".into(), json!(true));

    assert!(matches!(
        parse_profile(&profile.to_string()),
        Err(ProfileError::InvalidJson(_))
    ));
}

#[rstest]
fn omitted_destructive_confirmation_defaults_to_fail_closed() {
    let profile = parse_profile(&minimal_profile_value().to_string()).expect("strict profile");

    assert!(profile.settings.destructive_confirmation_required);
}

#[rstest]
fn profile_and_vocabulary_ids_must_be_canonical_lowercase() {
    for pointer in [
        "/id",
        "/surfaces/0/id",
        "/surfaces/0/targets/0/id",
        "/surfaces/0/targets/0/fallback/profile_id",
        "/surfaces/0/targets/0/fallback/surface_id",
    ] {
        let mut profile = minimal_profile_value();
        *profile.pointer_mut(pointer).expect("identifier") = json!("Ambiguous-ID");
        assert!(
            parse_profile(&profile.to_string()).is_err(),
            "{pointer} accepted a non-canonical identifier"
        );
    }
}
