use dcc_cua_core::ComputerUsePoint;
use rstest::rstest;
use serde_json::json;

use crate::actions::{
    map_visible_snapshot_coordinates, snapshot_coordinate_space_value, visible_snapshot_dimensions,
    visible_snapshot_dimensions_for_action,
};

use super::*;

fn observation(width: u32, height: u32, source_rect: [i32; 4]) -> ComputerUseObservation {
    ComputerUseObservation {
        observation_id: "fresh".into(),
        window_handle: 7,
        process_id: 42,
        window_title: "Unity".into(),
        width,
        height,
        source_rect,
        capture_backend: "dcc-cua-wgc-exact-window".into(),
        capture_provenance: json!({"pixels_captured": true}),
        session_id: "session".into(),
    }
}

#[rstest]
fn successful_post_snapshot_names_the_encoded_image_coordinate_space() {
    let value = window_post_snapshot_value(
        Ok(dcc_cua_core::ComputerUseScreenshot {
            data: Vec::new(),
            observation: observation(1568, 931, [-16, -16, 3872, 2312]),
            accessibility: json!({"elements": []}),
        }),
        None,
    );

    assert_eq!(
        value["coordinate_space"]["kind"],
        "exact_window_image_pixels"
    );
    assert_eq!(
        value["coordinate_space"]["dimensions_source"],
        "encoded_png_ihdr"
    );
    assert_eq!(value["coordinate_space"]["width"], 1568);
    assert_eq!(value["coordinate_space"]["height"], 931);
    assert_eq!(
        value["coordinate_space"]["source_rect"],
        json!([-16, -16, 3872, 2312])
    );
}

#[rstest]
fn friendly_visual_action_maps_the_visible_snapshot_into_the_fresh_observation() {
    let mut action = ComputerUseAction {
        action: "double_click".into(),
        x: Some(1318.0),
        y: Some(700.0),
        ..Default::default()
    };

    map_visible_snapshot_coordinates(
        &mut action,
        Some((1568, 931)),
        &observation(3840, 2280, [0, 0, 3840, 2280]),
    )
    .unwrap();
    assert_eq!(action.x, Some(1318.0 * 3840.0 / 1568.0));
    assert_eq!(action.y, Some(700.0 * 2280.0 / 931.0));
}

#[rstest]
fn visible_snapshot_dimensions_require_a_complete_positive_pair() {
    assert!(visible_snapshot_dimensions(&strings(["--observation-width", "1568"])).is_err());
    assert!(
        visible_snapshot_dimensions(&strings([
            "--observation-width",
            "0",
            "--observation-height",
            "931",
        ]))
        .is_err()
    );
    assert_eq!(
        visible_snapshot_dimensions(&strings([
            "--observation-width",
            "1568",
            "--observation-height",
            "931",
        ]))
        .unwrap(),
        Some((1568, 931))
    );
}

#[rstest]
fn coordinate_actions_require_the_declared_image_dimensions() {
    let click = ComputerUseAction {
        action: "click".into(),
        x: Some(100.0),
        y: Some(80.0),
        ..Default::default()
    };
    let error = visible_snapshot_dimensions_for_action(&[], &click).unwrap_err();
    assert!(error.to_string().contains("coordinate actions require"));
    assert_eq!(
        visible_snapshot_dimensions_for_action(
            &strings(["--observation-width", "1568", "--observation-height", "931"]),
            &click,
        )
        .unwrap(),
        Some((1568, 931))
    );

    let semantic = ComputerUseAction {
        action: "click".into(),
        element_index: Some(12),
        ..Default::default()
    };
    assert_eq!(
        visible_snapshot_dimensions_for_action(&[], &semantic).unwrap(),
        None
    );

    let drag = ComputerUseAction {
        action: "drag".into(),
        path: vec![ComputerUsePoint { x: 10.0, y: 20.0 }],
        ..Default::default()
    };
    assert!(visible_snapshot_dimensions_for_action(&[], &drag).is_err());
}

#[rstest]
fn snapshot_coordinate_space_uses_true_png_dimensions_and_action_flags() {
    let coordinate_space =
        snapshot_coordinate_space_value(&observation(1568, 931, [-16, -16, 3872, 2312]));
    assert_eq!(coordinate_space["origin"], "top_left");
    assert_eq!(coordinate_space["width"], 1568);
    assert_eq!(coordinate_space["height"], 931);
    assert_eq!(
        coordinate_space["action_flags"],
        json!({
            "width": "--observation-width",
            "height": "--observation-height",
        })
    );
}
