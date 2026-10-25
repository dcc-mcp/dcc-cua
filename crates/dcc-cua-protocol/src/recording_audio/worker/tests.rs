use super::*;
use rstest::rstest;
use std::io::Cursor;

#[rstest]
fn audio_worker_protocol_bounds_before_allocating_payload() {
    let data = (MAX_AUDIO_WORKER_MESSAGE as u32 + 1).to_le_bytes();
    assert!(read_message::<_, AudioWorkerCommand>(&mut Cursor::new(data)).is_err());
    assert!(read_message::<_, AudioWorkerCommand>(&mut Cursor::new([1, 0])).is_err());
    assert!(read_message::<_, AudioWorkerCommand>(&mut Cursor::new([0, 0, 0, 0])).is_err());
}

#[rstest]
fn audio_worker_protocol_is_one_closed_control_surface() {
    let mut bytes = Cursor::new(Vec::new());
    write_message(&mut bytes, &AudioWorkerCommand::Start {}).unwrap();
    bytes.set_position(0);
    assert!(matches!(
        read_message(&mut bytes).unwrap(),
        Some(AudioWorkerCommand::Start {})
    ));
    assert!(
        read_message::<_, AudioWorkerCommand>(&mut bytes)
            .unwrap()
            .is_none()
    );
    for raw in [
        r#"{"type":"start","process_id":99}"#,
        r#"{"type":"desktop"}"#,
        r#"{"type":"microphone"}"#,
    ] {
        let mut bytes = (raw.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(raw.as_bytes());
        assert!(read_message::<_, AudioWorkerCommand>(&mut Cursor::new(bytes)).is_err());
    }
}

#[rstest]
fn worker_failure_keeps_bounded_numeric_diagnostic_and_rejects_extra_fields() {
    let diagnostic = AudioWorkerFailureDiagnostic {
        stage: Some(AudioWorkerFailureStage::PacketValidation),
        hresult: None,
        packet: Some(AudioWorkerPacketMetadata {
            frames: 480,
            flags: 1,
            qpc_100ns: 100,
            device_position: 500,
            expected_device_position: Some(480),
            null_data: false,
        }),
        stream_stop_succeeded: Some(true),
    };
    let event = AudioWorkerEvent::Failed {
        code: AudioWorkerFailure::ContinuityLost,
        diagnostic: Some(diagnostic.clone()),
    };
    let mut wire = Cursor::new(Vec::new());
    write_message(&mut wire, &event).unwrap();
    wire.set_position(0);
    let decoded: AudioWorkerEvent = read_message(&mut wire).unwrap().unwrap();
    assert!(
        matches!(decoded, AudioWorkerEvent::Failed { diagnostic: Some(actual), .. } if actual == diagnostic)
    );
    assert!(serde_json::from_str::<AudioWorkerFailureDiagnostic>(r#"{"stage":null,"hresult":null,"packet":null,"stream_stop_succeeded":null,"samples":[1]}"#).is_err());
}
