use super::{state::PreparationState, *};
use rstest::rstest;

#[rstest]
#[case(100, 1_000, 1_100, Ok(1_100))]
#[case(350, 1_000, 1_100, Ok(1_100))]
#[case(900, 100, 1_100, Ok(1_000))]
#[case(1_100, 100, 1_100, Err(PreparationFailure::Expired))]
#[case(1_101, 100, 1_100, Err(PreparationFailure::Expired))]
#[case(100, 0, 1_100, Err(PreparationFailure::InvalidBinding))]
#[case(100, MAX_PREPARATION_LIFETIME_MS + 1, u64::MAX, Err(PreparationFailure::InvalidBinding))]
#[case(u64::MAX - 1, 2, u64::MAX, Err(PreparationFailure::InvalidBinding))]
fn trusted_fixed_deadline_cannot_slide_after_target_lookup(
    #[case] issued: u64,
    #[case] lifetime: u64,
    #[case] authorization: u64,
    #[case] expected: Result<u64, PreparationFailure>,
) {
    assert_eq!(
        super::state::effective_deadline(issued, lifetime, authorization)
            .map_err(|error| error.reason),
        expected
    );
}

#[rstest]
fn restore_sequence_overflow_never_reuses_a_journal_sequence() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.promotion_returned = true;
    state.status.pending_sequence = None;
    state.status.last_completed_sequence = Some(u64::MAX);
    state.revoke(PreparationFailure::Stopped);
    assert!(!state.dispatch_restore());
    assert_eq!(state.status.phase, PreparationPhase::CleanupUnknown);
    assert_eq!(state.status.pending_sequence, None);
    assert!(!state.status.cleanup_verified);
    assert_eq!(
        state.status.failure.unwrap().reason,
        PreparationFailure::ProtocolMismatch
    );
}

#[cfg(windows)]
#[rstest]
fn query_sequence_mismatch_and_overflow_revoke_without_increment_or_exit() {
    let mut expected = 1;
    assert!(super::process::accept_query_sequence(&mut expected, 1));
    assert_eq!(expected, 2);
    assert!(!super::process::accept_query_sequence(&mut expected, 1));
    assert_eq!(expected, 2);
    expected = u64::MAX;
    assert!(!super::process::accept_query_sequence(
        &mut expected,
        u64::MAX
    ));
    assert_eq!(expected, u64::MAX);
}

fn receipt(
    sequence: u64,
    kind: PreparationMutationKind,
    success: bool,
) -> PreparationMutationReceipt {
    PreparationMutationReceipt {
        sequence,
        kind,
        returned_at_ms: 10,
        api_success: success,
        os_error: (!success).then_some(5),
        readback: vec![],
        failure: None,
        native_calls: vec![],
    }
}

#[rstest]
#[case(PreparationFailure::Stopped)]
#[case(PreparationFailure::ParentDied)]
#[case(PreparationFailure::Disconnected)]
#[case(PreparationFailure::Expired)]
fn revoke_while_native_call_is_pending_waits_for_actual_return(#[case] reason: PreparationFailure) {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.revoke(reason);
    assert!(!state.dispatch_restore());
    assert_eq!(state.status.pending_sequence, Some(1));
    assert!(!state.status.cleanup_verified);
    assert!(!state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 20));
    assert!(state.dispatch_restore());
    assert!(!state.dispatch_restore());
}

#[rstest]
fn late_promotion_cannot_reenable_capture() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    assert!(!state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 100));
    assert_eq!(state.status.phase, PreparationPhase::RestorePending);
    assert!(state.dispatch_restore());
}

#[rstest]
fn partial_native_error_still_requires_restoration() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    assert!(!state.promotion_completed(receipt(1, PreparationMutationKind::Promote, false), 20));
    assert!(state.dispatch_restore());
}

