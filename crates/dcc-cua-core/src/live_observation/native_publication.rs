//! A bounded handoff to the single native owner, without performing native work.
//!
//! The caller supplies the original deadline. Queued and in-flight work share
//! one slot; cancellation does not release an in-flight owner's capacity until
//! that owner completes or drops its work. No payload destructor or wake callback
//! runs while either internal mutex is held.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativePublicationPhase {
    Prepare,
    Encoded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Metadata {
    pub(crate) phase: NativePublicationPhase,
    pub(crate) stream_id: u64,
    pub(crate) sequence: u64,
    pub(crate) started_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    Full,
    Deadline,
    Stopped,
    Canceled,
    ReplyUnavailable,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Location {
    Queued,
    InFlight,
    Released,
}

#[derive(Clone, Copy)]
enum Outcome {
    Waiting,
    Completed,
    Consumed,
    Failed(Failure),
}

struct RequestState<P, R> {
    payload: Option<P>,
    result: Option<R>,
    location: Location,
    outcome: Outcome,
}

struct Request<P, R> {
    metadata: Metadata,
    deadline: Instant,
    state: Mutex<RequestState<P, R>>,
}

struct Slot<P, R> {
    stopped: bool,
    owner: Option<Arc<Request<P, R>>>,
}

struct Shared<P, R> {
    slot: Mutex<Slot<P, R>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl<P, R> Shared<P, R> {
    fn has_pending(&self) -> bool {
        let slot = lock(&self.slot);
        !slot.stopped
            && slot.owner.as_ref().is_some_and(|owner| {
                let state = lock(&owner.state);
                state.location == Location::Queued && matches!(state.outcome, Outcome::Waiting)
            })
    }
}

pub(super) struct PublicationController<P, R> {
    shared: Arc<Shared<P, R>>,
}

pub(super) struct PublicationInbox<P, R> {
    shared: Arc<Shared<P, R>>,
}

pub(super) struct PendingPublication<P, R> {
    shared: Arc<Shared<P, R>>,
    request: Arc<Request<P, R>>,
    finished: bool,
}

pub(super) struct PublicationWork<P, R> {
    shared: Arc<Shared<P, R>>,
    request: Arc<Request<P, R>>,
    payload: Option<P>,
    finished: bool,
}

// Generic values and the retired owner must outlive both lock guards.
struct Cleanup<P, R> {
    payload: Option<P>,
    result: Option<R>,
    owner: Option<Arc<Request<P, R>>>,
    notify: bool,
}

impl<P, R> Cleanup<P, R> {
    fn empty() -> Self {
        Self {
            payload: None,
            result: None,
            owner: None,
            notify: false,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn live_failure<P, R>(
    slot: &Slot<P, R>,
    request: &Request<P, R>,
    state: &RequestState<P, R>,
    now: Instant,
) -> Option<Failure> {
    // Callers hold both mutexes here: never admit an acknowledgement using a
    // timestamp captured before a lock wait or scheduling delay. Future clock
    // injection remains valid for deterministic expiry tests.
    let now = now.max(Instant::now());
    if let Outcome::Failed(failure) = state.outcome {
        Some(failure)
    } else if slot.stopped {
        Some(Failure::Stopped)
    } else if now >= request.deadline {
        Some(Failure::Deadline)
    } else {
        None
    }
}

fn release_owner<P, R>(
    slot: &mut Slot<P, R>,
    request: &Arc<Request<P, R>>,
) -> Option<Arc<Request<P, R>>> {
    if slot
        .owner
        .as_ref()
        .is_some_and(|owner| Arc::ptr_eq(owner, request))
    {
        slot.owner.take()
    } else {
        None
    }
}

fn close_failed<P, R>(
    slot: &mut Slot<P, R>,
    request: &Arc<Request<P, R>>,
    state: &mut RequestState<P, R>,
    failure: Failure,
    release_in_flight: bool,
) -> Cleanup<P, R> {
    let (failure, changed) = match state.outcome {
        Outcome::Failed(existing) => (existing, false),
        _ => (failure, true),
    };
    state.outcome = Outcome::Failed(failure);
    let mut cleanup = Cleanup {
        payload: state.payload.take(),
        result: state.result.take(),
        owner: None,
        notify: changed,
    };
    if state.location == Location::Queued || release_in_flight {
        state.location = Location::Released;
        cleanup.owner = release_owner(slot, request);
        cleanup.notify |= cleanup.owner.is_some();
    }
    cleanup
}

fn finish_cleanup<P, R>(shared: &Shared<P, R>, cleanup: Cleanup<P, R>) {
    let notify = cleanup.notify;
    // Read the fields explicitly so each owned generic is dropped here, never
    // inside a caller's critical section.
    drop(cleanup.payload);
    drop(cleanup.result);
    drop(cleanup.owner);
    if notify {
        (shared.wake)();
    }
}

pub(super) fn channel<P, R>(
    wake: Arc<dyn Fn() + Send + Sync>,
) -> (PublicationController<P, R>, PublicationInbox<P, R>) {
    let shared = Arc::new(Shared {
        slot: Mutex::new(Slot {
            stopped: false,
            owner: None,
        }),
        wake,
    });
    (
        PublicationController {
            shared: Arc::clone(&shared),
        },
        PublicationInbox { shared },
    )
}

impl<P, R> Clone for PublicationController<P, R> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<P, R> PublicationController<P, R> {
    pub(super) fn submit(
        &self,
        payload: P,
        metadata: Metadata,
        deadline: Instant,
    ) -> Result<PendingPublication<P, R>, Failure> {
        let request = Arc::new(Request {
            metadata,
            deadline,
            state: Mutex::new(RequestState {
                payload: Some(payload),
                result: None,
                location: Location::Queued,
                outcome: Outcome::Waiting,
            }),
        });
        let failure = {
            let mut slot = lock(&self.shared.slot);
            let failure = if slot.stopped {
                Some(Failure::Stopped)
            } else if Instant::now() >= deadline {
                Some(Failure::Deadline)
            } else if slot.owner.is_some() {
                Some(Failure::Full)
            } else {
                None
            };
            if failure.is_none() {
                slot.owner = Some(Arc::clone(&request));
            }
            failure
        };
        if let Some(failure) = failure {
            return Err(failure);
        }
        let pending = PendingPublication {
            shared: Arc::clone(&self.shared),
            request,
            finished: false,
        };
        (self.shared.wake)();
        Ok(pending)
    }
}

impl<P, R> PublicationInbox<P, R> {
    pub(super) fn has_pending(&self) -> bool {
        self.shared.has_pending()
    }

    pub(super) fn take(&self) -> Option<PublicationWork<P, R>> {
        let (work, cleanup) = {
            let mut slot = lock(&self.shared.slot);
            let request = slot.owner.as_ref().map(Arc::clone)?;
            let mut state = lock(&request.state);
            if state.location != Location::Queued {
                return None;
            }
            if let Some(failure) = live_failure(&slot, &request, &state, Instant::now()) {
                let cleanup = close_failed(&mut slot, &request, &mut state, failure, true);
                (None, cleanup)
            } else {
                state.location = Location::InFlight;
                let work = PublicationWork {
                    shared: Arc::clone(&self.shared),
                    request: Arc::clone(&request),
                    payload: state.payload.take(),
                    finished: false,
                };
                (Some(work), Cleanup::empty())
            }
        };
        finish_cleanup(&self.shared, cleanup);
        work
    }

    pub(super) fn stop(&self) {
        let cleanup = {
            let mut slot = lock(&self.shared.slot);
            if slot.stopped {
                return;
            }
            slot.stopped = true;
            if let Some(request) = slot.owner.as_ref().map(Arc::clone) {
                let mut state = lock(&request.state);
                close_failed(&mut slot, &request, &mut state, Failure::Stopped, true)
            } else {
                Cleanup {
                    notify: true,
                    ..Cleanup::empty()
                }
            }
        };
        finish_cleanup(&self.shared, cleanup);
    }
}

impl<P, R> Drop for PublicationInbox<P, R> {
    fn drop(&mut self) {
        self.stop();
    }
}

impl<P, R> PendingPublication<P, R> {
    /// Poll once without waiting. A reply is rejected at or after the original
    /// deadline, even when the owner completed before that deadline.
    pub(super) fn try_result(&mut self, now: Instant) -> Result<Option<R>, Failure> {
        let (result, cleanup) = {
            let mut slot = lock(&self.shared.slot);
            let mut state = lock(&self.request.state);
            if let Some(failure) = live_failure(&slot, &self.request, &state, now) {
                let cleanup = close_failed(&mut slot, &self.request, &mut state, failure, false);
                self.finished = true;
                (Err(failure), cleanup)
            } else {
                match state.outcome {
                    Outcome::Waiting => (Ok(None), Cleanup::empty()),
                    Outcome::Completed => {
                        self.finished = true;
                        if let Some(result) = state.result.take() {
                            state.outcome = Outcome::Consumed;
                            (Ok(Some(result)), Cleanup::empty())
                        } else {
                            let cleanup = close_failed(
                                &mut slot,
                                &self.request,
                                &mut state,
                                Failure::ReplyUnavailable,
                                false,
                            );
                            (Err(Failure::ReplyUnavailable), cleanup)
                        }
                    }
                    Outcome::Consumed => {
                        self.finished = true;
                        (Err(Failure::ReplyUnavailable), Cleanup::empty())
                    }
                    Outcome::Failed(_) => unreachable!("live_failure handles failed requests"),
                }
            }
        };
        finish_cleanup(&self.shared, cleanup);
        result
    }
}

impl<P, R> Drop for PendingPublication<P, R> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let cleanup = {
            let mut slot = lock(&self.shared.slot);
            let mut state = lock(&self.request.state);
            close_failed(
                &mut slot,
                &self.request,
                &mut state,
                Failure::Canceled,
                false,
            )
        };
        finish_cleanup(&self.shared, cleanup);
    }
}

impl<P, R> PublicationWork<P, R> {
    pub(super) fn metadata(&self) -> Metadata {
        self.request.metadata
    }

    pub(super) fn deadline(&self) -> Instant {
        self.request.deadline
    }

    pub(super) fn ensure_live(&self, now: Instant) -> Result<(), Failure> {
        let (result, cleanup) = {
            let mut slot = lock(&self.shared.slot);
            let mut state = lock(&self.request.state);
            let failure = live_failure(&slot, &self.request, &state, now).or_else(|| {
                (!matches!(state.outcome, Outcome::Waiting)).then_some(Failure::ReplyUnavailable)
            });
            if let Some(failure) = failure {
                let cleanup = close_failed(&mut slot, &self.request, &mut state, failure, false);
                (Err(failure), cleanup)
            } else {
                (Ok(()), Cleanup::empty())
            }
        };
        finish_cleanup(&self.shared, cleanup);
        result
    }

    /// Transfer the exact typed payload once. The owner checks liveness before
    /// native validation and again through `complete` before acknowledging.
    pub(super) fn take_payload(&mut self) -> P {
        self.payload
            .take()
            .expect("native publication payload was already consumed")
    }

    pub(super) fn complete(mut self, result: R, now: Instant) -> Result<(), Failure> {
        let mut reply = Some(result);
        let (result, cleanup) = {
            let mut slot = lock(&self.shared.slot);
            let mut state = lock(&self.request.state);
            let failure = live_failure(&slot, &self.request, &state, now).or_else(|| {
                (!matches!(state.outcome, Outcome::Waiting)).then_some(Failure::ReplyUnavailable)
            });
            if let Some(failure) = failure {
                let cleanup = close_failed(&mut slot, &self.request, &mut state, failure, true);
                (Err(failure), cleanup)
            } else {
                state.result = reply.take();
                state.outcome = Outcome::Completed;
                state.location = Location::Released;
                let cleanup = Cleanup {
                    payload: state.payload.take(),
                    result: None,
                    owner: release_owner(&mut slot, &self.request),
                    notify: true,
                };
                (Ok(()), cleanup)
            }
        };
        self.finished = true;
        drop(reply);
        finish_cleanup(&self.shared, cleanup);
        result
    }
}

impl<P, R> Drop for PublicationWork<P, R> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let cleanup = {
            let mut slot = lock(&self.shared.slot);
            let mut state = lock(&self.request.state);
            let failure = live_failure(&slot, &self.request, &state, Instant::now())
                .unwrap_or(Failure::ReplyUnavailable);
            close_failed(&mut slot, &self.request, &mut state, failure, true)
        };
        finish_cleanup(&self.shared, cleanup);
    }
}

#[cfg(test)]
mod tests;
