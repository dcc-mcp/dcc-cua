//! Native-associated IME companions, never a class-name/baseline allowance.
#[allow(unused_imports)]
use rstest::rstest;

use super::{Identity, Window};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase<'a> {
    Fixture,
    Driver,
    Active(&'a str),
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lifetime {
    Fixture,
    Driver,
    Presenter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Owner {
    pub lifetime: Lifetime,
    pub window: Window,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Association {
    pub owner: Identity,
    pub companion: Option<Window>,
}

#[derive(Clone, Default)]
pub(super) struct Companions {
    bound: Vec<(Lifetime, Association)>,
}

impl Companions {
    pub fn validate(
        &mut self,
        owners: &[Owner],
        inventory: &[Window],
        associations: &[Association],
        phase: Phase<'_>,
    ) -> Result<(), &'static str> {
        if associations.len() != owners.len() {
            return Err("missing or duplicate IME association");
        }
        for (lifetime, previous) in &self.bound {
            if !owners
                .iter()
                .any(|owner| owner.window.identity == previous.owner)
                && !(*lifetime == Lifetime::Presenter && phase == Phase::Stopped)
            {
                return Err("IME owner missing or replaced before its lifetime ended");
            }
        }
        let had_presenter = self
            .bound
            .iter()
            .any(|(role, _)| *role == Lifetime::Presenter);
        let mut next = Vec::new();
        let mut threads = std::collections::HashMap::<u32, Window>::new();
        for owner in owners {
            let matches = associations
                .iter()
                .filter(|a| a.owner == owner.window.identity)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err("wrong, foreign or duplicate IME owner association");
            }
            let association = matches[0];
            let Some(companion) = &association.companion else {
                return Err("native IME association is NULL");
            };
            let id = &companion.identity;
            if id.hwnd == 0
                || id.pid != owner.window.identity.pid
                || id.thread != owner.window.identity.thread
                || id.class != "IME"
                || id.title.is_empty()
                || companion.visible
                || owners
                    .iter()
                    .any(|root| root.window.identity.hwnd == id.hwnd)
                || inventory.iter().filter(|w| *w == companion).count() != 1
            {
                return Err("IME companion identity, hidden state or native association disagrees");
            }
            if threads
                .insert(id.thread, companion.clone())
                .is_some_and(|old| old != *companion)
            {
                return Err("multiple IME companions for one exact owner thread");
            }
            if let Some((role, previous)) = self
                .bound
                .iter()
                .find(|(_, previous)| previous.owner == association.owner)
            {
                if *role != owner.lifetime || previous != association {
                    return Err("bound IME association/identity changed");
                }
            } else {
                let may_bind = match owner.lifetime {
                    Lifetime::Fixture => phase == Phase::Fixture,
                    Lifetime::Driver => matches!(phase, Phase::Driver | Phase::Active(_)),
                    Lifetime::Presenter => matches!(phase, Phase::Active(_)) && !had_presenter,
                };
                if !may_bind {
                    return Err("new IME owner/companion outside its initial lifetime boundary");
                }
            }
            next.push((owner.lifetime, association.clone()));
        }
        for window in inventory {
            if !owners.iter().any(|owner| owner.window == *window)
                && !associations
                    .iter()
                    .any(|a| a.companion.as_ref() == Some(window))
            {
                return Err("unknown, duplicate or retained session/IME resource");
            }
        }
        self.bound = next;
        Ok(())
    }
}