#[rstest]
fn worker_loss_does_not_fake_cancel_or_release_pending_sequence() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.unknown(PreparationFailure::WorkerLost);
    assert_eq!(state.status.phase, PreparationPhase::CleanupUnknown);
    assert_eq!(state.status.pending_sequence, Some(1));
    assert!(!state.dispatch_restore());
    assert!(!state.status.cleanup_verified);
}

#[rstest]
fn restore_requires_exact_sequence_and_real_readback() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    assert!(state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 20));
    state.revoke(PreparationFailure::Stopped);
    assert!(state.dispatch_restore());
    state.restore_completed(receipt(2, PreparationMutationKind::Restore, true), false);
    assert_eq!(state.status.phase, PreparationPhase::CleanupUnknown);
    assert!(!state.status.cleanup_verified);
}

#[rstest]
fn verified_restoration_never_restores_capture_permission() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    assert!(state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 20));
    state.revoke(PreparationFailure::Stopped);
    assert!(state.dispatch_restore());
    state.restore_completed(receipt(2, PreparationMutationKind::Restore, true), true);
    assert_eq!(state.status.phase, PreparationPhase::Restored);
    assert!(state.status.capture_revoked);
    assert!(state.status.cleanup_verified);
}

#[rstest]
fn wrong_return_sequence_preserves_the_real_pending_call() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    assert!(!state.promotion_completed(receipt(9, PreparationMutationKind::Promote, true), 20));
    assert_eq!(state.status.pending_sequence, Some(1));
    assert!(!state.dispatch_restore());
}

#[rstest]
fn failed_restoration_can_retry_only_after_actual_return_in_a_new_sequence() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 20);
    state.revoke(PreparationFailure::Stopped);
    assert!(state.dispatch_restore());
    state.restore_completed(receipt(2, PreparationMutationKind::Restore, false), false);
    assert_eq!(state.status.last_completed_sequence, Some(2));
    assert!(state.dispatch_restore());
    assert_eq!(state.status.pending_sequence, Some(3));
    assert!(!state.dispatch_restore());
    state.restore_completed(receipt(3, PreparationMutationKind::Restore, true), true);
    assert!(state.status.cleanup_verified);
    assert!(state.status.capture_revoked);
}

#[rstest]
fn an_authenticated_late_return_after_unknown_state_allows_only_restoration() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.unknown(PreparationFailure::WorkerLost);
    assert!(!state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 20));
    assert!(state.dispatch_restore());
    assert_eq!(state.status.phase, PreparationPhase::RestorePending);
}

#[rstest]
fn partial_restore_preserves_actual_native_returns_and_cannot_claim_cleanup() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.revoke(PreparationFailure::ParentDied);
    state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 150);
    assert!(state.dispatch_restore());
    let mut partial = receipt(2, PreparationMutationKind::Restore, false);
    partial.native_calls = vec![
        PreparationNativeCallReceipt {
            window_handle: 2,
            insert_after: -2,
            api_success: true,
            os_error: None,
            returned_at_ms: 151,
        },
        PreparationNativeCallReceipt {
            window_handle: 2,
            insert_after: 3,
            api_success: false,
            os_error: Some(5),
            returned_at_ms: 152,
        },
    ];
    let exact_returns = partial.native_calls.clone();
    state.restore_completed(partial, true);
    assert!(!state.status.cleanup_verified);
    assert_eq!(
        state.status.last_mutation.as_ref().unwrap().native_calls,
        exact_returns
    );
    assert_eq!(state.status.phase, PreparationPhase::CleanupUnknown);
    assert!(state.dispatch_restore());
    assert_eq!(state.status.pending_sequence, Some(3));
}

#[cfg(windows)]
#[rstest]
fn affected_scope_follows_exact_owner_graph_without_pid_wide_expansion() {
    let census = vec![(30, 20), (20, 10), (10, 0), (40, 0), (50, 40)];
    assert_eq!(
        super::native::connected_scope(20, &census).unwrap(),
        vec![30, 20, 10]
    );
    assert_eq!(
        super::native::connected_scope(40, &census).unwrap(),
        vec![40, 50]
    );
}

