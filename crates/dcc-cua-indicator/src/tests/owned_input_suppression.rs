//! Runs the production request/ack/lease protocol without any desktop calls.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use rstest::rstest;

use super::*;

struct Presenter {
    state: Arc<RendererSuppression>,
    visible: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Presenter {
    fn start() -> Self {
        let state = Arc::new(RendererSuppression::default());
        let visible = Arc::new(AtomicBool::new(true));
        let running = Arc::new(AtomicBool::new(true));
        let (renderer, surface, alive) = (state.clone(), visible.clone(), running.clone());
        let thread = thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                match renderer.hide_if_requested(|| {
                    surface.store(false, Ordering::Release);
                    true
                }) {
                    Ok(false) => surface.store(true, Ordering::Release),
                    Ok(true) => {}
                    Err(_) => break,
                }
                thread::yield_now();
            }
            surface.store(false, Ordering::Release);
            renderer.stop();
        });
        Self {
            state,
            visible,
            running,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.state.stop();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.stop();
    }
}

fn eventually(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "renderer did not reach the expected state"
        );
        thread::yield_now();
    }
}

#[rstest]
fn owned_surfaces_hide_before_ack_and_restore_only_after_last_lease() {
    let presenter = Presenter::start();
    let first = presenter.state.acquire(|| true).unwrap();
    assert!(!presenter.visible.load(Ordering::Acquire));
    let second = presenter.state.acquire(|| true).unwrap();
    drop(first);
    assert!(!presenter.visible.load(Ordering::Acquire));
    drop(second);
    eventually(|| presenter.visible.load(Ordering::Acquire));
}

#[rstest]
fn lease_for_one_renderer_never_requests_or_hides_a_peer() {
    let owner = Presenter::start();
    let peer = Presenter::start();
    let lease = owner.state.acquire(|| true).unwrap();
    assert!(!owner.visible.load(Ordering::Acquire));
    assert!(peer.visible.load(Ordering::Acquire));
    assert_eq!(peer.state.requests.lock().unwrap().holders, 0);
    drop(lease);
}

#[rstest]
fn stopped_renderer_cannot_reacquire_or_revive_when_old_lease_drops() {
    let mut presenter = Presenter::start();
    let lease = presenter.state.acquire(|| true).unwrap();
    presenter.stop();
    assert!(presenter.state.acquire(|| true).is_err());
    drop(lease);
    assert!(!presenter.visible.load(Ordering::Acquire));
    assert!(
        presenter
            .state
            .hide_if_requested(|| panic!("stopped renderer must not touch windows"))
            .is_err()
    );
}

#[rstest]
fn invalid_owner_cannot_acquire_even_after_physical_ack() {
    let state = Arc::new(RendererSuppression::default());
    let valid = Arc::new(AtomicBool::new(true));
    let (entered, waiting) = mpsc::channel();
    let (renderer, validity) = (state.clone(), valid.clone());
    let worker = thread::spawn(move || {
        entered.send(()).unwrap();
        renderer.acquire(|| validity.load(Ordering::Acquire))
    });
    waiting.recv().unwrap();
    eventually(|| state.requests.lock().unwrap().holders == 1);
    state
        .hide_if_requested(|| {
            valid.store(false, Ordering::Release);
            true
        })
        .unwrap();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(state.requests.lock().unwrap().holders, 0);
}

#[rstest]
fn missing_ack_times_out_without_retaining_a_hidden_request() {
    let state = Arc::new(RendererSuppression::default());
    let result = state.acquire_until(|| true, Instant::now() + Duration::from_millis(1));
    assert!(result.is_err());
    assert_eq!(state.requests.lock().unwrap().holders, 0);
    assert!(
        !state
            .hide_if_requested(|| panic!("no request remains"))
            .unwrap()
    );
}

#[rstest]
fn failed_physical_hide_never_acknowledges_input() {
    let state = Arc::new(RendererSuppression::default());
    let renderer = state.clone();
    let worker = thread::spawn(move || renderer.acquire(|| true));
    eventually(|| state.requests.lock().unwrap().holders == 1);
    assert!(state.hide_if_requested(|| false).is_err());
    state.stop();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(state.requests.lock().unwrap().holders, 0);
}

#[rstest]
fn stopping_wakes_an_unacknowledged_worker_without_waiting_for_timeout() {
    let state = Arc::new(RendererSuppression::default());
    let renderer = state.clone();
    let worker = thread::spawn(move || renderer.acquire(|| true));
    eventually(|| state.requests.lock().unwrap().holders == 1);
    state.stop();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(state.requests.lock().unwrap().holders, 0);
}

#[rstest]
#[case("wrong_pid")]
#[case("wrong_hwnd")]
#[case("inactive")]
#[case("stopped")]
#[case("generation_changed")]
fn source_rejects_wrong_target_or_dead_owner_before_requesting_hide(#[case] variant: &str) {
    use super::super::PlatformInputSuppressionSource;
    let state = Arc::new(RendererSuppression::default());
    let source = PlatformInputSuppressionSource {
        renderer: state.clone(),
        active: Arc::new(AtomicBool::new(variant != "inactive")),
        stop: Arc::new(AtomicBool::new(variant == "stopped")),
        generation: dcc_cua_interrupt::interrupt_generation()
            .wrapping_add(u64::from(variant == "generation_changed")),
        target: (42, 99),
        input_capture: Arc::default(),
    };
    let pid = if variant == "wrong_pid" { 43 } else { 42 };
    let hwnd = if variant == "wrong_hwnd" { 100 } else { 99 };
    assert!(source.begin(pid, hwnd).is_err());
    assert_eq!(state.requests.lock().unwrap().holders, 0);
}
