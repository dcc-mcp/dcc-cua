use super::gates::{run_gated_preinvalidated_window_mutation, run_preinvalidated_window_mutation};
use super::*;
use cua_driver_sdk::remote::{
    DRIVER_ENVELOPE_VERSION, DriverChannelCapabilities, DriverEnvelopeChannel,
    DriverRequestEnvelope, DriverResponseEnvelope,
};
use cua_driver_sdk::worker::ActionCompletion;

fn structured_error_details(error: &ComputerUseError) -> &ComputerUseErrorDetails {
    error
        .details
        .as_ref()
        .unwrap_or_else(|| panic!("safety-relevant errors must expose structured details"))
}
use cua_driver_sdk::{CuaDriver, DriverError, TrustedSessionOptions};
use rstest::rstest;
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

mod browser_boundaries;
#[cfg(windows)]
mod continuity;
mod degraded_shutdown;
mod dispatch_errors;
mod modal_takeover;
mod recording;
mod visual_only;

#[rstest]
fn failed_window_restore_still_invalidates_action_cache_before_mutation() {
    let invalidated = Cell::new(false);

    let result = run_preinvalidated_window_mutation(
        || invalidated.set(true),
        || {
            assert!(invalidated.get(), "cache must be stale before mutation");
            Err::<(), _>("foreground denied")
        },
    );

    assert_eq!(result, Err("foreground denied"));
    assert!(invalidated.get());
}

#[rstest]
fn restore_input_gate_failure_never_reaches_mutation_or_cache_invalidation() {
    let invalidations = Cell::new(0);
    let mutations = Cell::new(0);

    let result = run_gated_preinvalidated_window_mutation(
        || Err::<(), _>("desktop locked"),
        || invalidations.set(invalidations.get() + 1),
        || {
            mutations.set(mutations.get() + 1);
            Ok(())
        },
    );

    assert_eq!(result, Err("desktop locked"));
    assert_eq!(invalidations.get(), 0);
    assert_eq!(mutations.get(), 0);
}

#[derive(Clone)]
struct CountingRemoteChannel {
    calls: Arc<AtomicUsize>,
    names: Arc<Mutex<Vec<String>>>,
    completion_known: bool,
    tool_is_error: bool,
    response_ok: bool,
}

impl DriverEnvelopeChannel for CountingRemoteChannel {
    fn negotiate<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<dyn Future<Output = Result<DriverChannelCapabilities, String>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(DriverChannelCapabilities {
                minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
                maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
                supports_cancellation: true,
            })
        })
    }

    fn exchange<'life0, 'async_trait>(
        &'life0 self,
        request: DriverRequestEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<DriverResponseEnvelope, String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.names
                .lock()
                .expect("record remote tool name")
                .push(request.name.clone().unwrap_or_default());
            let structured = match request.name.as_deref() {
                Some("list_windows") => json!({
                    "windows": [{
                        "pid": 42,
                        "window_id": 77,
                        "title": "Test DCC",
                        "app_name": "test.exe",
                        "bounds": {"x": 0, "y": 0, "width": 800, "height": 600},
                        "is_foreground": true,
                        "minimized": false,
                        "is_on_screen": true
                    }]
                }),
                Some("get_recording_state" | "start_recording") => json!({
                    "enabled": true,
                    "owner": "refresh-boundary-session",
                }),
                Some("stop_recording") => json!({
                    "enabled": false,
                    "owner": Value::Null,
                }),
                Some("get_session_state") => json!({"active": true}),
                Some("browser_click") => json!({
                    "effect": "unverifiable",
                    "route": "dom"
                }),
                Some("move_cursor") => json!({
                    "effect": "unverifiable",
                    "route": "global_input"
                }),
                _ => json!({"success": true}),
            };
            Ok(DriverResponseEnvelope {
                envelope_version: DRIVER_ENVELOPE_VERSION,
                request_id: request.request_id,
                ok: self.response_ok,
                result: self.response_ok.then(|| {
                    json!({
                        "content": [{"type": "text", "text": "ok"}],
                        "structuredContent": structured,
                        "isError": self.tool_is_error
                    })
                }),
                error: if !self.completion_known {
                    Some("simulated remote completion loss".into())
                } else if !self.response_ok {
                    Some("simulated known tool failure".into())
                } else {
                    None
                },
                error_code: (!self.response_ok).then(|| match request.name.as_deref() {
                    Some("browser_click") => "browser_refused".into(),
                    _ => "backend_unavailable".into(),
                }),
                completion_known: self.completion_known,
            })
        })
    }

    fn bind_session<'life0, 'async_trait>(
        &'life0 self,
        _options: TrustedSessionOptions,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Arc<dyn DriverEnvelopeChannel>, String>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Arc::new(self.clone()) as Arc<dyn DriverEnvelopeChannel>) })
    }

    fn close<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(()) })
    }

    fn cancel<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _request_id: &'life1 str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(()) })
    }

    fn authenticated_principal(&self) -> &str {
        "refresh-boundary-test"
    }

    fn connection_generation(&self) -> &str {
        "generation-1"
    }
}

fn counting_session() -> (ComputerUseSession, Arc<AtomicUsize>) {
    let (session, calls, _) = counting_session_with_names();
    (session, calls)
}

