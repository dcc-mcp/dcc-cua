//! Exact owner-resource checks; native reads run only in the disposable CI test.
use rstest::rstest;

mod diagnostics;
mod ime;
mod ime_tests;
mod native;
pub(super) use ime::Phase;
use ime::{Association, Companions, Lifetime, Owner};
pub(super) use native::read;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub hwnd: u64,
    pub pid: u32,
    pub thread: u32,
    pub class: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub identity: Identity,
    pub visible: bool,
}

#[derive(Clone)]
pub(super) struct Contract {
    fixtures: [Window; 3], // target, decoy, hidden/explicitly blocked fixture
    cursor_title: String,
    cursor: Option<Window>,
    companions: Companions,
}

impl Contract {
    pub fn new(fixtures: [Window; 3], renderer_id: &str) -> Self {
        Self {
            fixtures,
            cursor_title: format!("Cua.AgentCursorOverlay.{renderer_id}"),
            cursor: None,
            companions: Companions::default(),
        }
    }

    // This selection grants no allowance: check() first validates only these
    // exact roots, then companion validation must account for EVERY other root.
    fn owners(
        &self,
        windows: &[Window],
        phase: Phase<'_>,
        blocked: bool,
    ) -> Result<Vec<Owner>, &'static str> {
        let label = match phase {
            Phase::Active(label) => Some(label),
            _ => None,
        };
        let mut owners = Vec::new();
        for window in windows {
            let lifetime = if self.fixtures.iter().any(|f| f.identity == window.identity) {
                Some(Lifetime::Fixture)
            } else if window.identity.class == "Cua.AgentCursorOverlay"
                && window.identity.title == self.cursor_title
            {
                Some(Lifetime::Driver)
            } else if label.is_some_and(|label| {
                (window.identity.class == "DccCuaControlBanner" && window.identity.title == label)
                    || (window.identity.class == "DccCuaControlFrame"
                        && window.identity.title.is_empty())
            }) {
                Some(Lifetime::Presenter)
            } else {
                None
            };
            if let Some(lifetime) = lifetime {
                owners.push(Owner {
                    lifetime,
                    window: window.clone(),
                });
            }
        }
        let mut probe = self.clone();
        probe.check(
            &owners.iter().map(|o| o.window.clone()).collect::<Vec<_>>(),
            label,
            matches!(phase, Phase::Driver | Phase::Active(_)),
            blocked,
        )?;
        Ok(owners)
    }

    fn check_associated(
        &mut self,
        windows: &[Window],
        associations: &[Association],
        phase: Phase<'_>,
        blocked: bool,
    ) -> Result<(), &'static str> {
        let owners = self.owners(windows, phase, blocked)?;
        let mut next = self.clone();
        next.companions
            .validate(&owners, windows, associations, phase)?;
        let label = match phase {
            Phase::Active(label) => Some(label),
            _ => None,
        };
        next.check(
            &owners.iter().map(|o| o.window.clone()).collect::<Vec<_>>(),
            label,
            matches!(phase, Phase::Driver | Phase::Active(_)),
            blocked,
        )?;
        *self = next;
        Ok(())
    }

    pub fn check_native(
        &mut self,
        pid: u32,
        stage: &str,
        phase: Phase<'_>,
        blocked: bool,
    ) -> Result<(), &'static str> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let windows = native::read_until(pid, stage, deadline);
        let owners = self.owners(&windows, phase, blocked)?;
        let associations = native::associations(&owners, stage, deadline)?;
        self.check_associated(&windows, &associations, phase, blocked)
    }

    /// A cursor may first be bound only while the configured driver/session is
    /// alive before stop. Stop cannot introduce a new permitted resource.
    pub fn check(
        &mut self,
        windows: &[Window],
        active_label: Option<&str>,
        bind_cursor: bool,
        blocked: bool,
    ) -> Result<(), &'static str> {
        let pid = self.fixtures[0].identity.pid;
        let mut handles = std::collections::HashSet::new();
        for window in windows {
            if window.identity.pid != pid
                || window.identity.thread == 0
                || !handles.insert(window.identity.hwnd)
            {
                return Err("foreign owner, invalid thread or duplicate HWND");
            }
        }
        for (index, expected) in self.fixtures.iter().enumerate() {
            if !windows.iter().any(|actual| {
                actual.identity == expected.identity
                    && actual.visible
                        == if index == 2 {
                            blocked
                        } else {
                            expected.visible
                        }
            }) {
                return Err("fixture target/decoy/blocker missing, replaced or visibility changed");
            }
        }
        let extras = windows
            .iter()
            .filter(|window| {
                !self
                    .fixtures
                    .iter()
                    .any(|fixture| fixture.identity == window.identity)
            })
            .collect::<Vec<_>>();
        let cursors = extras
            .iter()
            .copied()
            .filter(|window| {
                window.identity.class == "Cua.AgentCursorOverlay"
                    && window.identity.title == self.cursor_title
            })
            .collect::<Vec<_>>();
        if cursors.len() > 1 {
            return Err("duplicate configured cursor");
        }
        let candidate = cursors.first().copied();
        match (&self.cursor, candidate) {
            (Some(bound), Some(actual))
                if bound.identity == actual.identity
                    && (active_label.is_some() || bound.visible == actual.visible) => {}
            (Some(_), _) => return Err("bound driver cursor missing, replaced or not restored"),
            (None, Some(actual)) if bind_cursor => {
                if self
                    .fixtures
                    .iter()
                    .any(|f| f.identity.thread == actual.identity.thread)
                {
                    return Err("cursor is not on its renderer thread");
                }
            }
            (None, Some(_)) => return Err("new cursor appeared only after stop"),
            (None, None) => {}
        }
        let mut banner_count = 0;
        let mut frame_count = 0;
        let mut presenter_thread = None;
        for window in extras
            .into_iter()
            .filter(|window| candidate.is_none_or(|cursor| cursor.identity != window.identity))
        {
            let Some(label) = active_label else {
                return Err("unexplained or retained session resource after stop");
            };
            match (
                window.identity.class.as_str(),
                window.identity.title.as_str(),
            ) {
                ("DccCuaControlBanner", title) if title == label => banner_count += 1,
                ("DccCuaControlFrame", "") => frame_count += 1,
                _ => return Err("unexplained active owner resource"),
            }
            let thread = window.identity.thread;
            if self.fixtures.iter().any(|f| f.identity.thread == thread)
                || candidate.is_some_and(|cursor| cursor.identity.thread == thread)
                || presenter_thread.is_some_and(|expected| expected != thread)
            {
                return Err("session presenter thread identity changed");
            }
            presenter_thread = Some(thread);
        }
        if active_label.is_some() && (banner_count != 1 || frame_count == 0) {
            return Err("active session presenter inventory incomplete");
        }
        if self.cursor.is_none() {
            self.cursor = candidate.cloned();
        }
        Ok(())
    }
}

