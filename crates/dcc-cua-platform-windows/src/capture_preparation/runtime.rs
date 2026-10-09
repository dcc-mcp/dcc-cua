//! Responsive child supervisor: all window reads/writes use one worker.
use super::{journal::Journal, native, state::PreparationState, *};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

/// Monotonic Windows GetTickCount64 uptime milliseconds, not wall-clock UTC.
pub(super) fn ticks() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

enum Job {
    Initialize,
    Mutate {
        sequence: u64,
        kind: PreparationMutationKind,
    },
    Probe,
    Settle(CapturePreparationStatus),
}
enum WorkerReturn {
    Initialized(Result<(Vec<PreparedWindowState>, String), PreparationError>),
    Mutated(PreparationMutationReceipt),
    Probed(Result<Vec<PreparedWindowState>, PreparationError>),
    Settled(Result<CapturePreparationStatus, PreparationError>),
    Panicked,
}

pub(super) struct Supervisor {
    pub state: PreparationState,
    spec: CapturePreparationSpec,
    sender: Sender<Job>,
    receiver: Receiver<WorkerReturn>,
    initialized: bool,
    revoked_before_initialization: bool,
    probing: bool,
    next_probe: Instant,
    expected_settlement: Option<CapturePreparationStatus>,
    settlement_pending: bool,
    promotion_revoked: Arc<AtomicBool>,
    _gate: native::TargetGate,
}

impl Supervisor {
    pub fn start(spec: CapturePreparationSpec, deadline: u64) -> Result<Self, PreparationError> {
        spec.validate()?;
        if ticks() >= deadline {
            return Err(PreparationError::new(PreparationFailure::Expired));
        }
        let gate = native::acquire_gate(&spec.target)?;
        let journal = Journal::prepare(&spec)?;
        let mut state = PreparationState::new(spec.preparation_id, deadline, vec![]);
        state.status.journal_path = journal.directory.clone();
        let (send, worker_receive) = mpsc::channel();
        let (worker_send, receive) = mpsc::channel();
        let worker_spec = spec.clone();
        let worker_journal = journal.clone();
        let promotion_revoked = Arc::new(AtomicBool::new(false));
        let worker_revoked = Arc::clone(&promotion_revoked);
        std::thread::Builder::new()
            .name("dcc-cua-preparation-mutation".into())
            .spawn(move || {
                let mut original = vec![];
                let mut desktop = String::new();
                while let Ok(job) = worker_receive.recv() {
                    let outcome =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                            Job::Initialize => {
                                let result = (|| {
                                    let first = native::read_scope(&worker_spec)?;
                                    let second = native::read_scope(&worker_spec)?;
                                    if !native::restored_matches(&first, &second) {
                                        return Err(PreparationError::new(
                                            PreparationFailure::ReadbackFailed,
                                        ));
                                    }
                                    let root = first
                                        .iter()
                                        .find(|state| state.identity == worker_spec.target)
                                        .ok_or_else(|| {
                                            PreparationError::new(
                                                PreparationFailure::IdentityChanged,
                                            )
                                        })?;
                                    if !root.visible || root.minimized {
                                        return Err(PreparationError::new(
                                            PreparationFailure::TargetUnavailable,
                                        ));
                                    }
                                    native::require_inside_desktop(root.visible_bounds)?;
                                    native::require_inside_desktop(root.bounds)?;
                                    let current_desktop = crate::desktop_state();
                                    if current_desktop.input_desktop_error.is_some() {
                                        return Err(PreparationError::new(
                                            PreparationFailure::DesktopUnavailable,
                                        ));
                                    }
                                    desktop =
                                        current_desktop.input_desktop_name.ok_or_else(|| {
                                            PreparationError::new(
                                                PreparationFailure::DesktopUnavailable,
                                            )
                                        })?;
                                    worker_journal.record("original", &first)?;
                                    worker_journal.record("original-desktop", &desktop)?;
                                    original = first.clone();
                                    Ok((first, desktop.clone()))
                                })();
                                WorkerReturn::Initialized(result)
                            }
                            Job::Mutate { sequence, kind } => {
                                let dispatched = worker_journal.record(
                                    &format!("sequence-{sequence}-dispatch"),
                                    &(sequence, kind, &original),
                                );
                                let mut receipt = if let Err(error) = dispatched {
                                    PreparationMutationReceipt {
                                        sequence,
                                        kind,
                                        returned_at_ms: ticks(),
                                        api_success: false,
                                        os_error: None,
                                        readback: vec![],
                                        failure: Some(error),
                                        native_calls: vec![],
                                    }
                                } else {
                                    native::mutate(
                                        &native::MutationContext {
                                            spec: &worker_spec,
                                            original: &original,
                                            original_desktop: &desktop,
                                            journal: &worker_journal,
                                            sequence,
                                            promotion_revoked: &worker_revoked,
                                            deadline,
                                        },
                                        kind,
                                    )
                                };
                                if let Err(error) = worker_journal
                                    .record(&format!("sequence-{sequence}-returned"), &receipt)
                                {
                                    receipt.failure = Some(error);
                                }
                                WorkerReturn::Mutated(receipt)
                            }
                            Job::Probe => WorkerReturn::Probed(native::read_scope(&worker_spec)),
                            Job::Settle(status) => WorkerReturn::Settled(
                                worker_journal.record("settled", &status).map(|_| status),
                            ),
                        }));
                    let returned = outcome.unwrap_or(WorkerReturn::Panicked);
                    if worker_send.send(returned).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?;
        send.send(Job::Initialize)
            .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?;
        Ok(Self {
            state,
            spec,
            sender: send,
            receiver: receive,
            initialized: false,
            revoked_before_initialization: false,
            probing: false,
            next_probe: Instant::now(),
            expected_settlement: None,
            settlement_pending: false,
            promotion_revoked,
            _gate: gate,
        })
    }