fn counting_session_with_names() -> (
    ComputerUseSession,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
) {
    counting_session_with_completion(true)
}

fn counting_session_with_completion(
    completion_known: bool,
) -> (
    ComputerUseSession,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
) {
    counting_session_with_response(completion_known, false)
}

fn counting_session_with_response(
    completion_known: bool,
    tool_is_error: bool,
) -> (
    ComputerUseSession,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
) {
    counting_session_with_envelope(completion_known, tool_is_error, true)
}

fn counting_session_with_envelope(
    completion_known: bool,
    tool_is_error: bool,
    response_ok: bool,
) -> (
    ComputerUseSession,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let names = Arc::new(Mutex::new(Vec::new()));
    let driver = CuaDriver::connect_remote(Arc::new(CountingRemoteChannel {
        calls: Arc::clone(&calls),
        names: Arc::clone(&names),
        completion_known,
        tool_is_error,
        response_ok,
    }))
    .expect("counting remote driver");
    let driver = ComputerUseDriver::from_driver((driver, false));
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "refresh-boundary-session",
        )
        .expect("test session");
    session.active = true;
    session.upstream_session_state = UpstreamSessionState::Active;
    session.target = Some(WindowTarget {
        pid: 42,
        window_id: 77,
        title: "Test DCC".into(),
        app_name: "test.exe".into(),
        bounds: [0, 0, 800, 600],
        is_foreground: true,
        is_minimized: false,
        is_on_screen: true,
        z_index: None,
    });
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-refresh".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "refresh-boundary-session".into(),
    });
    session.live_observation = Some(LiveObservation::from_test_frame(7, 12));
    session.last_upstream_session_refresh = None;
    (session, calls, names)
}

#[rstest]
#[tokio::test]
async fn session_health_probe_components_never_activate_or_send_input() {
    let (mut session, _, names) = counting_session_with_names();

    let _ = session.target_availability().await;
    let _ = session.recording_state().await;

    let names = names.lock().expect("read health probe tool names");
    assert!(!names.is_empty());
    assert!(
        names
            .iter()
            .all(|name| matches!(name.as_str(), "list_windows" | "get_recording_state")),
        "health probes called a mutation or input tool: {names:?}"
    );
}

#[rstest]
#[tokio::test]
async fn suspended_input_prevents_the_upstream_session_refresh_from_starting() {
    let refresh_calls = Cell::new(0_u32);

    let error = gated_upstream_session_refresh(
        Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "workstation is disconnected",
        )),
        || async {
            refresh_calls.set(refresh_calls.get() + 1);
            Ok(())
        },
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert_eq!(refresh_calls.get(), 0);
}

#[rstest]
#[tokio::test]
async fn target_transition_clears_action_evidence_but_keeps_a_live_freshness_fence() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-transition".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "session-1".into(),
    });
    session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
    session.live_observation = Some(LiveObservation::from_test_frame(7, 12));
    session.recording_active = true;
    session.recording_expected_video = true;
    #[cfg(windows)]
    {
        session.windows_uia = Some(WindowsUiaFallback::new(42, 77));
    }

    session.invalidate_action_observations();

    assert!(session.observation.is_none());
    assert!(session.post_action_live_sequence_fence.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    #[cfg(windows)]
    assert!(session.windows_uia.is_none());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert!(session.live_observation.is_some());
    assert!(session.showcase.is_none());
}

#[rstest]
#[tokio::test]
async fn refresh_timeout_stales_action_evidence_without_ending_the_long_running_session() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    session.active = true;
    session.upstream_session_state = UpstreamSessionState::Active;
    session.target = Some(WindowTarget {
        pid: 42,
        window_id: 77,
        title: "Test DCC".into(),
        app_name: "test.exe".into(),
        bounds: [0, 0, 800, 600],
        is_foreground: true,
        is_minimized: false,
        is_on_screen: true,
        z_index: None,
    });
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-refresh".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "session-1".into(),
    });
    session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
    session.live_observation = Some(LiveObservation::from_test_frame(7, 12));
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .finish_upstream_refresh_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            "CUA refresh CUA session before observation timed out after 15000 ms",
        )
        .with_details(ComputerUseErrorDetails {
            timed_out: Some(true),
            ..Default::default()
        })))
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    let details = structured_error_details(&error);
    assert_eq!(details.timed_out, Some(true));
    assert_eq!(
        details.phase,
        Some(ComputerUseErrorPhase::UpstreamSessionRefresh)
    );
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(
        details.completion,
        Some(ComputerUseCompletionState::Unknown)
    );
    assert_eq!(details.local_session_invalidated, Some(false));
    assert_eq!(details.session_remains_active, Some(true));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert!(session.active);
    assert_eq!(session.target.as_ref().map(|target| target.pid), Some(42));
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert!(session.observation.is_none());
    assert!(session.post_action_live_sequence_fence.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
}

