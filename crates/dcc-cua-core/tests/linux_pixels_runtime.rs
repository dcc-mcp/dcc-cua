//! Real native SDK construction with all upstream dispatch disabled.
#![cfg(target_os = "linux")]

use dcc_cua_core::ComputerUseDriver;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn linux_pixels_native_sdk_registry_has_no_dispatchable_tools() {
    let driver = ComputerUseDriver::create_read_only_pixels().unwrap();
    assert!(!driver.upstream_cursor_renderer_enabled());
    let tools = driver.list_tools().await.unwrap();
    assert_eq!(tools["tools"], serde_json::json!([]));
    for tool in [
        "get_window_state",
        "get_accessibility_tree",
        "start_session",
        "click",
        "clipboard_read",
        "start_recording",
        "run_script",
        "parse_visual_regions",
    ] {
        let result = driver
            .raw()
            .call_tool(tool.into(), "{}".into())
            .await
            .unwrap();
        assert!(result.is_error, "empty native runtime dispatched {tool}");
        assert_eq!(result.text, format!("Unknown tool: {tool}"));
        assert!(result.images.is_empty());
        assert!(result.structured_json.is_none());
    }
    assert!(ComputerUseDriver::create().is_err());
    driver.raw().shutdown().await.unwrap();
    assert!(ComputerUseDriver::create().is_err());
}
