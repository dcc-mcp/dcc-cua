//! In-memory OS boundary; exact production coordinator and lease code are compiled.
use rstest::rstest;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum IndicatorError {
    CaptureExclusion(String),
}
static GATE: AtomicBool = AtomicBool::new(false);
static PRESENTATION: AtomicBool = AtomicBool::new(false);
static CURSOR: AtomicBool = AtomicBool::new(false);
static PEER_VISIBLE: AtomicBool = AtomicBool::new(false);
static ACQUIRES: AtomicUsize = AtomicUsize::new(0);

mod platform {
    use super::*;
    pub struct PlatformInputSuppressionGuard;
    impl Drop for PlatformInputSuppressionGuard {
        fn drop(&mut self) {
            PRESENTATION.store(false, Ordering::Release);
        }
    }
    pub type PlatformInputCursorGuard = capture_exclusion::InputCursorGuard;
    pub mod capture_exclusion {
        use super::*;
        pub const CAPTURE_EXCLUSION_TIMEOUT: Duration = Duration::from_secs(1);
        pub struct CrossProcessGate;
        impl CrossProcessGate {
            pub fn try_acquire() -> std::io::Result<Self> {
                ACQUIRES.fetch_add(1, Ordering::Relaxed);
                GATE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .map(|_| Self)
                    .map_err(|_| std::io::ErrorKind::WouldBlock.into())
            }
        }
        impl Drop for CrossProcessGate {
            fn drop(&mut self) {
                GATE.store(false, Ordering::Release);
            }
        }
        pub struct Guard {
            _gate: CrossProcessGate,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                PRESENTATION.store(false, Ordering::Release);
                CURSOR.store(false, Ordering::Release);
            }
        }
        pub fn begin_with_gate(
            _: &AtomicBool,
            gate: CrossProcessGate,
        ) -> Result<Guard, IndicatorError> {
            PRESENTATION.store(true, Ordering::Release);
            CURSOR.store(true, Ordering::Release);
            Ok(Guard { _gate: gate })
        }
        pub struct InputCursorGuard {
            _gate: CrossProcessGate,
        }
        impl Drop for InputCursorGuard {
            fn drop(&mut self) {
                CURSOR.store(false, Ordering::Release);
            }
        }
        pub fn begin_input_cursor(_: &AtomicBool) -> Result<InputCursorGuard, IndicatorError> {
            let gate = CrossProcessGate::try_acquire()
                .map_err(|_| IndicatorError::CaptureExclusion("busy".into()))?;
            CURSOR.store(true, Ordering::Release);
            Ok(InputCursorGuard { _gate: gate })
        }
        pub fn confirm_overlays_hidden() -> Result<(), IndicatorError> {
            if PRESENTATION.load(Ordering::Acquire)
                && CURSOR.load(Ordering::Acquire)
                && !PEER_VISIBLE.load(Ordering::Acquire)
            {
                return Ok(());
            }
            Err(IndicatorError::CaptureExclusion(
                "visible peer or unprotected owner".into(),
            ))
        }
    }
    // PRODUCTION_COORDINATOR

    pub(super) fn exercise() {
        use self::cooperative_capture::*;
        let pool = Arc::new(Pool::default());
        let active = Arc::new(AtomicBool::new(true));
        PRESENTATION.store(true, Ordering::Release);
        let input = begin_input(&pool, &active, platform::PlatformInputSuppressionGuard).unwrap();
        let before = ACQUIRES.load(Ordering::Acquire);
        std::thread::scope(|scope| {
            // Two consumers, like live readback and recording publication, run
            // longer than the unchanged1s exclusive acquisition deadline.
            for _ in 0..2 {
                let pool = &pool;
                let active = &active;
                scope.spawn(move || {
                    let start = Instant::now();
                    let mut frames = 0;
                    while start.elapsed() < Duration::from_millis(1600) {
                        let _capture = begin_capture(pool, active, true).unwrap();
                        frames += 1;
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert!(frames > 1);
                });
            }
        });
        assert_eq!(
            ACQUIRES.load(Ordering::Acquire),
            before,
            "own capture must not reacquire input's gate"
        );
        let drain = begin_capture(&pool, &active, true).unwrap();
        PEER_VISIBLE.store(true, Ordering::Release);
        assert!(
            begin_capture(&pool, &active, true).is_err(),
            "peer remains visible and is never suppressed"
        );
        PEER_VISIBLE.store(false, Ordering::Release);
        let peer = Arc::new(Pool::default());
        assert!(
            begin_capture(&peer, &active, true).is_err(),
            "another task cannot borrow the epoch"
        );
        drop(input);
        assert!(GATE.load(Ordering::Acquire));
        assert!(PRESENTATION.load(Ordering::Acquire) && CURSOR.load(Ordering::Acquire));
        assert!(pool.borrow().unwrap().is_none());
        drop(drain);
        assert!(!GATE.load(Ordering::Acquire));
        assert!(!PRESENTATION.load(Ordering::Acquire) && !CURSOR.load(Ordering::Acquire));
        let snapshot = begin_capture(&pool, &active, false).unwrap();
        let acquired = ACQUIRES.load(Ordering::Acquire);
        let recording = begin_capture(&pool, &active, true).unwrap();
        let second_snapshot = begin_capture(&pool, &active, false).unwrap();
        assert_eq!(ACQUIRES.load(Ordering::Acquire), acquired);
        drop(snapshot);
        assert!(GATE.load(Ordering::Acquire));
        drop(recording);
        assert!(GATE.load(Ordering::Acquire));
        drop(second_snapshot);
        assert!(!GATE.load(Ordering::Acquire));
    }
}
#[rstest]
fn recording_and_snapshots_share_only_the_owned_long_input_epoch() {
    platform::exercise();
}
