//! Pure locator controls; no Win32 window, title or input call runs here.
use super::*;
use rstest::rstest;

fn root(hwnd: u64, class: &str, cookie: usize) -> Root {
    Root {
        identity: Identity {
            hwnd,
            pid: 42,
            thread: 7,
            class: class.into(),
            cookie,
        },
        visible: true,
        size: [820, 520],
    }
}

fn complete() -> Vec<Root> {
    let mut roots = vec![root(10, BANNER, 1), root(11, FRAME, 2), root(12, FRAME, 3)];
    roots.push(Root {
        visible: false,
        size: [1, 1],
        ..root(13, FRAME, 0)
    });
    roots
}

#[rstest]
fn metadata_locator_requires_complete_exact_renderer_and_excludes_auxiliary() {
    let roots = complete();
    let (family, bands) = new_family(42, &[], &roots, 2).unwrap();
    assert_eq!(family, [10, 11, 12]);
    assert_eq!(bands, [11, 12]);
    let mut with_peer = roots.clone();
    let mut peer = complete();
    for root in &mut peer {
        root.identity.hwnd += 10;
        root.identity.thread = 8;
        if root.identity.cookie != 0 {
            root.identity.cookie += 10;
        }
    }
    with_peer.extend(peer);
    assert_eq!(
        new_family(42, &roots, &with_peer, 2).unwrap().0,
        [20, 21, 22]
    );
}

#[rstest]
#[case("missing_band")]
#[case("extra_band")]
#[case("second_banner")]
#[case("foreign_pid")]
#[case("foreign_thread")]
#[case("zero_banner_cookie")]
#[case("duplicate_cookie")]
#[case("visible_auxiliary")]
#[case("non_probe_unbound_surface")]
#[case("extra_auxiliary")]
fn metadata_locator_never_turns_incomplete_or_ambiguous_roots_into_a_family(#[case] variant: &str) {
    let mut roots = complete();
    match variant {
        "missing_band" => {
            roots.remove(1);
        }
        "extra_band" => roots.push(root(14, FRAME, 4)),
        "second_banner" => roots.push(root(14, BANNER, 4)),
        "foreign_pid" => roots[1].identity.pid = 43,
        "foreign_thread" => roots[1].identity.thread = 8,
        "zero_banner_cookie" => roots[0].identity.cookie = 0,
        "duplicate_cookie" => roots[1].identity.cookie = 1,
        "visible_auxiliary" => roots[3].visible = true,
        "non_probe_unbound_surface" => roots[3].size = [820, 520],
        "extra_auxiliary" => roots.push(Root {
            visible: false,
            size: [1, 1],
            ..root(14, FRAME, 0)
        }),
        _ => unreachable!(),
    }
    assert!(new_family(42, &[], &roots, 2).is_err(), "{variant}");
}

#[rstest]
#[case("cookie_drift")]
#[case("handle_reuse")]
#[case("thread_drift")]
#[case("missing_old_frame")]
fn a_known_renderer_cannot_disappear_or_change_instance_during_peer_start(#[case] variant: &str) {
    let before = complete();
    let mut after = before.clone();
    match variant {
        "cookie_drift" => after[1].identity.cookie = 90,
        "handle_reuse" => {
            after[1].identity.cookie = 90;
            after[1].identity.thread = 8;
        }
        "thread_drift" => after[1].identity.thread = 8,
        "missing_old_frame" => {
            after.remove(1);
        }
        _ => unreachable!(),
    }
    assert!(new_family(42, &before, &after, 2).is_err(), "{variant}");
}