#[cfg(windows)]
#[rstest]
fn incomplete_owner_graph_is_refused_instead_of_guessing_an_affected_scope() {
    assert_eq!(
        super::native::connected_scope(10, &[(10, 99)])
            .unwrap_err()
            .reason,
        PreparationFailure::EnumerationIncomplete
    );
}

#[cfg(windows)]
#[rstest]
fn inconsistent_owner_cycle_is_refused_without_mutating_any_window() {
    assert_eq!(
        super::native::connected_scope(10, &[(10, 20), (20, 10)])
            .unwrap_err()
            .reason,
        PreparationFailure::EnumerationIncomplete
    );
}

fn window(foreground: bool) -> PreparedWindowState {
    PreparedWindowState {
        identity: PreparationWindowIdentity {
            process_id: 1,
            window_handle: 2,
            native_instance: PreparationNativeInstance {
                process_creation_time_100ns: 3,
                window_thread_id: 4,
                window_class_hash: 5,
                owner_window_handle: 0,
            },
            executable: PreparationExecutableIdentity {
                canonical_image_path: "F:\\fixture.exe".into(),
                volume_serial_number: 1,
                file_id: [1; 16],
            },
        },
        topmost: false,
        bounds: [0, 0, 100, 100],
        visible_bounds: [0, 0, 100, 100],
        dpi: 96,
        visible: true,
        minimized: false,
        foreground,
        anchors: PreparationAnchors {
            above: None,
            below: None,
        },
    }
}

#[cfg(windows)]
#[rstest]
fn passive_preparation_restoration_does_not_claim_keyboard_foreground() {
    assert!(super::native::restored_matches(
        &[window(false)],
        &[window(true)]
    ));
    let mut changed = window(false);
    changed.topmost = true;
    assert!(!super::native::restored_matches(
        &[window(false)],
        &[changed]
    ));
}

#[cfg(windows)]
#[rstest]
fn reused_native_instance_or_changed_geometry_cannot_verify_restoration() {
    let original = window(false);
    let mut reused = original.clone();
    reused.identity.native_instance.process_creation_time_100ns += 1;
    assert!(!super::native::restored_matches(
        std::slice::from_ref(&original),
        &[reused]
    ));
    let mut moved = original.clone();
    moved.bounds[0] += 1;
    assert!(!super::native::restored_matches(&[original], &[moved]));
}

#[cfg(windows)]
#[rstest]
fn handles_and_sealed_guards_are_send_sync_without_executing_native_code() {
    fn assert_traits<T: Send + Sync>() {}
    assert_traits::<CapturePreparationHandle>();
    assert_traits::<PreparedEvidenceGuard>();
}

#[rstest]
fn cleanup_is_not_reported_until_the_exact_durable_settlement_is_acknowledged() {
    let mut state = PreparationState::new([1; 16], 100, vec![]);
    state.promotion_completed(receipt(1, PreparationMutationKind::Promote, true), 20);
    state.revoke(PreparationFailure::Stopped);
    state.dispatch_restore();
    state.restore_completed(receipt(2, PreparationMutationKind::Restore, true), true);
    let fixed = state.await_settlement();
    assert!(!state.status.cleanup_verified);
    assert_eq!(state.status.phase, PreparationPhase::RestorePending);
    let mut substituted = fixed.clone();
    substituted.preparation_id = [2; 16];
    assert!(!state.persisted_settlement(&fixed, &substituted));
    assert!(!state.status.cleanup_verified);
    assert!(state.persisted_settlement(&fixed, &fixed));
    assert!(state.status.cleanup_verified);
    assert!(state.status.capture_revoked);
}