#[rstest]
#[tokio::test]
async fn an_action_never_crosses_a_due_upstream_session_refresh() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    session.active = true;
    session.upstream_session_state = UpstreamSessionState::Active;
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-refresh".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "session-1".into(),
    });
    session.live_observation = Some(LiveObservation::from_test_frame(7, 12));
    session.recording_active = true;
    session.recording_expected_video = true;
    session.last_upstream_session_refresh = None;

    let error = session
        .require_current_upstream_session_for_evidence()
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    let details = structured_error_details(&error);
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert!(session.active);
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.observation.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
}

#[rstest]
#[tokio::test]
async fn successful_refresh_before_observation_requires_a_strictly_new_live_frame() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    session.active = true;
    session.upstream_session_state = UpstreamSessionState::Active;
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-refresh".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "session-1".into(),
    });
    session.live_observation = Some(LiveObservation::from_test_frame(7, 12));
    session.last_upstream_session_refresh = None;

    session.complete_upstream_session_refresh();

    assert!(session.last_upstream_session_refresh.is_some());
    assert!(session.observation.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    assert!(
        session
            .require_current_upstream_session_for_evidence()
            .is_ok()
    );
}

#[rstest]
#[tokio::test]
async fn successful_session_state_refresh_invalidates_the_previous_observation() {
    let (mut session, calls) = counting_session();
    let evidence_epoch_before_refresh = session.action_evidence_epoch();

    let state = session.session_state().await.expect("refreshed state");

    assert!(session.action_evidence_epoch() > evidence_epoch_before_refresh);
    assert_eq!(state["structuredContent"]["active"], true);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    assert!(session.last_upstream_session_refresh.is_some());
    assert!(session.observation.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    let retry = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-refresh".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(retry.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
}

#[cfg(not(windows))]
#[rstest]
#[tokio::test]
async fn accessibility_snapshot_refreshes_a_due_upstream_session_before_observing() {
    let (mut session, calls, names) = counting_session_with_names();
    session.live_observation = None;

    session
        .accessibility_snapshot(128, 16)
        .await
        .expect("semantic observation after refresh");

    assert!(session.last_upstream_session_refresh.is_some());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 4);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        [
            "start_session",
            "list_windows",
            "get_window_state",
            "list_windows"
        ]
    );
}

#[rstest]
#[tokio::test]
async fn starting_a_new_live_observation_advances_the_action_evidence_epoch() {
    let (mut session, _) = counting_session();
    let evidence_epoch_before_start = session.action_evidence_epoch();

    session.begin_live_observation_replacement();

    assert!(session.action_evidence_epoch() > evidence_epoch_before_start);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn known_driver_tool_failure_invalidates_evidence_but_preserves_the_exact_session() {
    let (mut session, calls) = counting_session();
    session.recording_active = true;
    session.recording_expected_video = true;
    let evidence_epoch_before_dispatch = session.action_evidence_epoch();

    let error = session
        .finish_typed_dispatch_result::<()>(
            "call CUA browser_click",
            Ok(Err(DriverError::Tool {
                tool: "browser_click".into(),
                message: "simulated known tool failure".into(),
                error_code: "browser_refused".into(),
            })),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::BrowserRefused);
    let details = error
        .details
        .as_ref()
        .expect("known dispatch failure must expose structured action metadata");
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::ActionDispatch));
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::Unknown));
    assert_eq!(details.completion, Some(ComputerUseCompletionState::Known));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert!(!error.message.contains("action_attempted="));
    assert!(!error.message.contains("completion_unknown="));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_none());
    assert!(session.action_evidence_epoch() > evidence_epoch_before_dispatch);
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn post_dispatch_sdk_failures_are_completion_unknown_and_terminal() {
    let failures = [
        DriverError::Transport {
            socket_path: "test.sock".into(),
            reason: "simulated transport loss".into(),
        },
        DriverError::Protocol {
            reason: "simulated response identity mismatch".into(),
        },
        DriverError::Worker {
            reason: "simulated worker loss".into(),
        },
        DriverError::Remote {
            reason: "simulated remote loss".into(),
        },
    ];

    for failure in failures {
        let (mut session, calls) = counting_session();
        session.recording_active = true;
        session.recording_expected_video = true;

        let error = session
            .finish_typed_dispatch_result::<()>("call CUA browser_click", Ok(Err(failure)))
            .await
            .unwrap_err();

        assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
        let details = structured_error_details(&error);
        assert_eq!(details.action_attempted, Some(true));
        assert_eq!(
            details.completion,
            Some(ComputerUseCompletionState::Unknown)
        );
        assert_eq!(details.blind_retry, Some(false));
        assert!(!session.active);
        assert!(session.target.is_none());
        assert!(session.observation.is_none());
        assert!(session.live_observation.is_none());
        assert!(!session.recording_active);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    }
}

#[rstest]
#[tokio::test]
async fn typed_pre_dispatch_sdk_failures_preserve_session_and_evidence() {
    let failures = [
        DriverError::Configuration {
            reason: "simulated invalid configuration".into(),
        },
        DriverError::InvalidArguments {
            tool: "browser_click".into(),
            reason: "simulated invalid arguments".into(),
        },
        DriverError::Shutdown,
        DriverError::RuntimeAlreadyExists,
    ];

    for failure in failures {
        let (mut session, calls) = counting_session();
        session.recording_active = true;
        session.recording_expected_video = true;

        let error = session
            .finish_typed_dispatch_result::<()>("call CUA browser_click", Ok(Err(failure)))
            .await
            .unwrap_err();

        assert_ne!(error.code, ComputerUseErrorCode::CompletionUnknown);
        let details = structured_error_details(&error);
        assert_eq!(details.phase, Some(ComputerUseErrorPhase::PreDispatch));
        assert_eq!(details.action_attempted, Some(false));
        assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
        assert_eq!(details.blind_retry, Some(false));
        assert!(session.active);
        assert!(session.target.is_some());
        assert!(session.observation.is_some());
        assert!(session.live_observation.is_some());
        assert!(session.recording_active);
        assert!(session.recording_expected_video);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    }
}

