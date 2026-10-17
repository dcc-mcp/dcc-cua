use super::*;
use rstest::rstest;

#[derive(Clone, Default)]
struct NoDispatchChannel(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl cua_driver_sdk::remote::DriverEnvelopeChannel for NoDispatchChannel {
    async fn negotiate(&self) -> Result<cua_driver_sdk::remote::DriverChannelCapabilities, String> {
        Ok(cua_driver_sdk::remote::DriverChannelCapabilities {
            minimum_envelope_version: cua_driver_sdk::remote::DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: cua_driver_sdk::remote::DRIVER_ENVELOPE_VERSION,
            supports_cancellation: true,
        })
    }
    async fn exchange(
        &self,
        _: cua_driver_sdk::remote::DriverRequestEnvelope,
    ) -> Result<cua_driver_sdk::remote::DriverResponseEnvelope, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err("native menu boundary test forbids all SDK dispatch".into())
    }
    async fn bind_session(
        &self,
        _: cua_driver_sdk::TrustedSessionOptions,
    ) -> Result<Arc<dyn cua_driver_sdk::remote::DriverEnvelopeChannel>, String> {
        Ok(Arc::new(self.clone()))
    }
    async fn close(&self) -> Result<(), String> {
        Ok(())
    }
    async fn cancel(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn authenticated_principal(&self) -> &str {
        "native-menu-test"
    }
    fn connection_generation(&self) -> &str {
        "generation-1"
    }
}

fn metadata() -> NativeMenuMetadata {
    let popup = dcc_cua_platform_windows::ExactWindowNativeState {
        process_id: 42,
        window_handle: 99,
        bounds: Some([10, 20, 80, 100]),
        visible_bounds: Some([10, 20, 80, 100]),
        dpi: 96,
        visible: true,
        minimized: false,
        maximized: false,
        foreground: false,
        instance: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: 9,
            owner_window_handle: 0,
        },
    };
    NativeMenuMetadata {
        id: "native-menu-state-1".into(),
        session_id: "session-1".into(),
        read_at: Instant::now(),
        proof: dcc_cua_platform_windows::NativeMenuProof {
            popup,
            owner: dcc_cua_platform_windows::ExactWindowNativeState {
                window_handle: 88,
                visible: false,
                ..popup
            },
            flags: 20,
            active_window_handle: 0,
            capture_window_handle: 0,
        },
    }
}

#[rstest]
fn native_menu_authority_is_purpose_session_instance_and_freshness_bound() {
    let scope = ComputerUseTargetScope {
        process_id: Some(42),
        window_handle: Some(99),
        window_title: None,
    };
    metadata()
        .validate(
            "native-menu-state-1",
            &scope,
            "session-1",
            Duration::from_secs(5),
        )
        .unwrap();
    for change in 0..14 {
        let mut proof = metadata();
        let mut actual_scope = scope.clone();
        let mut id = "native-menu-state-1";
        let mut session = "session-1";
        let mut age = Duration::ZERO;
        match change {
            0 => id = "native-state-1",
            1 => id = "snapshot-1",
            2 => session = "session-2",
            3 => age = Duration::from_secs(5) + Duration::from_nanos(1),
            4 => actual_scope.process_id = Some(43),
            5 => actual_scope.window_handle = Some(100),
            6 => proof.proof.popup.instance.process_creation_time_100ns = 0,
            7 => proof.proof.popup.instance.window_thread_id = 0,
            8 => proof.proof.popup.instance.window_class_hash = 0,
            9 => proof.proof.owner.process_id = 43,
            10 => proof.proof.owner.window_handle = 99,
            11 => proof.proof.owner.instance.window_thread_id = 10,
            12 => proof.proof.popup.visible = false,
            _ => proof.proof.popup.minimized = true,
        }
        assert!(
            proof.validate(id, &actual_scope, session, age).is_err(),
            "change {change}"
        );
    }
}

#[rstest]
#[case("token_age", ComputerUseErrorCode::StaleObservation)]
#[case("task_lease", ComputerUseErrorCode::SessionRefreshRequired)]
fn native_menu_expiry_stops_new_dispatch_but_not_read_only_confirmation(
    #[case] expiry: &str,
    #[case] error_code: ComputerUseErrorCode,
) {
    use dcc_cua_platform_windows::NativeMenuGatePhase::{PostDispatch, PreDispatch};
    let metadata = metadata();
    let scope = ComputerUseTargetScope {
        process_id: Some(42),
        window_handle: Some(99),
        window_title: None,
    };
    let admitted_at = metadata.read_at + Duration::from_secs(1);
    let deadline =
        metadata.read_at + Duration::from_secs(if expiry == "task_lease" { 2 } else { 10 });
    metadata
        .validate_gate_authority(
            &metadata.id,
            &scope,
            &metadata.session_id,
            PreDispatch,
            admitted_at,
            deadline,
        )
        .unwrap();
    let confirmed_at = if expiry == "token_age" {
        metadata.read_at + Duration::from_secs(5) + Duration::from_nanos(1)
    } else {
        deadline
    };
    let failure = metadata
        .validate_gate_authority(
            &metadata.id,
            &scope,
            &metadata.session_id,
            PreDispatch,
            confirmed_at,
            deadline,
        )
        .unwrap_err();
    assert_eq!(failure.code, error_code);
    // This is only the authority-time gate. Production still applies its
    // current stop/interrupt/desktop fences and exact native postconditions.
    metadata
        .validate_gate_authority(
            &metadata.id,
            &scope,
            &metadata.session_id,
            PostDispatch,
            confirmed_at,
            deadline,
        )
        .unwrap();
}

#[rstest]
fn native_menu_dispatch_failure_never_claims_confirmation_or_input() {
    for attempted in [false, true] {
        let error = menu_attempt_error(
            ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "stopped"),
            attempted,
        );
        assert_eq!(
            error.code,
            if attempted {
                ComputerUseErrorCode::CompletionUnknown
            } else {
                ComputerUseErrorCode::UserInterrupted
            }
        );
        let details = error.details.unwrap();
        assert_eq!(details.action_attempted, Some(attempted));
        assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
        assert_eq!(
            details.completion,
            Some(if attempted {
                ComputerUseCompletionState::Unknown
            } else {
                ComputerUseCompletionState::Known
            })
        );
        assert_eq!(details.effect_unknown, Some(attempted));
        assert_eq!(details.blind_retry, Some(false));
        assert_eq!(details.fresh_observation_required, Some(true));
    }
}

#[rstest]
#[tokio::test]
async fn native_menu_refused_attempt_consumes_purpose_metadata_and_prior_evidence() {
    let channel = NoDispatchChannel::default();
    let driver = ComputerUseDriver::from_driver((
        cua_driver_sdk::CuaDriver::connect_remote(Arc::new(channel.clone())).unwrap(),
        false,
    ));
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(99),
                window_title: None,
            },
            "test",
            "session-1",
        )
        .unwrap();
    session.active = true;
    session.pixel_observation_route = Some(PixelObservationRoute::ExplicitPixelsOnly);
    session.native_menu_metadata = Some(metadata());
    let prior = session.action_evidence_epoch();
    let failure = session
        .dismiss_native_menu_from_state("native-state-1", Instant::now() + Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(session.native_menu_metadata.is_none());
    assert_ne!(session.action_evidence_epoch(), prior);
    assert_eq!(failure.details.unwrap().action_attempted, Some(false));
    assert_eq!(channel.0.load(std::sync::atomic::Ordering::Relaxed), 0);
}
