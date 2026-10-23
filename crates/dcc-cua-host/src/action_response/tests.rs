use super::*;
use rstest::rstest;

fn observation(handle: u64) -> dcc_cua_core::ComputerUseObservation {
    dcc_cua_core::ComputerUseObservation {
        observation_id: format!("obs-{handle}"),
        window_handle: handle,
        process_id: 77,
        window_title: String::new(),
        width: 10,
        height: 10,
        source_rect: [0, 0, 10, 10],
        capture_backend: "dcc-cua-visible-exact-window".into(),
        capture_provenance: json!({}),
        session_id: "session".into(),
    }
}

#[rstest]
fn composite_snapshot_transport_keeps_root_and_selected_image_order_distinct() {
    let overlays = vec![
        ComputerUseScreenshot {
            data: b"a".to_vec(),
            observation: observation(200),
            accessibility: json!({}),
        },
        ComputerUseScreenshot {
            data: b"bc".to_vec(),
            observation: observation(300),
            accessibility: json!({}),
        },
    ];
    let mut public = json!({"overlays":[{"window_handle":200},{"window_handle":300}]});
    let prepared = snapshot_image_transport(
        b"root".to_vec(),
        &observation(100),
        overlays,
        &mut public,
        SnapshotTransport::BinaryFrame,
        &mut None,
    )
    .unwrap();
    assert_eq!(prepared.attachment.unwrap(), b"rootabc");
    assert_eq!(prepared.primary.unwrap()["offset"], 0);
    assert_eq!(public["overlays"][0]["image"]["offset"], 4);
    assert_eq!(public["overlays"][0]["image"]["length"], 1);
    assert_eq!(public["overlays"][1]["image"]["offset"], 5);
    assert_eq!(public["overlays"][1]["image"]["length"], 2);
    assert_eq!(prepared.attachments.len(), 3);
}

#[rstest]
fn ordinary_snapshot_descriptor_remains_unchanged_without_secondary_images() {
    let prepared = snapshot_image_transport(
        b"root".to_vec(),
        &observation(100),
        Vec::new(),
        &mut Value::Null,
        SnapshotTransport::BinaryFrame,
        &mut None,
    )
    .unwrap();
    assert!(prepared.attachments.is_empty());
    assert_eq!(prepared.primary.unwrap()["id"], "obs-100");
}