#[rstest]
#[tokio::test]
async fn known_complete_native_mutation_invalidates_the_previous_observation() {
    let (mut session, calls) = counting_session();
    session.last_upstream_session_refresh = Some(Instant::now());

    session
        .finish_typed_dispatch_result("call CUA debug_window_info", Ok(Ok(())))
        .await
        .expect("known-complete native mutation");

    assert!(session.active);
    assert!(session.observation.is_none());
    let error = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-refresh".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn successful_fast_mutation_invalidates_old_evidence_and_fences_capture_after() {
    let (mut session, calls) = counting_session();
    let (live_observation, publisher) = LiveObservation::from_test_stream(7, 12);
    session.live_observation = Some(live_observation);

    let result = session.complete_mutating_action(ComputerUseToolResult {
        status: ComputerUseToolStatus::Succeeded,
        value: json!({"success": true, "route": "windows_fast_test"}),
        text: "simulated successful Windows mutation".into(),
        images: Vec::new(),
        degraded: false,
    });

    assert_eq!(result.value["success"], true);
    assert!(session.active);
    assert!(session.observation.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    assert_eq!(
        session.post_action_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    let after_sequence = observation_sequence_fence(
        7,
        None,
        session.post_action_live_sequence_fence,
        session.observation_transition_live_sequence_fence,
    );
    publisher.publish_frame(13, "capture_after");
    let frame = session
        .live_observation
        .as_mut()
        .expect("live observation stays active")
        .latest_after(after_sequence)
        .await
        .expect("capture_after publishes a new frame");
    assert_eq!(frame.sequence(), 13);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn local_post_preflight_failure_invalidates_evidence_and_requires_fresh_capture() {
    let (mut session, calls) = counting_session();
    let (live_observation, publisher) = LiveObservation::from_test_stream(7, 12);
    session.live_observation = Some(live_observation);
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .finish_local_mutation_attempt::<()>(Err(ComputerUseError::new(
            ComputerUseErrorCode::InputFailed,
            "simulated partial Windows SendInput failure",
        )))
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    let details = structured_error_details(&error);
    assert_eq!(
        details.phase,
        Some(ComputerUseErrorPhase::LocalMutationDispatch)
    );
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::Unknown));
    assert_eq!(details.blind_retry, Some(false));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_none());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(session.post_action_live_sequence_fence, None);
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    let after_sequence = observation_sequence_fence(
        7,
        None,
        session.post_action_live_sequence_fence,
        session.observation_transition_live_sequence_fence,
    );
    publisher.publish_frame(13, "post_failure_capture");
    let frame = session
        .live_observation
        .as_mut()
        .expect("live observation stays active")
        .latest_after(after_sequence)
        .await
        .expect("fresh capture follows the partial local mutation");
    assert_eq!(frame.sequence(), 13);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn attempted_fast_failure_consumes_evidence_for_partial_drag_and_synthetic_touch() {
    let mut results = Vec::new();
    let (seed, _) = counting_session();
    let target = seed.target.as_ref().expect("test target");
    results.push(windows_synthetic_touch_result(
        Err("simulated synthetic-touch API rejection".into()),
        target,
        true,
    ));
    results.push(ComputerUseToolResult {
        status: ComputerUseToolStatus::Rejected,
        value: json!({
            "success": false,
            "route": "windows_scoped_fast_input",
            "delivery": {
                "completion_known": false,
                "input_sent": true,
                "path_sent": false,
                "retry_safe": false
            },
            "effect": "unverifiable"
        }),
        text: "simulated partial raw drag".into(),
        images: Vec::new(),
        degraded: true,
    });

    for result in results {
        assert_eq!(result.value["success"], false);
        assert_eq!(result.value["delivery"]["completion_known"], false);
        let (mut session, calls) = counting_session();

        session.complete_attempted_fast_action(result);

        assert!(session.active);
        assert!(session.observation.is_none());
        assert_eq!(
            session.observation_transition_live_sequence_fence,
            Some(LiveObservationFence::new(7, 12))
        );
        assert_eq!(
            session.post_action_live_sequence_fence,
            Some(LiveObservationFence::new(7, 12))
        );
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    }
}

#[cfg(windows)]
#[rstest]
#[tokio::test]
async fn fast_backend_preflight_rejection_preserves_evidence() {
    let (mut session, calls) = counting_session();
    let target = session.target.clone().expect("test target");
    let action = ComputerUseAction {
        action: "drag".into(),
        delivery_mode: Some("foreground".into()),
        input_backend_id: Some(WINDOWS_SYNTHETIC_TOUCH_BACKEND_ID.into()),
        button: Some("right".into()),
        path: vec![
            ComputerUsePoint { x: 10.0, y: 10.0 },
            ComputerUsePoint { x: 20.0, y: 20.0 },
        ],
        ..Default::default()
    };

    let rejection = windows_fast_preflight_rejection(&action, &target)
        .expect("unsupported backend request is rejected before activation");
    let result = session.complete_action(rejection);

    assert_eq!(result.value["success"], false);
    assert_eq!(result.value["effect"], "not_attempted");
    assert!(session.active);
    assert!(session.observation.is_some());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn successful_implicit_activation_consumes_evidence_before_a_later_not_started_failure() {
    let (mut session, calls) = counting_session();

    session
        .run_gated_implicit_activation_attempt(Ok(()), || Ok(()))
        .expect("implicit activation succeeds");
    let error = session
        .finish_typed_dispatch_result::<()>(
            "execute CUA click",
            Ok(Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::NotStarted,
                reason: "simulated rejection after foreground activation".into(),
            })),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(false)
    );
    assert!(session.active);
    assert!(session.observation.is_none());
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 12))
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn failed_implicit_activation_preflight_preserves_evidence() {
    let (mut session, calls) = counting_session();
    let activation_attempts = Cell::new(0_u32);

    let error = session
        .run_gated_implicit_activation_attempt(
            Err(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "simulated activation preflight failure",
            )),
            || {
                activation_attempts.set(activation_attempts.get() + 1);
                Ok(())
            },
        )
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert!(session.active);
    assert!(session.observation.is_some());
    assert_eq!(activation_attempts.get(), 0);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
