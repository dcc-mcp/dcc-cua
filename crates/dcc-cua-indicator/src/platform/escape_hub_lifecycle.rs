#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscapeHubAcquireAction {
    Start,
    Reuse,
    Restart,
}

pub(crate) const fn acquire_action(hub_present: bool, hub_active: bool) -> EscapeHubAcquireAction {
    match (hub_present, hub_active) {
        (false, _) => EscapeHubAcquireAction::Start,
        (true, true) => EscapeHubAcquireAction::Reuse,
        (true, false) => EscapeHubAcquireAction::Restart,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscapeHubReleaseAction {
    KeepRunning,
    Stop,
}

pub(crate) const fn release_action(active_leases: usize) -> EscapeHubReleaseAction {
    if active_leases <= 1 {
        EscapeHubReleaseAction::Stop
    } else {
        EscapeHubReleaseAction::KeepRunning
    }
}
