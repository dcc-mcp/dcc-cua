use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
fn failed_inventory_is_unknown_even_with_partial_candidates() {
    for rows in [vec![], vec![json!({"pid":42,"window_id":7})]] {
        let count = rows.len();
        let result = finish_inventory(rows, Some("EnumWindows: access denied (os error 5)".into()));
        assert!(
            result.is_err(),
            "failure became a successful inventory: {result:?}"
        );
        let error = result.unwrap_err();
        assert!(error.message.contains("os error 5"));
        let inventory = error.details.unwrap().window_inventory.unwrap();
        assert!(!inventory.complete);
        assert_eq!(inventory.partial_candidates.len(), count);
    }
}

#[rstest]
fn completed_inventory_can_be_empty() {
    assert!(finish_inventory(vec![], None).unwrap().is_empty());
}

#[rstest]
fn metadata_failures_are_scoped_to_candidates_that_could_match_the_query() {
    let good = json!({"pid":42,"window_id":7,"title":"Figma A","app_name":"Figma.exe","is_on_screen":true,"backend":"windows-native-window-inventory","metadata_complete":true});
    let bad = json!({"pid":99,"window_id":8,"title":null,"app_name":"Other.exe","metadata_complete":false,"metadata_errors":["GetWindowTextW: os error 5"]});
    let app = crate::ComputerUseWindowQuery {
        app: Some("Figma".into()),
        ..Default::default()
    };
    assert_eq!(
        app.filter_inventory(vec![good.clone(), bad.clone()])
            .unwrap(),
        vec![good.clone()]
    );
    let pid = crate::ComputerUseWindowQuery {
        process_id: Some(42),
        ..Default::default()
    };
    assert_eq!(
        pid.filter_inventory(vec![good.clone(), bad.clone()])
            .unwrap(),
        vec![good.clone()]
    );
    let unknown_app = json!({"pid":99,"window_id":8,"app_name":null,"metadata_complete":false,"metadata_errors":["OpenProcess: os error 5"]});
    assert!(
        app.filter_inventory(vec![good.clone(), unknown_app.clone()])
            .is_err()
    );
    let exact = crate::ComputerUseWindowQuery {
        window_handle: Some(7),
        ..Default::default()
    };
    assert_eq!(
        exact
            .filter_inventory(vec![good.clone(), unknown_app])
            .unwrap(),
        vec![good]
    );
    assert!(
        crate::ComputerUseWindowQuery::default()
            .filter_inventory(vec![bad.clone()])
            .unwrap()
            .len()
            == 1
    );
    let unknown = crate::ComputerUseWindowQuery {
        process_id: Some(99),
        ..Default::default()
    }
    .filter_inventory(vec![bad])
    .unwrap_err();
    assert!(unknown.message.contains("os error 5"));
}

#[rstest]
fn metadata_failures_preserve_candidates_without_fabricating_identity_or_geometry() {
    for api in [
        "OpenProcess",
        "QueryFullProcessImageNameW",
        "GetWindowThreadProcessId",
        "GetWindowRect",
        "GetWindowTextW",
    ] {
        let row = json!({"pid":null,"window_id":7,"app_name":null,"bounds":null,"title":"private UI text"});
        let error = finish_inventory(vec![row], Some(format!("{api}: os error 5"))).unwrap_err();
        let inventory = error.details.unwrap().window_inventory.unwrap();
        assert!(inventory.reason.contains(api));
        assert!(inventory.reason.contains("os error 5"));
        assert_eq!(inventory.partial_candidates[0].process_id, None);
        assert_eq!(inventory.partial_candidates[0].window_handle, 7);
        assert!(
            !serde_json::to_string(&inventory)
                .unwrap()
                .contains("private UI text")
        );
    }
}

#[rstest]
fn windows_discovery_accepts_only_the_exact_executable_or_its_stem() {
    let row = json!({"app_name":"Figma.exe","backend":"windows-native-window-inventory","pid":42,"window_id":7});
    for app in ["Figma", "figma.EXE"] {
        let query = crate::contracts::ComputerUseWindowQuery {
            app: Some(app.into()),
            ..Default::default()
        };
        assert!(query.matches_window(&row), "{app} did not match");
    }
    for app in ["Fig", "Figma.exe.exe", "Figma.exe/other"] {
        let query = crate::contracts::ComputerUseWindowQuery {
            app: Some(app.into()),
            ..Default::default()
        };
        assert!(!query.matches_window(&row));
    }
    let query = crate::contracts::ComputerUseWindowQuery {
        app: Some("Figma".into()),
        process_id: Some(43),
        ..Default::default()
    };
    assert!(!query.matches_window(&row));
    let query = crate::contracts::ComputerUseWindowQuery {
        app: Some("Figma".into()),
        ..Default::default()
    };
    assert!(!query.matches_window(&json!({"app_name":null})));
    assert!(!query.matches_window(&json!({"app_name":"Figma.exe"})));
    assert!(!query.matches_window(
        &json!({"app_name":"Figma Helper.exe","backend":"windows-native-window-inventory"})
    ));
}