#[case("foreground action activation final validation")]
#[case("foreground cursor move activation final validation")]
async fn attempted_implicit_activation_error_consumes_evidence_and_requires_fresh_observation(
    #[case] context: &str,
) {
    let (mut session, calls) = counting_session();
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .run_gated_implicit_activation_attempt(Ok(()), || {
            Err::<(), _>(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                format!("{context} failed after foreground mutation"),
            ))
        })
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    let details = structured_error_details(&error);
    assert_eq!(
        details.phase,
        Some(ComputerUseErrorPhase::ActivationDispatch)
    );
    assert_eq!(details.focus_mutation_attempted, Some(true));
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
    assert_eq!(details.fresh_observation_required, Some(true));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_none());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn non_mutating_action_completion_preserves_previous_evidence() {
    let (mut session, calls) = counting_session();

    let result = session.complete_action(ComputerUseToolResult {
        status: ComputerUseToolStatus::Rejected,
        value: json!({"success": false, "effect": "not_attempted"}),
        text: "simulated pre-dispatch rejection".into(),
        images: Vec::new(),
        degraded: true,
    });

    assert_eq!(result.value["success"], false);
    assert!(session.active);
    assert!(session.observation.is_some());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn unknown_read_only_evidence_completion_preserves_the_exact_session_and_recording() {
    let (mut session, calls) = counting_session();
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .finish_read_only_evidence_dispatch_result::<()>(
            "call CUA get_browser_state",
            Ok(Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::Unknown,
                reason: "simulated evidence response loss".into(),
            })),
        )
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    let details = structured_error_details(&error);
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::EvidenceDispatch));
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(
        details.completion,
        Some(ComputerUseCompletionState::Unknown)
    );
    assert!(session.active);
    assert_eq!(session.target.as_ref().map(|target| target.pid), Some(42));
    assert!(session.observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn outer_read_only_evidence_timeout_preserves_the_exact_session_and_recording() {
    let (mut session, calls) = counting_session();
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .finish_read_only_evidence_dispatch_result::<()>(
            "call CUA get_browser_state",
            Err(ComputerUseError::new(
                ComputerUseErrorCode::InputFailed,
                "call CUA get_browser_state timed out",
            )
            .with_details(ComputerUseErrorDetails {
                timed_out: Some(true),
                ..Default::default()
            })),
        )
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    let details = structured_error_details(&error);
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::EvidenceDispatch));
    assert_eq!(details.action_attempted, Some(false));
    assert_eq!(details.timed_out, Some(true));
    assert!(session.active);
    assert_eq!(session.target.as_ref().map(|target| target.pid), Some(42));
    assert!(session.observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

async fn assert_due_refresh_fences_evidence_tool(route: &str) {
    let (mut session, calls) = counting_session();

    let error = match route {
        "zoom" => session
            .zoom(&ComputerUseZoomRequest {
                observation_id: "observation-before-refresh".into(),
                x1: 10.0,
                y1: 20.0,
                x2: 400.0,
                y2: 200.0,
            })
            .await
            .map(|_| ())
            .unwrap_err(),
        "verify_state" => session
            .verify_state(
                json!([{"window": {"exists": true}}]),
                Some(1_000),
                Some(2),
                false,
            )
            .await
            .map(|_| ())
            .unwrap_err(),
        other => panic!("unexpected evidence route {other}"),
    };

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(false)
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_zoom_before_any_driver_call() {
    assert_due_refresh_fences_evidence_tool("zoom").await;
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_verify_state_before_any_driver_call() {
    assert_due_refresh_fences_evidence_tool("verify_state").await;
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_mutating_preflight_before_any_driver_call() {
    let (mut session, calls) = counting_session();

    let error = session.preflight_mutating_bound_tool().await.unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(false)
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_an_action_before_any_driver_call() {
    let (mut session, calls) = counting_session();

    let error = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-refresh".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(false)
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn unavailable_input_gate_makes_direct_core_action_evidence_stale_after_resume() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    session.active = true;
    session.upstream_session_state = UpstreamSessionState::Active;
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-lock".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "session-1".into(),
    });
    session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
    session.recording_active = true;
    session.recording_expected_video = true;
    #[cfg(windows)]
    {
        session.windows_uia = Some(WindowsUiaFallback::new(42, 77));
    }

    let error = session
        .finish_observed_input_gate(Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "workstation locked",
        )))
        .unwrap_err();

    assert_eq!(
        error.code,
        ComputerUseErrorCode::InteractiveDesktopUnavailable
    );
    assert!(session.observation.is_none());
    assert!(session.post_action_live_sequence_fence.is_none());
    #[cfg(windows)]
    assert!(session.windows_uia.is_none());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert!(session.live_observation.is_none());
    assert!(session.showcase.is_none());

    let retry = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-lock".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(retry.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
#[tokio::test]
async fn target_revalidation_failure_makes_direct_core_action_evidence_stale_after_recovery() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    session.active = true;
    session.upstream_session_state = UpstreamSessionState::Active;
    session.observation = Some(ComputerUseObservation {
        observation_id: "observation-before-target-loss".into(),
        window_handle: 77,
        process_id: 42,
        window_title: "Test DCC".into(),
        width: 800,
        height: 600,
        source_rect: [0, 0, 800, 600],
        capture_backend: "test".into(),
        capture_provenance: json!({"accessibility_backend": "windows_uia"}),
        session_id: "session-1".into(),
    });
    session.post_action_live_sequence_fence = Some(LiveObservationFence::new(7, 11));
    session.recording_active = true;
    session.recording_expected_video = true;
    #[cfg(windows)]
    {
        session.windows_uia = Some(WindowsUiaFallback::new(42, 77));
    }

    let error = session
        .finish_observed_target_revalidation(Err(ComputerUseError::new(
            ComputerUseErrorCode::TargetUnavailable,
            "exact target identity changed",
        )))
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::TargetUnavailable);
    assert!(session.observation.is_none());
    assert!(session.post_action_live_sequence_fence.is_none());
    #[cfg(windows)]
    assert!(session.windows_uia.is_none());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);

    let retry = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-target-loss".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(retry.code, ComputerUseErrorCode::StaleObservation);
}

