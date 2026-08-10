use std::collections::VecDeque;

use dcc_cua_core::{
    ComputerUseInputReadiness, ComputerUseInputResumeRequirements, ComputerUseInputStatus,
    ComputerUseInputTarget, ComputerUseSessionEvent, ComputerUseSessionEventEnvelope,
    ComputerUseSessionEventKind, ComputerUseSessionEventsPage, ComputerUseSessionInputState,
    ComputerUseSessionTargetEvent, ComputerUseSessionTargetEventKind,
    ComputerUseSessionTargetState, ComputerUseTargetAvailability,
    ComputerUseTargetRecoveryRequirements, ComputerUseTargetStatus,
};

pub(super) fn observed_at_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

pub(super) fn input_readiness_sample() -> (ComputerUseInputReadiness, u64) {
    (dcc_cua_core::probe_input_readiness(), observed_at_millis())
}

pub(super) fn refresh_target_availability(
    queue: &mut SessionInputEventQueue,
    availability: ComputerUseTargetAvailability,
) -> bool {
    queue
        .observe_target(availability, observed_at_millis())
        .is_some()
}

pub(super) struct SessionEventQueue {
    input_current: ComputerUseSessionInputState,
    target_current: ComputerUseSessionTargetState,
    events: VecDeque<ComputerUseSessionEventEnvelope>,
    latest_sequence: u64,
    restore_activate_available: bool,
    capacity: usize,
}

impl SessionEventQueue {
    #[cfg(test)]
    pub(super) fn new(
        target: ComputerUseInputTarget,
        readiness: ComputerUseInputReadiness,
        target_availability: ComputerUseTargetAvailability,
        observed_at: u64,
    ) -> Self {
        Self::new_with_restore_capability(target, readiness, target_availability, true, observed_at)
    }

    pub(super) fn new_with_restore_capability(
        target: ComputerUseInputTarget,
        readiness: ComputerUseInputReadiness,
        target_availability: ComputerUseTargetAvailability,
        restore_activate_available: bool,
        observed_at: u64,
    ) -> Self {
        Self::with_capacity(
            target,
            readiness,
            target_availability,
            restore_activate_available,
            observed_at,
            crate::MAX_SESSION_INPUT_EVENTS,
        )
    }

    fn with_capacity(
        target: ComputerUseInputTarget,
        readiness: ComputerUseInputReadiness,
        target_availability: ComputerUseTargetAvailability,
        restore_activate_available: bool,
        observed_at: u64,
        capacity: usize,
    ) -> Self {
        Self {
            input_current: ComputerUseSessionInputState::initial(
                target.clone(),
                readiness,
                observed_at,
            ),
            target_current: ComputerUseSessionTargetState::initial(
                target,
                target_availability,
                observed_at,
            ),
            events: VecDeque::new(),
            latest_sequence: 1,
            restore_activate_available,
            capacity: capacity.max(1),
        }
    }

    pub(super) fn current(&self) -> &ComputerUseSessionInputState {
        &self.input_current
    }

    pub(super) fn target_state(&self) -> &ComputerUseSessionTargetState {
        &self.target_current
    }

    pub(super) const fn latest_sequence(&self) -> u64 {
        self.latest_sequence
    }

    pub(super) fn observe(
        &mut self,
        readiness: ComputerUseInputReadiness,
        observed_at: u64,
    ) -> Option<ComputerUseSessionEvent> {
        if self.input_current.status == readiness.status {
            self.input_current.code = readiness.code;
            self.input_current.reason = readiness.reason;
            self.input_current.observed_at = observed_at;
            return None;
        }
        let previous = self.input_current.clone();
        let sequence = self.latest_sequence.saturating_add(1);
        let current = ComputerUseSessionInputState {
            status: readiness.status,
            code: readiness.code,
            reason: readiness.reason,
            observed_at,
            sequence,
            target: previous.target.clone(),
        };
        let event = ComputerUseSessionEvent {
            kind: if current.status == ComputerUseInputStatus::Ready {
                ComputerUseSessionEventKind::InputResumed
            } else {
                ComputerUseSessionEventKind::InputSuspended
            },
            sequence: current.sequence,
            previous,
            current: current.clone(),
            resume_requirements: (current.status == ComputerUseInputStatus::Ready)
                .then(ComputerUseInputResumeRequirements::safe_continuation),
        };
        self.input_current = current;
        self.latest_sequence = sequence;
        self.events.push_back(event.clone().into());
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
        Some(event)
    }

