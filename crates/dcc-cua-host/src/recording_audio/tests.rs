use super::*;
use dcc_cua_protocol::recording_audio::ProcessAudioTarget;
use rstest::rstest;

#[rstest]
#[case("valid", true)]
#[case("missing_grant", false)]
#[case("missing_action", false)]
#[case("missing_root", false)]
#[case("wrong_pid", false)]
#[case("wrong_hwnd", false)]
#[case("missing_stop", false)]
#[case("invalid_birth", false)]
fn audio_lease_requires_independent_scope_and_exact_recording_identity(
    #[case] change: &str,
    #[case] accepted: bool,
) {
    let mut grant = Some(ProcessAudioGrant {
        target: ProcessAudioTarget {
            process_id: 42,
            process_creation_time_100ns: 123,
            window_handle: 43,
            window_thread_id: 44,
            window_class_hash: 45,
        },
        max_duration_ms: 1000,
        max_output_bytes: 1000000,
    });
    let mut actions = vec![TrustedTaskActionScope {
        action: "recording_audio".into(),
        input_kind: "playback_audio".into(),
        secret_input: false,
        authorization_category: "recording_audio".into(),
        browser_origin: None,
    }];
    let mut methods = vec![
        "recording_start".into(),
        "recording_state".into(),
        "recording_stop".into(),
    ];
    let (mut pid, mut hwnd, mut directory) = (42, 43, Some("issuer-owned-directory"));
    match change {
        "missing_grant" => grant = None,
        "missing_action" => actions.clear(),
        "missing_root" => directory = None,
        "wrong_pid" => pid = 99,
        "wrong_hwnd" => hwnd = 99,
        "missing_stop" => {
            methods.pop();
        }
        "invalid_birth" => grant.as_mut().unwrap().target.process_creation_time_100ns = 0,
        _ => {}
    }
    assert_eq!(
        valid_scope(grant.as_ref(), pid, hwnd, directory, &actions, &methods),
        accepted
    );
}

#[rstest]
fn existing_silent_video_scope_does_not_implicitly_authorize_audio() {
    assert!(valid_scope(
        None,
        42,
        43,
        Some("issuer-owned-directory"),
        &[],
        &["recording_start".into()]
    ));
}