#[rstest]
#[tokio::test]
async fn direct_core_resume_rebaselines_past_frames_cached_while_input_was_suspended() {
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(42),
                window_handle: Some(77),
                window_title: None,
            },
            "Test DCC",
            "session-1",
        )
        .unwrap();
    let (live_observation, publisher) = LiveObservation::from_test_stream(7, 10);
    session.live_observation = Some(live_observation);

    session
        .finish_observed_input_gate(Err(ComputerUseError::new(
            ComputerUseErrorCode::InteractiveDesktopUnavailable,
            "workstation locked",
        )))
        .unwrap_err();
    publisher.publish_frame(20, "suspended_capture");

    session.finish_observed_input_gate(Ok(())).unwrap();
    session.rebaseline_live_observation_transition_fence();
    assert_eq!(
        session.observation_transition_live_sequence_fence,
        Some(LiveObservationFence::new(7, 20))
    );
    let after_sequence = observation_sequence_fence(
        7,
        None,
        None,
        session.observation_transition_live_sequence_fence,
    );
    publisher.publish_frame(21, "resumed_capture");
    let frame = session
        .live_observation
        .as_mut()
        .unwrap()
        .latest_after(after_sequence)
        .await
        .unwrap();

    assert_eq!(frame.sequence(), 21);
}

#[rstest]
#[case("get_browser_state", json!({}), BrowserToolDisposition::ReadOnlyEvidence)]
#[case(
    "browser_dialog",
    json!({"action": "inspect"}),
    BrowserToolDisposition::ReadOnlyEvidence
)]
#[case("browser_prepare", json!({}), BrowserToolDisposition::PotentialMutation)]
#[case("browser_navigate", json!({}), BrowserToolDisposition::PotentialMutation)]
#[case("browser_click", json!({}), BrowserToolDisposition::PotentialMutation)]
#[case("browser_type", json!({}), BrowserToolDisposition::PotentialMutation)]
#[case("browser_pointer", json!({}), BrowserToolDisposition::PotentialMutation)]
#[case(
    "browser_set_input_files",
    json!({}),
    BrowserToolDisposition::PotentialMutation
)]
#[case(
    "browser_dialog",
    json!({"action": "accept"}),
    BrowserToolDisposition::PotentialMutation
)]
#[case(
    "browser_dialog",
    json!({"action": "dismiss"}),
    BrowserToolDisposition::PotentialMutation
)]
fn browser_route_table_is_fail_closed(
    #[case] name: &str,
    #[case] arguments: Value,
    #[case] expected: BrowserToolDisposition,
) {
    assert_eq!(browser_tool_route(name, &arguments), Some(expected));
    assert_eq!(
        browser_tool_requires_input(name, &arguments),
        expected == BrowserToolDisposition::PotentialMutation
    );
}

