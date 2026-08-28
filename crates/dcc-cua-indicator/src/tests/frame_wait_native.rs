// Only the clock and Win32 boundary are simulated. The wait statements and
// scheduler body are extracted from production by frame_wait_boundary.rs.
use rstest::rstest;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Default)]
struct Schedule {
    now: u64,
    sent_at: Option<u64>,
    delivered_at: Option<u64>,
    sent_flood: bool,
    posted_flood: bool,
    stop_at: Option<u64>,
    stop_signal: Option<Arc<AtomicBool>>,
    escape_at: Option<u64>,
    hub_stop_at: Option<u64>,
    hub_signal: Option<Arc<AtomicBool>>,
    wait_result: Option<u32>,
    quit: bool,
    waits: usize,
    peeks: usize,
    dispatched: usize,
}
thread_local! { static OS: RefCell<Schedule> = RefCell::default(); }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Instant(u64);
impl Instant {
    fn now() -> Self {
        OS.with_borrow(|os| Self(os.now))
    }
    fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}
impl std::ops::Add<Duration> for Instant {
    type Output = Self;
    fn add(self, duration: Duration) -> Self {
        Self(self.0 + duration.as_millis() as u64)
    }
}
#[derive(Default)]
struct MSG {
    message: u32,
}
struct BOOL(bool);
impl BOOL {
    fn as_bool(&self) -> bool {
        self.0
    }
}
#[derive(PartialEq, Eq)]
struct WaitResult(u32);
const WAIT_OBJECT_0: WaitResult = WaitResult(0);
const WAIT_TIMEOUT: WaitResult = WaitResult(258);
const WAIT_FAILED: WaitResult = WaitResult(u32::MAX);
const PM_REMOVE: u32 = 1;
const QS_ALLINPUT: u32 = 0x04FF;
const MWMO_INPUTAVAILABLE: u32 = 4;
const WM_QUIT: u32 = 0x12;

fn PeekMessageW(message: &mut MSG, _: Option<()>, min: u32, max: u32, flags: u32) -> BOOL {
    assert_eq!((min, max, flags), (0, 0, PM_REMOVE));
    OS.with_borrow_mut(|os| {
        os.peeks += 1;
        assert!(os.peeks <= 100, "scheduler starved its fixed deadline");
        if os.sent_at.is_some_and(|at| at <= os.now) {
            os.delivered_at = Some(os.now);
            os.sent_at = None;
        }
        if os.sent_flood || os.posted_flood {
            os.now += 1; // returning native message processing takes bounded time
            update_signals(os);
        }
        message.message = if os.quit { WM_QUIT } else { 0x8000 };
        BOOL(os.posted_flood || os.quit)
    }) // sent messages are dispatched even without a queued message
}
fn TranslateMessage(_: &MSG) -> bool {
    true
}
fn DispatchMessageW(_: &MSG) {
    OS.with_borrow_mut(|os| os.dispatched += 1);
}
fn MsgWaitForMultipleObjectsEx(_: Option<()>, ms: u32, mask: u32, flags: u32) -> WaitResult {
    assert!(ms > 0 && ms <= 33, "no busy poll or deadline extension");
    assert_eq!((mask, flags), (QS_ALLINPUT, MWMO_INPUTAVAILABLE));
    OS.with_borrow_mut(|os| {
        os.waits += 1;
        assert!(os.waits <= 100, "no spinning on failed waits");
        if let Some(result) = os.wait_result {
            return WaitResult(result);
        }
        if os.sent_flood {
            return WAIT_OBJECT_0;
        }
        if let Some(at) = os.sent_at.filter(|at| *at <= os.now + u64::from(ms)) {
            os.now = os.now.max(at);
            update_signals(os);
            WAIT_OBJECT_0
        } else {
            os.now += u64::from(ms);
            update_signals(os);
            WAIT_TIMEOUT
        }
    })
}
fn update_signals(os: &Schedule) {
    if os.stop_at.is_some_and(|at| os.now >= at) {
        os.stop_signal
            .as_ref()
            .unwrap()
            .store(true, Ordering::Release);
    }
    if os.hub_stop_at.is_some_and(|at| os.now >= at) {
        os.hub_signal
            .as_ref()
            .unwrap()
            .store(false, Ordering::Release);
    }
}
fn interrupt_generation() -> u64 {
    OS.with_borrow(|os| u64::from(os.escape_at.is_some_and(|at| os.now >= at)))
}
fn interrupt_generation_changed(before: u64, after: u64) -> bool {
    before != after
}
mod windows {
    pub mod core {
        pub struct Error;
        impl Error {
            pub fn from_win32() -> &'static str {
                "modeled native failure"
            }
        }
    }
}

mod thread {
    pub fn sleep(duration: super::Duration) {
        super::OS.with_borrow_mut(|os| os.now += duration.as_millis() as u64);
    }
}

#[derive(Debug)]
enum IndicatorError {
    Backend(String),
}
struct BannerRuntime {
    hub_active: Arc<AtomicBool>,
    generation: u64,
}
type WaitSite = fn(&AtomicBool, &AtomicBool, &BannerRuntime) -> Result<(), IndicatorError>;

#[rstest]
fn sent_message_during_normal_production_wait_meets_unchanged_client_budget() {
    check_sent_message("normal", normal);
}

#[rstest]
fn sent_message_during_suppressed_production_wait_meets_unchanged_client_budget() {
    check_sent_message("suppressed", suppressed);
}