    pub(super) fn observe_target(
        &mut self,
        availability: ComputerUseTargetAvailability,
        observed_at: u64,
    ) -> Option<ComputerUseSessionEventEnvelope> {
        if self.target_current.status == availability.status {
            self.target_current.code = availability.code;
            self.target_current.visible = availability.visible;
            self.target_current.minimized = availability.minimized;
            self.target_current.foreground = availability.foreground;
            self.target_current.observed_at = observed_at;
            return None;
        }
        let previous = self.target_current.clone();
        let sequence = self.latest_sequence.saturating_add(1);
        let current = ComputerUseSessionTargetState {
            status: availability.status,
            code: availability.code,
            visible: availability.visible,
            minimized: availability.minimized,
            foreground: availability.foreground,
            observed_at,
            sequence,
            target: previous.target.clone(),
        };
        let kind = match (previous.status, current.status) {
            (_, ComputerUseTargetStatus::Minimized) => {
                ComputerUseSessionTargetEventKind::TargetMinimized
            }
            (ComputerUseTargetStatus::Minimized, ComputerUseTargetStatus::Available) => {
                ComputerUseSessionTargetEventKind::TargetRestored
            }
            (_, ComputerUseTargetStatus::Available) => {
                ComputerUseSessionTargetEventKind::TargetAvailable
            }
            (_, ComputerUseTargetStatus::Unavailable) => {
                ComputerUseSessionTargetEventKind::TargetUnavailable
            }
        };
        let event = ComputerUseSessionTargetEvent {
            kind,
            sequence,
            previous,
            current: current.clone(),
            recovery_requirements: (current.status == ComputerUseTargetStatus::Minimized
                && self.restore_activate_available)
                .then(ComputerUseTargetRecoveryRequirements::explicit_restore_activate),
            continuation_requirements: (current.status == ComputerUseTargetStatus::Available)
                .then(ComputerUseInputResumeRequirements::safe_continuation),
        };
        self.target_current = current;
        self.latest_sequence = sequence;
        let event = ComputerUseSessionEventEnvelope::Target(event);
        self.events.push_back(event.clone());
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
        Some(event)
    }

    pub(super) fn events_after(&self, sequence: u64) -> Vec<ComputerUseSessionEventEnvelope> {
        self.events
            .iter()
            .filter(|event| event.sequence() > sequence)
            .cloned()
            .collect()
    }

    pub(super) fn page_after(
        &self,
        after_sequence: u64,
        timed_out: bool,
    ) -> ComputerUseSessionEventsPage {
        let oldest_available_sequence = self.events.front().map(|event| event.sequence());
        let resync_required = after_sequence > self.latest_sequence
            || oldest_available_sequence
                .is_some_and(|oldest| after_sequence.saturating_add(1) < oldest);
        ComputerUseSessionEventsPage {
            after_sequence,
            latest_sequence: self.latest_sequence,
            oldest_available_sequence,
            resync_required,
            timed_out: timed_out && !resync_required,
            current_state: self.input_current.clone(),
            current_target_state: self.target_current.clone(),
            events: self.events_after(after_sequence),
        }
    }
}

pub(super) type SessionInputEventQueue = SessionEventQueue;

#[cfg(test)]
mod tests {
    use super::*;

    fn readiness(status: ComputerUseInputStatus, code: &str) -> ComputerUseInputReadiness {
        ComputerUseInputReadiness {
            status,
            code: code.into(),
            reason: (status == ComputerUseInputStatus::Suspended).then(|| "locked".into()),
        }
    }

    #[test]
    fn ready_to_suspended_enqueues_one_typed_session_event() {
        let target = ComputerUseInputTarget {
            session_id: "session-1".into(),
            process_id: 42,
            window_handle: 77,
        };
        let mut monitor = SessionInputEventQueue::new(
            target.clone(),
            readiness(ComputerUseInputStatus::Ready, "interactive_desktop_ready"),
            target_availability(ComputerUseTargetStatus::Available),
            100,
        );

        let event = monitor
            .observe(
                readiness(
                    ComputerUseInputStatus::Suspended,
                    "interactive_desktop_unavailable",
                ),
                200,
            )
            .expect("transition event");

        assert_eq!(event.kind, ComputerUseSessionEventKind::InputSuspended);
        assert_eq!(event.sequence, 2);
        assert_eq!(event.previous.status, ComputerUseInputStatus::Ready);
        assert_eq!(event.current.status, ComputerUseInputStatus::Suspended);
        assert_eq!(event.current.target, target);
        assert_eq!(monitor.current().sequence, 2);
        assert_eq!(monitor.events_after(1), vec![event.into()]);
    }