#[rstest]
fn pixels_only_replays_recorded_hidden_ime_baseline() {
    // Original job98862800391: these four exact records occurred at both
    // fixture-ready and before-driver. No native operation runs in this replay.
    let window = |hwnd, title: &str, visible| Window {
        identity: Identity {
            hwnd,
            pid: 868,
            thread: 8508,
            class: "DccCuaPixelsOnlyFixture".into(),
            title: title.into(),
        },
        visible,
    };
    let fixtures = [
        window(262262, "pixels-only custom target", true),
        window(196908, "pixels-only custom decoy", true),
        window(262498, "pixels-only capture blocker", false),
    ];
    let mut ime = window(393328, "Default IME", false);
    ime.identity.class = "IME".into();
    let inventory = [fixtures.to_vec(), vec![ime.clone()]].concat();
    // The old log has no IMM query. This supplied boundary result is a pure
    // positive control; the new controlled CI must prove the real association.
    let associations = fixtures
        .iter()
        .map(|owner| Association {
            owner: owner.identity.clone(),
            companion: Some(ime.clone()),
        })
        .collect::<Vec<_>>();
    let mut contract = Contract::new(fixtures, "replay-only");
    assert!(
        contract
            .check_associated(&inventory, &associations, Phase::Fixture, false)
            .is_ok(),
        "recorded pre-driver IME companion must be modeled, not treated as a session leak"
    );
    assert!(
        contract
            .check_associated(&inventory, &associations, Phase::Driver, false)
            .is_ok()
    );
}