fn check_sent_message(label: &str, wait: WaitSite) {
    OS.set(Schedule {
        sent_at: Some(1),
        ..Schedule::default()
    });
    let runtime = BannerRuntime {
        hub_active: Arc::new(AtomicBool::new(true)),
        generation: 0,
    };
    wait(&AtomicBool::new(false), &AtomicBool::new(false), &runtime).unwrap();
    OS.with_borrow(|os| {
        assert!(
            os.delivered_at.is_some_and(|at| at <= 21),
            "{label}: sent at 1ms, expires at 21ms, wait returned at {}ms, delivered={:?}",
            os.now,
            os.delivered_at
        );
        assert_eq!(os.now, 33, "message wake must not accelerate rendering");
    });
}

fn run_scenario(
    wait: WaitSite,
    mut schedule: Schedule,
) -> (Result<(), IndicatorError>, bool, bool) {
    let stop = Arc::new(AtomicBool::new(false));
    let interrupted = AtomicBool::new(false);
    let runtime = BannerRuntime {
        hub_active: Arc::new(AtomicBool::new(true)),
        generation: 0,
    };
    schedule.stop_signal = Some(Arc::clone(&stop));
    schedule.hub_signal = Some(Arc::clone(&runtime.hub_active));
    update_signals(&schedule);
    OS.set(schedule);
    let result = wait(&stop, &interrupted, &runtime);
    (
        result,
        stop.load(Ordering::Acquire),
        interrupted.load(Ordering::Acquire),
    )
}

#[rstest]
fn empty_queue_blocks_once_without_changing_render_cadence() {
    for wait in [normal as WaitSite, suppressed] {
        run_scenario(wait, Schedule::default()).0.unwrap();
        OS.with_borrow(|os| assert_eq!((os.now, os.waits, os.peeks), (33, 1, 1)));
    }
}

#[rstest]
fn already_pending_sent_messages_are_dispatched_without_a_posted_message() {
    for wait in [normal as WaitSite, suppressed] {
        run_scenario(
            wait,
            Schedule {
                sent_at: Some(0),
                ..Schedule::default()
            },
        )
        .0
        .unwrap();
        OS.with_borrow(|os| assert_eq!((os.delivered_at, os.now), (Some(0), 33)));
    }
}

#[rstest]
fn sent_and_posted_floods_cannot_reset_the_deadline() {
    for wait in [normal as WaitSite, suppressed] {
        for posted in [false, true] {
            run_scenario(
                wait,
                Schedule {
                    posted_flood: posted,
                    sent_flood: !posted,
                    ..Schedule::default()
                },
            )
            .0
            .unwrap();
            OS.with_borrow(|os| {
                assert_eq!((os.now, os.peeks), (33, 33));
                assert_eq!(os.dispatched, if posted { 33 } else { 0 });
            });
        }
    }
}

#[rstest]
fn stop_and_escape_are_checked_between_messages_even_under_flood() {
    for wait in [normal as WaitSite, suppressed] {
        for posted in [false, true] {
            for escape in [false, true] {
                let (result, stopped, interrupted) = run_scenario(
                    wait,
                    Schedule {
                        posted_flood: posted,
                        sent_flood: !posted,
                        stop_at: (!escape).then_some(7),
                        escape_at: escape.then_some(7),
                        ..Schedule::default()
                    },
                );
                result.unwrap();
                assert!(stopped);
                assert_eq!(interrupted, escape);
                OS.with_borrow(|os| assert_eq!(os.now, 7));
            }
        }
    }
}

#[rstest]
fn idle_stop_and_escape_keep_the_original_maximum_frame_wait() {
    for wait in [normal as WaitSite, suppressed] {
        for escape in [false, true] {
            let (result, stopped, interrupted) = run_scenario(
                wait,
                Schedule {
                    stop_at: (!escape).then_some(1),
                    escape_at: escape.then_some(1),
                    ..Schedule::default()
                },
            );
            result.unwrap();
            assert!(stopped);
            assert_eq!(interrupted, escape);
            OS.with_borrow(|os| assert_eq!(os.now, 33));
        }
    }
}

#[rstest]
fn wait_failure_quit_and_hub_loss_propagate_fail_closed_from_both_sites() {
    for wait in [normal as WaitSite, suppressed] {
        for (scenario, expected) in [
            (
                Schedule {
                    wait_result: Some(u32::MAX),
                    ..Schedule::default()
                },
                "wait for indicator messages: modeled native failure",
            ),
            (
                Schedule {
                    wait_result: Some(99),
                    ..Schedule::default()
                },
                "unexpected indicator message wait result: 99",
            ),
            (
                Schedule {
                    quit: true,
                    ..Schedule::default()
                },
                "indicator message queue quit",
            ),
            (
                Schedule {
                    hub_stop_at: Some(7),
                    posted_flood: true,
                    ..Schedule::default()
                },
                "Escape hub stopped",
            ),
        ] {
            let (result, stopped, interrupted) = run_scenario(wait, scenario);
            let Err(IndicatorError::Backend(message)) = result else {
                panic!("must fail closed")
            };
            assert_eq!(message, expected);
            assert!(
                !stopped && !interrupted,
                "backend failure is not user Escape"
            );
            OS.with_borrow(|os| assert!(os.now <= 7));
        }
    }
}
