//! Executed pure controls for the same contract called by controlled native CI.
use super::{Association, Contract, Identity, Phase, Window};
use rstest::rstest;

#[derive(Clone)]
struct Snapshot {
    windows: Vec<Window>,
    associations: Vec<Association>,
}

fn window(hwnd: u64, thread: u32, class: &str, title: &str, visible: bool) -> Window {
    Window {
        identity: Identity {
            hwnd,
            pid: 42,
            thread,
            class: class.into(),
            title: title.into(),
        },
        visible,
    }
}

fn snapshot(owners: Vec<Window>, companions: Vec<Window>) -> Snapshot {
    let associations = owners
        .iter()
        .map(|owner| Association {
            owner: owner.identity.clone(),
            companion: Some(
                companions
                    .iter()
                    .find(|ime| ime.identity.thread == owner.identity.thread)
                    .unwrap()
                    .clone(),
            ),
        })
        .collect();
    Snapshot {
        windows: [owners, companions].concat(),
        associations,
    }
}

fn scene() -> (Contract, [Snapshot; 3]) {
    let fixtures = [
        window(1, 10, "Fixture", "target", true),
        window(2, 10, "Fixture", "decoy", true),
        window(3, 10, "Fixture", "blocker", false),
    ];
    let cursor = window(
        4,
        20,
        "Cua.AgentCursorOverlay",
        "Cua.AgentCursorOverlay.token",
        true,
    );
    let banner = window(5, 30, "DccCuaControlBanner", "label", true);
    let frame = window(6, 30, "DccCuaControlFrame", "", false);
    let imes = [
        window(11, 10, "IME", "Default IME", false),
        window(12, 20, "IME", "Default IME", false),
        window(13, 30, "IME", "Default IME", false),
    ];
    let driver = [fixtures.to_vec(), vec![cursor]].concat();
    let active = [driver.clone(), vec![banner, frame]].concat();
    (
        Contract::new(fixtures.clone(), "token"),
        [
            snapshot(fixtures.to_vec(), imes[..1].to_vec()),
            snapshot(driver, imes[..2].to_vec()),
            snapshot(active, imes.to_vec()),
        ],
    )
}

fn check(
    contract: &mut Contract,
    data: &Snapshot,
    phase: Phase<'_>,
    blocked: bool,
) -> Result<(), &'static str> {
    contract.check_associated(&data.windows, &data.associations, phase, blocked)
}

#[rstest]
fn pixels_only_ime_companions_follow_all_three_owner_lifetimes() {
    let (mut contract, [fixture, driver, active]) = scene();
    check(&mut contract, &fixture, Phase::Fixture, false).unwrap();
    check(&mut contract, &driver, Phase::Driver, false).unwrap();
    check(&mut contract, &active, Phase::Active("label"), false).unwrap();
    check(&mut contract, &driver, Phase::Stopped, false).unwrap();

    // Restart creates a new presenter thread, roots and companion; never retain
    // the first presenter's IME while its original owner has ended.
    let mut restarted = active.clone();
    for item in &mut restarted.windows {
        if item.identity.thread == 30 {
            item.identity.thread = 31;
            item.identity.hwnd += 100;
        }
        if item.identity.hwnd == 3 {
            item.visible = true;
        }
    }
    for association in &mut restarted.associations {
        if association.owner.thread == 30 {
            association.owner.thread = 31;
            association.owner.hwnd += 100;
            let ime = association.companion.as_mut().unwrap();
            ime.identity.thread = 31;
            ime.identity.hwnd += 100;
        }
    }
    check(&mut contract, &restarted, Phase::Active("label"), true).unwrap();
    let mut blocked_driver = driver.clone();
    blocked_driver
        .windows
        .iter_mut()
        .find(|w| w.identity.hwnd == 3)
        .unwrap()
        .visible = true;
    check(&mut contract, &blocked_driver, Phase::Stopped, true).unwrap();
    check(&mut contract, &driver, Phase::Stopped, false).unwrap();
}

