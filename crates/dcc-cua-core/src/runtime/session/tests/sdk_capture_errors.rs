use super::super::observation::require_sdk_window_image;
use super::*;
use rstest::rstest;

fn window_reply(structured_json: Option<String>) -> cua_driver_sdk::ToolResult {
    cua_driver_sdk::ToolResult {
        is_error: false,
        error_code: None,
        raw_json: "{}".into(),
        text: "Fixture accessibility tree, without pixels".into(),
        structured_json,
        images: Vec::new(),
        degraded: false,
        action: None,
        verification: None,
    }
}

#[rstest]
fn partial_window_reply_preserves_capture_failure_without_accepting_the_tree_as_pixels() {
    let result = window_reply(Some(
        json!({
            "elements": [{"element_token": "fixture:1"}],
            "screenshot_error": "fixture native capture failure"
        })
        .to_string(),
    ));

    let error = require_sdk_window_image(&result).unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
    assert_eq!(
        error.message,
        "CUA window state returned no screenshot: fixture native capture failure"
    );
    assert!(!error.message.contains("Fixture accessibility tree"));
    assert!(!error.message.contains("fixture:1"));
}

#[rstest]
#[case(None)]
#[case(Some("not JSON"))]
#[case(Some("{}"))]
#[case(Some(r#"{"screenshot_error":null}"#))]
#[case(Some(r#"{"screenshot_error":{"message":"not a native error string"}}"#))]
#[case(Some(r#"{"screenshot_error":"   "}"#))]
fn missing_or_invalid_capture_diagnostic_keeps_the_existing_failure(#[case] raw: Option<&str>) {
    let result = window_reply(raw.map(str::to_owned));

    let error = require_sdk_window_image(&result).unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CaptureFailed);
    assert_eq!(error.message, "CUA window state returned no screenshot");
}

#[rstest]
fn window_image_remains_available_to_the_existing_png_validation() {
    let mut result = window_reply(None);
    result.images.push(cua_driver_sdk::ImageContent {
        mime_type: "image/png".into(),
        data_base64: "fixture image data".into(),
    });

    let image = require_sdk_window_image(&result).unwrap();

    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.data_base64, "fixture image data");
}