#[rstest]
fn pixels_only_inventory_controls_reject_resource_identity_mutations() {
    let window = |hwnd, thread, class: &str, title: &str, visible| Window {
        identity: Identity {
            hwnd,
            pid: 42,
            thread,
            class: class.into(),
            title: title.into(),
        },
        visible,
    };
    let fixtures = [
        window(1, 10, "Fixture", "target", true),
        window(2, 10, "Fixture", "decoy", true),
        window(3, 10, "Fixture", "blocker", false),
    ];
    let cursor = window(
        4,
        20,
        "Cua.AgentCursorOverlay",
        "Cua.AgentCursorOverlay.opaque-id",
        true,
    );
    let banner = window(5, 30, "DccCuaControlBanner", "session label", true);
    let frame = window(6, 30, "DccCuaControlFrame", "", false);
    let baseline = [fixtures.to_vec(), vec![cursor.clone()]].concat();
    let mut contract = Contract::new(fixtures.clone(), "opaque-id");
    assert!(contract.check(&fixtures, None, false, false).is_ok());
    assert!(contract.check(&baseline, None, true, false).is_ok());
    let active = [baseline.clone(), vec![banner.clone(), frame.clone()]].concat();
    assert!(
        contract
            .check(&active, Some("session label"), true, false)
            .is_ok()
    );
    assert!(contract.check(&baseline, None, false, false).is_ok());

    let mut suppressed_active = active.clone();
    suppressed_active[3].visible = false;
    assert!(
        contract
            .check(&suppressed_active, Some("session label"), false, false)
            .is_ok()
    );
    let mut blocked_active = active.clone();
    blocked_active[2].visible = true;
    assert!(
        contract
            .check(&blocked_active, Some("session label"), false, true)
            .is_ok()
    );
    let mut blocked_stopped = baseline.clone();
    blocked_stopped[2].visible = true;
    assert!(contract.check(&blocked_stopped, None, false, true).is_ok());
    assert!(contract.check(&baseline, None, false, false).is_ok());

    let mut mutations = vec![active]; // A retained banner/frame must not pass stop.
    for extra in [
        window(
            7,
            21,
            "Cua.AgentCursorOverlay",
            "Cua.AgentCursorOverlay.opaque-id.extra",
            true,
        ),
        window(
            7,
            21,
            "Cua.AgentCursorOverlay",
            "Cua.AgentCursorOverlay.opaque-id",
            true,
        ),
        window(7, 21, "Unknown", "extra", false),
    ] {
        mutations.push([baseline.clone(), vec![extra]].concat());
    }
    for index in [0, 1, 3] {
        let mut missing = baseline.clone();
        missing.remove(index);
        mutations.push(missing);
        for field in 0..5 {
            let mut changed = baseline.clone();
            let identity = &mut changed[index].identity;
            match field {
                0 => identity.hwnd += 100,
                1 => identity.pid += 1,
                2 => identity.thread += 1,
                3 => identity.class.push('x'),
                _ => identity.title.push('x'),
            }
            mutations.push(changed);
        }
    }
    let mut blocked = baseline.clone();
    blocked[2].visible = true;
    mutations.push(blocked);
    let mut hidden_cursor = baseline.clone();
    hidden_cursor[3].visible = false;
    mutations.push(hidden_cursor);
    for mutated in mutations {
        assert!(
            contract.check(&mutated, None, false, false).is_err(),
            "{mutated:?}"
        );
    }
    let mut unbound = Contract::new(fixtures, "opaque-id");
    assert!(unbound.check(&baseline, None, false, false).is_err());
}