    #[test]
    fn suspended_to_ready_enqueues_input_resumed_with_monotonic_sequence() {
        let mut monitor = SessionInputEventQueue::new(
            ComputerUseInputTarget {
                session_id: "session-1".into(),
                process_id: 42,
                window_handle: 77,
            },
            readiness(ComputerUseInputStatus::Ready, "interactive_desktop_ready"),
            target_availability(ComputerUseTargetStatus::Available),
            100,
        );
        monitor.observe(
            readiness(
                ComputerUseInputStatus::Suspended,
                "interactive_desktop_unavailable",
            ),
            200,
        );

        let resumed = monitor
            .observe(
                readiness(ComputerUseInputStatus::Ready, "interactive_desktop_ready"),
                300,
            )
            .expect("resume event");

        assert_eq!(resumed.kind, ComputerUseSessionEventKind::InputResumed);
        assert_eq!(resumed.sequence, 3);
        assert_eq!(resumed.previous.status, ComputerUseInputStatus::Suspended);
        assert_eq!(resumed.current.status, ComputerUseInputStatus::Ready);
        let requirements = resumed
            .resume_requirements
            .as_ref()
            .expect("resumed events carry safe continuation requirements");
        assert!(!requirements.automatic_input);
        assert!(requirements.exact_target_revalidation);
        assert!(requirements.fresh_observation);
        assert!(requirements.foreground_or_explicit_activation);
        assert!(requirements.upstream_session_refresh_may_be_required);
        assert_eq!(monitor.events_after(2), vec![resumed.into()]);
    }

    #[test]
    fn repeated_readiness_is_deduplicated_without_advancing_sequence() {
        let mut monitor = SessionInputEventQueue::new(
            ComputerUseInputTarget {
                session_id: "session-1".into(),
                process_id: 42,
                window_handle: 77,
            },
            readiness(ComputerUseInputStatus::Ready, "interactive_desktop_ready"),
            target_availability(ComputerUseTargetStatus::Available),
            100,
        );

        let duplicate = monitor.observe(
            ComputerUseInputReadiness {
                status: ComputerUseInputStatus::Ready,
                code: "interactive_desktop_platform_managed".into(),
                reason: Some("platform readiness was sampled again".into()),
            },
            999,
        );

        assert!(duplicate.is_none());
        assert_eq!(monitor.current().sequence, 1);
        assert_eq!(monitor.current().observed_at, 999);
        assert_eq!(
            monitor.current().code,
            "interactive_desktop_platform_managed"
        );
        assert_eq!(
            monitor.current().reason.as_deref(),
            Some("platform readiness was sampled again")
        );
        assert!(monitor.events_after(0).is_empty());
    }