#[rstest]
#[case("browser_unknown", json!({}))]
#[case("browser_dialog", json!({}))]
#[case("browser_dialog", json!({"action": "unexpected"}))]
fn unknown_browser_route_is_rejected(#[case] name: &str, #[case] arguments: Value) {
    assert_eq!(browser_tool_route(name, &arguments), None);
    assert!(!browser_tool_requires_input(name, &arguments));
}

#[rstest]
#[tokio::test]
async fn unknown_browser_dialog_action_is_rejected_before_dispatch() {
    let (mut session, calls) = counting_session();

    let error = session
        .call_browser_tool("browser_dialog", json!({"action": "unexpected"}))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

async fn assert_due_browser_mutation_is_fenced(name: &str, arguments: Value) {
    let (mut session, calls) = counting_session();

    let error = session
        .call_browser_tool(name, arguments)
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(false)
    );
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_browser_prepare_before_dispatch() {
    assert_due_browser_mutation_is_fenced("browser_prepare", json!({})).await;
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_browser_dialog_accept_before_dispatch() {
    assert_due_browser_mutation_is_fenced(
        "browser_dialog",
        json!({"action": "accept", "dialog_id": "dialog-1"}),
    )
    .await;
}

async fn assert_browser_evidence_refreshes_before_dispatch(name: &str, arguments: Value) {
    let (mut session, calls, names) = counting_session_with_names();
    let route = browser_tool_route(name, &arguments).expect("known browser evidence route");
    assert_eq!(route, BrowserToolDisposition::ReadOnlyEvidence);

    session
        .refresh_upstream_session_before_observation_if_needed()
        .await
        .expect("refresh before browser evidence");
    let mut object = arguments
        .as_object()
        .cloned()
        .expect("browser evidence arguments");
    object.insert("session".into(), json!(session.session_id));
    let result = session
        .dispatch_browser_tool(route, name, Value::Object(object), INPUT_CALL_TIMEOUT)
        .await
        .expect("fresh browser evidence dispatch");
    let value: Value = serde_json::from_str(&result.raw_json).expect("browser evidence JSON");

    assert_eq!(value["isError"], false);
    assert_eq!(value["content"][0]["text"], "ok");
    assert!(session.last_upstream_session_refresh.is_some());
    assert!(session.observation.is_none());
    assert!(session.active);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["start_session", name]
    );
}

#[rstest]
#[tokio::test]
async fn due_refresh_precedes_browser_snapshot_evidence() {
    assert_browser_evidence_refreshes_before_dispatch("get_browser_state", json!({})).await;
}

#[rstest]
#[tokio::test]
async fn due_refresh_precedes_browser_dialog_inspection_evidence() {
    assert_browser_evidence_refreshes_before_dispatch(
        "browser_dialog",
        json!({"action": "inspect"}),
    )
    .await;
}

#[rstest]
#[tokio::test]
async fn unknown_browser_mutation_dispatch_invalidates_the_exact_session() {
    let (mut session, calls, names) = counting_session_with_completion(false);

    let error = session
        .dispatch_browser_tool(
            BrowserToolDisposition::PotentialMutation,
            "browser_click",
            json!({"session": session.session_id}),
            INPUT_CALL_TIMEOUT,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(true)
    );
    assert!(!session.active);
    assert!(session.target.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["browser_click"]
    );
}

#[rstest]
#[tokio::test]
async fn unknown_browser_evidence_dispatch_preserves_the_exact_session_and_recording() {
    let (mut session, calls, names) = counting_session_with_completion(false);
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .dispatch_browser_tool(
            BrowserToolDisposition::ReadOnlyEvidence,
            "get_browser_state",
            json!({"session": session.session_id}),
            INPUT_CALL_TIMEOUT,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    let details = structured_error_details(&error);
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::EvidenceDispatch));
    assert_eq!(details.action_attempted, Some(false));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_some());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["get_browser_state"]
    );
}

#[rstest]
#[tokio::test]
async fn known_complete_browser_mutation_invalidates_native_evidence_before_cross_domain_use() {
    let (mut session, calls, names) = counting_session_with_names();
    session.last_upstream_session_refresh = Some(Instant::now());

    session
        .dispatch_browser_tool(
            BrowserToolDisposition::PotentialMutation,
            "browser_click",
            json!({"session": session.session_id}),
            INPUT_CALL_TIMEOUT,
        )
        .await
        .expect("known-complete browser mutation");

    assert!(session.active);
    assert!(session.observation.is_none());
    let error = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-refresh".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["browser_click"]
    );
}

#[rstest]
#[tokio::test]
async fn known_complete_browser_tool_error_still_invalidates_native_evidence() {
    let (mut session, calls, names) = counting_session_with_response(true, true);
    session.last_upstream_session_refresh = Some(Instant::now());

    let error = session
        .dispatch_browser_tool(
            BrowserToolDisposition::PotentialMutation,
            "browser_click",
            json!({"session": session.session_id}),
            INPUT_CALL_TIMEOUT,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert!(session.active);
    assert!(session.observation.is_none());
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["browser_click"]
    );
}