struct FileFixture {
    root: std::path::PathBuf,
    temporary_parent: std::path::PathBuf,
}
impl FileFixture {
    fn new() -> Self {
        let temporary_parent = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let root =
            temporary_parent.join(format!("dcc-cua-preparation-pure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self {
            root,
            temporary_parent,
        }
    }
    fn spec(&self, id: u8) -> CapturePreparationSpec {
        CapturePreparationSpec {
            preparation_id: [id; 16],
            target: window(false).identity,
            allowed_affected_scope: vec![],
            lifetime_ms: 1_000,
            authorization_deadline_ms: 10_000,
            journal_directory: self.root.clone(),
        }
    }
    fn index(&self) -> std::path::PathBuf {
        self.root.join("target-1-3-2.json")
    }
}
impl Drop for FileFixture {
    fn drop(&mut self) {
        // Delete only this fresh, canonical, uniquely named fixture child.
        if let Ok(actual) = std::fs::canonicalize(&self.root)
            && actual.parent() == Some(self.temporary_parent.as_path())
            && actual.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("dcc-cua-preparation-pure-")
            })
        {
            let _ = std::fs::remove_dir_all(actual);
        }
    }
}

#[rstest]
fn immutable_journal_accepts_identical_retry_and_refuses_substitution() {
    let fixture = FileFixture::new();
    let journal = super::journal::Journal::prepare(&fixture.spec(1)).unwrap();
    journal.record("sequence-1-dispatch", &[1, 2, 3]).unwrap();
    journal.record("sequence-1-dispatch", &[1, 2, 3]).unwrap();
    assert_eq!(
        journal
            .record("sequence-1-dispatch", &[4, 5, 6])
            .unwrap_err()
            .reason,
        PreparationFailure::JournalFailed
    );
    assert_eq!(
        std::fs::read(journal.directory.join("sequence-1-dispatch.json")).unwrap(),
        b"[1,2,3]"
    );
}

#[rstest]
fn partial_journal_record_is_never_repaired_or_reported_as_durable_success() {
    let fixture = FileFixture::new();
    let journal = super::journal::Journal::prepare(&fixture.spec(1)).unwrap();
    let path = journal.directory.join("settled.json");
    std::fs::write(&path, b"{\"cleanup_verified\":").unwrap();
    assert_eq!(
        journal.record("settled", &true).unwrap_err().reason,
        PreparationFailure::JournalFailed
    );
    assert_eq!(std::fs::read(path).unwrap(), b"{\"cleanup_verified\":");
}

#[rstest]
fn a_new_epoch_cannot_bypass_an_unresolved_target_journal() {
    let fixture = FileFixture::new();
    let first = super::journal::Journal::prepare(&fixture.spec(1)).unwrap();
    let fixed_index = std::fs::read(fixture.index()).unwrap();
    assert_eq!(
        super::journal::Journal::prepare(&fixture.spec(2))
            .err()
            .unwrap()
            .reason,
        PreparationFailure::GateBusy
    );
    assert_eq!(std::fs::read(fixture.index()).unwrap(), fixed_index);
    assert!(!first.directory.join("settled.json").exists());
}

#[rstest]
#[case(PreparationPhase::Restored)]
#[case(PreparationPhase::Refused)]
fn only_exact_durable_terminal_cleanup_allows_the_next_epoch(#[case] phase: PreparationPhase) {
    let fixture = FileFixture::new();
    let first = super::journal::Journal::prepare(&fixture.spec(1)).unwrap();
    let mut settled = PreparationState::new([1; 16], 100, vec![]).status;
    settled.journal_path = first.directory.clone();
    settled.phase = phase;
    settled.pending_sequence = None;
    settled.capture_revoked = true;
    settled.cleanup_verified = true;
    first.record("settled", &settled).unwrap();
    let second = super::journal::Journal::prepare(&fixture.spec(2)).unwrap();
    assert_ne!(first.directory, second.directory);
    assert!(first.directory.join("settled.json").is_file());
    let actual: std::path::PathBuf =
        serde_json::from_slice(&std::fs::read(fixture.index()).unwrap()).unwrap();
    assert_eq!(actual, second.directory);
}

