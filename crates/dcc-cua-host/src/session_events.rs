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
mod tests;