    #[test]
    fn bounded_queue_reports_resync_when_subscriber_falls_behind() {
        let mut monitor = SessionInputEventQueue::with_capacity(
            ComputerUseInputTarget {
                session_id: "session-1".into(),
                process_id: 42,
                window_handle: 77,
            },
            readiness(ComputerUseInputStatus::Ready, "ready"),
            target_availability(ComputerUseTargetStatus::Available),
            true,
            100,
            2,
        );
        for (index, status) in [
            ComputerUseInputStatus::Suspended,
            ComputerUseInputStatus::Ready,
            ComputerUseInputStatus::Suspended,
            ComputerUseInputStatus::Ready,
        ]
        .into_iter()
        .enumerate()
        {
            monitor.observe(readiness(status, "transition"), 200 + index as u64);
        }

        let page = monitor.page_after(1, false);

        assert!(page.resync_required);
        assert_eq!(page.oldest_available_sequence, Some(4));
        assert_eq!(page.latest_sequence, 5);
        assert_eq!(
            page.events
                .iter()
                .map(ComputerUseSessionEventEnvelope::sequence)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert_eq!(page.current_state.sequence, 5);
    }

    #[test]
    fn future_cursor_immediately_resyncs_to_this_sessions_current_state() {
        let monitor = SessionInputEventQueue::new(
            ComputerUseInputTarget {
                session_id: "session-a".into(),
                process_id: 42,
                window_handle: 77,
            },
            readiness(ComputerUseInputStatus::Ready, "ready"),
            target_availability(ComputerUseTargetStatus::Available),
            100,
        );

        let page = monitor.page_after(99, false);

        assert!(page.resync_required);
        assert!(!page.timed_out);
        assert_eq!(page.after_sequence, 99);
        assert_eq!(page.latest_sequence, 1);
        assert_eq!(page.current_state.target.session_id, "session-a");
        assert_eq!(page.current_target_state.target.session_id, "session-a");
        assert!(page.events.is_empty());
    }

    #[test]
    fn input_events_are_isolated_by_exact_host_session() {
        let mut first = SessionInputEventQueue::new(
            ComputerUseInputTarget {
                session_id: "session-a".into(),
                process_id: 42,
                window_handle: 77,
            },
            readiness(ComputerUseInputStatus::Ready, "ready"),
            target_availability(ComputerUseTargetStatus::Available),
            100,
        );
        let second = SessionInputEventQueue::new(
            ComputerUseInputTarget {
                session_id: "session-b".into(),
                process_id: 43,
                window_handle: 78,
            },
            readiness(ComputerUseInputStatus::Ready, "ready"),
            target_availability(ComputerUseTargetStatus::Available),
            100,
        );

        first.observe(readiness(ComputerUseInputStatus::Suspended, "locked"), 200);

        assert_eq!(first.events_after(1).len(), 1);
        assert_eq!(first.current().target.session_id, "session-a");
        assert!(second.events_after(1).is_empty());
        assert_eq!(second.current().sequence, 1);
        assert_eq!(second.current().target.session_id, "session-b");
    }

    #[test]
    fn minimized_then_restored_uses_the_same_bounded_sequence_and_never_auto_inputs() {
        let target = ComputerUseInputTarget {
            session_id: "session-1".into(),
            process_id: 42,
            window_handle: 77,
        };
        let mut events = SessionEventQueue::with_capacity(
            target,
            readiness(ComputerUseInputStatus::Ready, "ready"),
            target_availability(ComputerUseTargetStatus::Available),
            true,
            100,
            4,
        );

        let minimized = events
            .observe_target(target_availability(ComputerUseTargetStatus::Minimized), 200)
            .expect("target minimized event");
        let duplicate =
            events.observe_target(target_availability(ComputerUseTargetStatus::Minimized), 250);
        let restored = events
            .observe_target(target_availability(ComputerUseTargetStatus::Available), 300)
            .expect("target restored event");
        let input_sequence_before = events.current().sequence;
        let input_suspended = events
            .observe(readiness(ComputerUseInputStatus::Suspended, "locked"), 400)
            .expect("input transition after target transitions");

        assert_eq!(minimized.sequence(), 2);
        assert!(duplicate.is_none());
        assert_eq!(restored.sequence(), 3);
        assert_eq!(input_suspended.sequence, 4);
        let ComputerUseSessionEventEnvelope::Target(minimized) = minimized else {
            panic!("expected target event")
        };
        assert_eq!(
            minimized.kind,
            ComputerUseSessionTargetEventKind::TargetMinimized
        );
        let recovery = minimized
            .recovery_requirements
            .as_ref()
            .expect("minimized target advertises explicit recovery");
        assert!(!recovery.automatic_input);
        assert!(!recovery.blind_retry);
        let ComputerUseSessionEventEnvelope::Target(restored) = restored else {
            panic!("expected target event")
        };
        assert_eq!(
            restored.kind,
            ComputerUseSessionTargetEventKind::TargetRestored
        );
        assert!(restored.recovery_requirements.is_none());
        let continuation = restored
            .continuation_requirements
            .as_ref()
            .expect("restored target advertises safe continuation fences");
        assert!(!continuation.automatic_input);
        assert!(continuation.exact_target_revalidation);
        assert!(continuation.fresh_observation);
        assert_eq!(input_sequence_before, 1, "input state is orthogonal");
        assert_eq!(events.current().sequence, 4);
        assert_eq!(events.target_state().sequence, 3);
        assert_eq!(events.page_after(1, false).events.len(), 3);
    }

    #[test]
    fn non_exact_grant_does_not_advertise_the_exact_restore_operation() {
        let mut events = SessionEventQueue::new_with_restore_capability(
            ComputerUseInputTarget {
                session_id: "session-title-only".into(),
                process_id: 42,
                window_handle: 77,
            },
            readiness(ComputerUseInputStatus::Ready, "ready"),
            target_availability(ComputerUseTargetStatus::Available),
            false,
            100,
        );

        let event = events
            .observe_target(target_availability(ComputerUseTargetStatus::Minimized), 200)
            .expect("target minimized event");
        let ComputerUseSessionEventEnvelope::Target(event) = event else {
            panic!("expected target event")
        };

        assert!(event.recovery_requirements.is_none());
    }

    fn target_availability(status: ComputerUseTargetStatus) -> ComputerUseTargetAvailability {
        ComputerUseTargetAvailability {
            status,
            code: match status {
                ComputerUseTargetStatus::Available => "target_available",
                ComputerUseTargetStatus::Minimized => "target_minimized",
                ComputerUseTargetStatus::Unavailable => "target_unavailable",
            }
            .into(),
            visible: status == ComputerUseTargetStatus::Available,
            minimized: status == ComputerUseTargetStatus::Minimized,
            foreground: status == ComputerUseTargetStatus::Available,
        }
    }
}
