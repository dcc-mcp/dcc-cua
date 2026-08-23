use rstest::rstest;
use serde_json::json;
use tokio::io::AsyncReadExt;

use super::*;

#[rstest]
#[tokio::test]
async fn output_error_keeps_the_stream_available_for_the_next_response() {
    let image_response = HostResponse {
        value: json!({
            "request_id": "core-snapshot-1",
            "type": "desktop_session_snapshot",
            "session_id": "desktop-1",
            "image": {
                "encoding": "binary_frame",
                "length": 3,
                "mime_type": "image/png"
            }
        }),
        binary_attachment: Some(b"png".to_vec()),
    };
    let state_response = HostResponse {
        value: json!({
            "request_id": "core-state-2",
            "type": "desktop_session_state",
            "session_id": "desktop-1",
            "active": true
        }),
        binary_attachment: None,
    };
    let (mut reader, mut writer) = tokio::io::duplex(4096);
    let mut metrics = HostJsonlMetrics::default();

    write_measured_jsonl_response(
        &mut writer,
        image_response,
        None,
        0,
        &mut metrics,
        "desktop_session_snapshot",
        HostJsonlResponseFormat::Host,
    )
    .await
    .unwrap();
    write_measured_jsonl_response(
        &mut writer,
        state_response,
        None,
        1,
        &mut metrics,
        "get_desktop_session_state",
        HostJsonlResponseFormat::Host,
    )
    .await
    .unwrap();
    drop(writer);

    let mut output = String::new();
    reader.read_to_string(&mut output).await.unwrap();
    let responses = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["request_id"], "core-snapshot-1");
    assert_eq!(responses[0]["code"], "output_error");
    assert_eq!(responses[1]["request_id"], "core-state-2");
    assert_eq!(responses[1]["type"], "desktop_session_state");
    assert_eq!(responses[1]["active"], true);
}