#[rstest]
fn pixels_only_ime_mutations_fail_closed_for_each_owner_lifetime() {
    let (mut contract, stages) = scene();
    for (index, phase) in [Phase::Fixture, Phase::Driver, Phase::Active("label")]
        .into_iter()
        .enumerate()
    {
        let stage = &stages[index];
        check(&mut contract, stage, phase, false).unwrap();
        let ime_handle = 11 + index as u64;
        let mut mutations = Vec::new();
        for field in 0..7 {
            let mut changed = stage.clone();
            let ime = changed
                .windows
                .iter_mut()
                .find(|w| w.identity.hwnd == ime_handle)
                .unwrap();
            match field {
                0 => ime.identity.hwnd += 1000,
                1 => ime.identity.pid += 1,
                2 => ime.identity.thread += 1,
                3 => ime.identity.class.push('x'),
                4 => ime.identity.title.push('x'),
                5 => ime.visible = true,
                _ => ime.identity.hwnd = 0,
            }
            let replacement = ime.clone();
            for a in &mut changed.associations {
                if a.companion.as_ref().unwrap().identity.hwnd == ime_handle {
                    a.companion = Some(replacement.clone());
                }
            }
            mutations.push(changed);
        }
        let association_index = stage
            .associations
            .iter()
            .position(|a| a.companion.as_ref().unwrap().identity.hwnd == ime_handle)
            .unwrap();
        let mut null = stage.clone();
        null.associations[association_index].companion = None;
        mutations.push(null);
        let mut missing = stage.clone();
        missing.associations.remove(association_index);
        mutations.push(missing);
        let mut duplicate = stage.clone();
        duplicate
            .associations
            .push(duplicate.associations[association_index].clone());
        mutations.push(duplicate);
        let mut wrong_owner = stage.clone();
        wrong_owner.associations[association_index].owner.thread += 1;
        mutations.push(wrong_owner);
        let mut wrong_association = stage.clone();
        wrong_association.associations[association_index].companion =
            Some(stage.windows[0].clone());
        mutations.push(wrong_association);
        let mut duplicate_window = stage.clone();
        duplicate_window.windows.push(
            stage
                .windows
                .iter()
                .find(|w| w.identity.hwnd == ime_handle)
                .unwrap()
                .clone(),
        );
        mutations.push(duplicate_window);
        let mut unknown = stage.clone();
        unknown
            .windows
            .push(window(99, 10, "IME", "Default IME", false));
        mutations.push(unknown);
        for changed in mutations {
            assert!(
                check(&mut contract.clone(), &changed, phase, false).is_err(),
                "lifetime={index}"
            );
        }
    }
}

#[rstest]
fn pixels_only_ime_stop_rejects_retained_or_first_seen_companions() {
    let (mut contract, [fixture, driver, active]) = scene();
    check(&mut contract, &fixture, Phase::Fixture, false).unwrap();
    check(&mut contract, &driver, Phase::Driver, false).unwrap();
    check(&mut contract, &active, Phase::Active("label"), false).unwrap();
    for resource in active.windows.iter().filter(|w| w.identity.thread == 30) {
        let mut retained = driver.clone();
        retained.windows.push(resource.clone());
        assert!(check(&mut contract.clone(), &retained, Phase::Stopped, false).is_err());
    }
    for owner_handle in [1, 4] {
        let mut missing_owner = driver.clone();
        missing_owner
            .windows
            .retain(|w| w.identity.hwnd != owner_handle);
        assert!(check(&mut contract.clone(), &missing_owner, Phase::Stopped, false).is_err());
    }
    let (mut fresh, _) = scene();
    assert!(check(&mut fresh, &fixture, Phase::Stopped, false).is_err());
    check(&mut fresh, &fixture, Phase::Fixture, false).unwrap();
    assert!(check(&mut fresh, &driver, Phase::Stopped, false).is_err());
}