#[rstest]
#[tokio::test]
async fn remote_driver_tool_failure_uses_known_attempted_mutation_semantics() {
    let (mut session, calls, names) = counting_session_with_envelope(true, false, false);
    session.last_upstream_session_refresh = Some(Instant::now());
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .dispatch_browser_tool(
            BrowserToolDisposition::PotentialMutation,
            "browser_click",
            json!({"session": session.session_id}),
            INPUT_CALL_TIMEOUT,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::BrowserRefused);
    let details = structured_error_details(&error);
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.completion, Some(ComputerUseCompletionState::Known));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_none());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["browser_click"]
    );
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_native_extension_before_schema_or_dispatch() {
    let (mut session, calls) = counting_session();

    let error = session
        .call_tool("debug_window_info", json!({}))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn due_refresh_fences_trusted_browser_download_before_dispatch() {
    let (mut session, calls) = counting_session();

    let error = session
        .call_browser_download_tool(json!({
            "target_id": "target-1",
            "tab_id": "tab-1",
            "ref": "ref-1",
            "destination_root": "C:\\bounded"
        }))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::SessionRefreshRequired);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(session.observation.is_none());
}

#[rstest]
#[tokio::test]
async fn trusted_browser_download_rejects_unbounded_arguments_before_any_dispatch() {
    let (mut session, calls) = counting_session();
    let oversized = "x".repeat(MAX_NATIVE_TOOL_ARGUMENT_BYTES + 1);

    let error = session
        .call_browser_download_tool(json!({"destination_root": oversized}))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert!(error.message.contains("arguments exceed"));
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn known_complete_cursor_move_consumes_evidence_before_cross_domain_use() {
    let (mut session, calls, names) = counting_session_with_names();
    session.last_upstream_session_refresh = Some(Instant::now());
    session.recording_active = true;
    session.recording_expected_video = true;

    session
        .call_mutating_bound_tool_without_refresh(
            "move_cursor",
            json!({"x": 10.0, "y": 20.0, "scope": "desktop"}),
        )
        .await
        .expect("known-complete cursor move");

    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_none());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    let error = session
        .perform_action(&ComputerUseAction {
            action: "click".into(),
            observation_id: Some("observation-before-refresh".into()),
            x: Some(10.0),
            y: Some(10.0),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["move_cursor"]
    );
}

#[rstest]
#[tokio::test]
async fn known_attempted_cursor_move_failure_consumes_evidence_but_preserves_session() {
    let (mut session, calls, names) = counting_session_with_envelope(true, false, false);
    session.last_upstream_session_refresh = Some(Instant::now());
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .call_mutating_bound_tool_without_refresh(
            "move_cursor",
            json!({"x": 10.0, "y": 20.0, "scope": "desktop"}),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    let details = structured_error_details(&error);
    assert_eq!(details.action_attempted, Some(true));
    assert_eq!(details.completion, Some(ComputerUseCompletionState::Known));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_none());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["move_cursor"]
    );
}

#[rstest]
#[tokio::test]
async fn unknown_cursor_move_completion_invalidates_the_exact_session() {
    let (mut session, calls, names) = counting_session_with_completion(false);
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .call_mutating_bound_tool_without_refresh(
            "move_cursor",
            json!({"x": 10.0, "y": 20.0, "scope": "desktop"}),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::CompletionUnknown);
    assert_eq!(
        structured_error_details(&error).action_attempted,
        Some(true)
    );
    assert!(!session.active);
    assert!(session.target.is_none());
    assert!(session.observation.is_none());
    assert!(session.live_observation.is_none());
    assert!(!session.recording_active);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(
        *names.lock().expect("read remote tool names"),
        ["move_cursor"]
    );
}

#[rstest]
#[tokio::test]
async fn not_started_cursor_move_preserves_session_and_evidence() {
    let (mut session, calls, names) = counting_session_with_names();
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .finish_typed_dispatch_result::<()>(
            "call CUA move_cursor",
            Ok(Err(DriverError::ActionInterrupted {
                completion: ActionCompletion::NotStarted,
                reason: "cursor move rejected before dispatch".into(),
            })),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    let details = structured_error_details(&error);
    assert_eq!(details.phase, Some(ComputerUseErrorPhase::PreDispatch));
    assert_eq!(details.action_attempted, Some(false));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_some());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(names.lock().expect("read remote tool names").is_empty());
}

#[rstest]
#[tokio::test]
async fn cursor_move_preflight_rejection_preserves_session_and_evidence_without_dispatch() {
    let (mut session, calls, names) = counting_session_with_names();
    session.recording_active = true;
    session.recording_expected_video = true;

    let error = session
        .cursor_tool("move_cursor", json!({"x": 10.0, "scope": "window"}))
        .await
        .unwrap_err();

    assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
    assert!(error.message.contains("requires numeric x and y"));
    assert!(session.active);
    assert!(session.target.is_some());
    assert!(session.observation.is_some());
    assert!(session.live_observation.is_some());
    assert!(session.recording_active);
    assert!(session.recording_expected_video);
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
    assert!(names.lock().expect("read remote tool names").is_empty());
}