    pub fn revoke(&mut self, reason: PreparationFailure) {
        self.promotion_revoked.store(true, Ordering::Release);
        if !self.initialized {
            self.revoked_before_initialization = true;
        }
        self.state.revoke(reason);
    }

    pub fn tick(&mut self) {
        self.state.expire(ticks());
        if self.state.status.capture_revoked {
            self.promotion_revoked.store(true, Ordering::Release);
        }
        if !self.initialized && self.state.status.capture_revoked {
            self.revoked_before_initialization = true;
        }
        loop {
            match self.receiver.try_recv() {
                Ok(WorkerReturn::Initialized(Ok((original, _)))) => {
                    self.initialized = true;
                    self.state.status.original = original;
                    if self.revoked_before_initialization {
                        self.state.status.pending_sequence = None;
                        self.settle_without_write();
                    } else if self
                        .sender
                        .send(Job::Mutate {
                            sequence: 1,
                            kind: PreparationMutationKind::Promote,
                        })
                        .is_err()
                    {
                        self.state.unknown(PreparationFailure::WorkerLost);
                    }
                }
                Ok(WorkerReturn::Initialized(Err(error))) => {
                    self.initialized = true;
                    self.state.status.failure = Some(error);
                    self.state.status.pending_sequence = None;
                    self.settle_without_write();
                }
                Ok(WorkerReturn::Mutated(receipt)) => {
                    let is_promotion = receipt.kind == PreparationMutationKind::Promote;
                    if is_promotion {
                        let exact = native::geometry_matches(
                            &self.state.status.original,
                            &receipt.readback,
                        ) && receipt
                            .readback
                            .iter()
                            .find(|state| state.identity == self.spec.target)
                            .is_some_and(|state| state.topmost);
                        if !exact {
                            self.state.revoke(PreparationFailure::ReadbackFailed);
                        }
                        self.state.promotion_completed(receipt, ticks());
                    } else {
                        let verified = receipt.api_success
                            && receipt.failure.is_none()
                            && native::restored_matches(
                                &self.state.status.original,
                                &receipt.readback,
                            );
                        self.state.restore_completed(receipt, verified);
                        if self.state.status.cleanup_verified {
                            self.begin_settlement();
                        }
                    }
                }
                Ok(WorkerReturn::Probed(result)) => {
                    self.probing = false;
                    if let Ok(states) = result {
                        if native::geometry_matches(&self.state.status.original, &states) {
                            self.state.status.affected_readback = states;
                            self.dispatch_restore();
                        }
                    }
                }
                Ok(WorkerReturn::Panicked) => self.state.unknown(PreparationFailure::WorkerLost),
                Ok(WorkerReturn::Settled(result)) => {
                    self.settlement_pending = false;
                    match result {
                        Ok(actual) => {
                            if let Some(expected) = &self.expected_settlement {
                                if self.state.persisted_settlement(expected, &actual) {
                                    self.expected_settlement = None;
                                }
                            } else {
                                self.state.unknown(PreparationFailure::JournalFailed);
                            }
                        }
                        Err(_) => self.state.unknown(PreparationFailure::JournalFailed),
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.state.unknown(PreparationFailure::WorkerLost);
                    break;
                }
            }
        }
        if self.state.status.capture_revoked
            && self.initialized
            && !self.state.status.cleanup_verified
        {
            if self.expected_settlement.is_some() {
                if !self.settlement_pending && Instant::now() >= self.next_probe {
                    self.next_probe = Instant::now() + Duration::from_millis(500);
                    self.settlement_pending = self
                        .sender
                        .send(Job::Settle(
                            self.expected_settlement.as_ref().unwrap().clone(),
                        ))
                        .is_ok();
                }
            } else if self.state.status.phase == PreparationPhase::RestorePending {
                self.dispatch_restore();
            } else if self.state.status.pending_sequence.is_none()
                && !self.probing
                && Instant::now() >= self.next_probe
            {
                self.next_probe = Instant::now() + Duration::from_millis(500);
                self.probing = self.sender.send(Job::Probe).is_ok();
            }
        }
    }

    fn dispatch_restore(&mut self) {
        if self.state.dispatch_restore() {
            let sequence = self.state.status.pending_sequence.unwrap();
            if self
                .sender
                .send(Job::Mutate {
                    sequence,
                    kind: PreparationMutationKind::Restore,
                })
                .is_err()
            {
                self.state.unknown(PreparationFailure::WorkerLost);
            }
        }
    }

    fn settle_without_write(&mut self) {
        self.state.status.capture_revoked = true;
        self.state.status.phase = PreparationPhase::Refused;
        self.state.status.cleanup_verified = true;
        self.begin_settlement();
    }

    fn begin_settlement(&mut self) {
        let fixed = self.state.await_settlement();
        self.expected_settlement = Some(fixed.clone());
        self.settlement_pending = self.sender.send(Job::Settle(fixed)).is_ok();
        if !self.settlement_pending {
            self.state.unknown(PreparationFailure::WorkerLost);
        }
    }
}