#[rstest]
#[case("foreign_id")]
#[case("pending_call")]
#[case("active")]
#[case("capture_live")]
#[case("cleanup_unknown")]
fn terminal_record_must_match_the_exact_epoch_and_settled_state(#[case] substitution: &str) {
    let fixture = FileFixture::new();
    let first = super::journal::Journal::prepare(&fixture.spec(1)).unwrap();
    let mut settled = PreparationState::new([1; 16], 100, vec![]).status;
    settled.journal_path = first.directory.clone();
    settled.phase = PreparationPhase::Restored;
    settled.pending_sequence = None;
    settled.capture_revoked = true;
    settled.cleanup_verified = true;
    match substitution {
        "foreign_id" => settled.preparation_id = [9; 16],
        "pending_call" => settled.pending_sequence = Some(2),
        "active" => settled.phase = PreparationPhase::Active,
        "capture_live" => settled.capture_revoked = false,
        "cleanup_unknown" => settled.cleanup_verified = false,
        _ => unreachable!(),
    }
    first.record("settled", &settled).unwrap();
    assert_eq!(
        super::journal::Journal::prepare(&fixture.spec(2))
            .err()
            .unwrap()
            .reason,
        PreparationFailure::GateBusy
    );
}

#[rstest]
fn prior_epoch_index_cannot_nominate_an_outside_directory() {
    let fixture = FileFixture::new();
    let outside = FileFixture::new();
    std::fs::write(fixture.index(), serde_json::to_vec(&outside.root).unwrap()).unwrap();
    assert_eq!(
        super::journal::Journal::prepare(&fixture.spec(1))
            .err()
            .unwrap()
            .reason,
        PreparationFailure::GateBusy
    );
}

#[rstest]
fn journal_record_names_cannot_escape_the_epoch() {
    let fixture = FileFixture::new();
    let journal = super::journal::Journal::prepare(&fixture.spec(1)).unwrap();
    assert_eq!(
        journal.record("../foreign", &true).unwrap_err().reason,
        PreparationFailure::JournalFailed
    );
    assert!(!fixture.root.join("foreign.json").exists());
}

#[cfg(windows)]
#[rstest]
fn promotion_fence_honors_known_cancellation_and_deadline_without_blocking_restoration() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let revoked = AtomicBool::new(false);
    assert!(super::native::require_promotion_permission(Some((&revoked, 100)), 99).is_ok());
    assert_eq!(
        super::native::require_promotion_permission(Some((&revoked, 100)), 100)
            .unwrap_err()
            .reason,
        PreparationFailure::Expired
    );
    revoked.store(true, Ordering::Release);
    assert_eq!(
        super::native::require_promotion_permission(Some((&revoked, 100)), 99)
            .unwrap_err()
            .reason,
        PreparationFailure::Stopped
    );
    assert!(super::native::require_promotion_permission(None, 101).is_ok());
}

#[cfg(windows)]
#[rstest]
#[case("matching", true)]
#[case("reused", false)]
#[case("moved", false)]
#[case("hidden", false)]
fn passive_pixels_must_prove_the_sealed_native_instance_and_geometry(
    #[case] mutation: &str,
    #[case] expected: bool,
) {
    let state = window(false);
    let mut evidence = crate::ExactWindowPixelEvidence {
        process_id: 1,
        window_handle: 2,
        bounds: state.bounds,
        visible_bounds: state.visible_bounds,
        dpi: state.dpi,
        visible: true,
        minimized: false,
        unobscured: true,
        visibility_failure: None,
        instance: crate::ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 3,
            window_thread_id: 4,
            window_class_hash: 5,
            owner_window_handle: 0,
        },
    };
    match mutation {
        "matching" => {}
        "reused" => evidence.instance.process_creation_time_100ns += 1,
        "moved" => evidence.bounds[0] += 1,
        "hidden" => evidence.visible = false,
        _ => unreachable!(),
    }
    assert_eq!(
        super::process::evidence_matches(&state, &evidence),
        expected
    );
}
