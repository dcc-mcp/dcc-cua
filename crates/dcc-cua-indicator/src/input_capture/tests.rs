use super::*;
use rstest::rstest;
use std::sync::{Barrier, mpsc};
use std::time::{Duration, Instant};

struct Resources(Arc<AtomicBool>);
impl Drop for Resources {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[rstest]
#[case(1600)]
#[case(2400)]
fn recording_borrows_live_input_guards_through_long_movement(#[case] duration_ms: u64) {
    let pool = Arc::new(Pool::default());
    let protected = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let input_pool = Arc::clone(&pool);
    let input_protected = Arc::clone(&protected);
    let input = std::thread::spawn(move || {
        let _owner = input_pool.publish(Resources(input_protected)).unwrap();
        ready_tx.send(()).unwrap();
        stop_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // No acquisition of the input's exclusive gate is needed by its own recorder.
    let capture = pool.borrow().unwrap().expect("live input capture lease");
    assert!(capture.resources().0.load(Ordering::Acquire));
    let started = Instant::now();
    let mut frames = 0;
    while started.elapsed() < Duration::from_millis(1200) {
        assert!(pool.borrow().unwrap().is_some());
        assert!(protected.load(Ordering::Acquire));
        frames += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        frames > 1,
        "recording must advance while input exceeds the old1s timeout"
    );
    std::thread::sleep(Duration::from_millis(duration_ms).saturating_sub(started.elapsed()));
    stop_tx.send(()).unwrap();
    input.join().unwrap();
    assert!(
        pool.borrow().unwrap().is_none(),
        "closed owner forbids borrower chains"
    );
    assert!(
        protected.load(Ordering::Acquire),
        "draining capture still owns all guards"
    );
    drop(capture);
    assert!(!protected.load(Ordering::Acquire));
}

#[rstest]
fn independent_task_cannot_borrow_and_new_input_cannot_replace_draining_capture() {
    let pool = Pool::default();
    let peer = Pool::<Resources>::default();
    let protected = Arc::new(AtomicBool::new(true));
    let owner = pool.publish(Resources(Arc::clone(&protected))).unwrap();
    let capture = pool.borrow().unwrap().unwrap();
    assert!(peer.borrow().unwrap().is_none());
    drop(owner);
    assert!(pool.has_resources().unwrap());
    assert!(
        pool.publish(Resources(Arc::new(AtomicBool::new(true))))
            .is_err()
    );
    assert!(protected.load(Ordering::Acquire));
    drop(capture);
    assert!(!pool.has_resources().unwrap());
}

#[rstest]
fn owner_exit_and_capture_borrow_race_keeps_resources_until_last_borrow() {
    let pool = Arc::new(Pool::default());
    let protected = Arc::new(AtomicBool::new(true));
    let owner = pool.publish(Resources(Arc::clone(&protected))).unwrap();
    let capture = pool.borrow().unwrap().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let other = Arc::clone(&barrier);
    let exit = std::thread::spawn(move || {
        other.wait();
        drop(owner);
    });
    barrier.wait();
    exit.join().unwrap();
    assert!(pool.borrow().unwrap().is_none());
    assert!(protected.load(Ordering::Acquire));
    drop(capture);
    assert!(!protected.load(Ordering::Acquire));
}
