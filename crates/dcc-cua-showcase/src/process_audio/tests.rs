use super::*;
use rstest::rstest;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
struct Calls {
    prepare: usize,
    start: usize,
    stop: usize,
    finalize: usize,
    packets: usize,
    events: Vec<&'static str>,
}
struct Source {
    calls: Arc<Mutex<Calls>>,
    effective: EffectiveAudioSource,
    fail_start: bool,
    fail_stop: bool,
}
impl ProcessAudioSource for Source {
    fn prepare(
        &mut self,
        _: ProcessAudioTarget,
    ) -> Result<EffectiveAudioSource, AudioLifecycleError> {
        self.calls.lock().unwrap().prepare += 1;
        Ok(self.effective)
    }
    fn start(&mut self) -> Result<(), AudioLifecycleError> {
        self.calls.lock().unwrap().start += 1;
        if self.fail_start {
            Err(AudioLifecycleError::Source)
        } else {
            Ok(())
        }
    }
    fn stop(&mut self) -> Result<(), AudioLifecycleError> {
        let mut c = self.calls.lock().unwrap();
        c.stop += 1;
        c.events.push("source_stop");
        if self.fail_stop {
            Err(AudioLifecycleError::Source)
        } else {
            Ok(())
        }
    }
}
struct Mux {
    calls: Arc<Mutex<Calls>>,
    fail_write: bool,
    fail_finalize: bool,
}
impl ProcessAudioMux for Mux {
    fn write_pcm(
        &mut self,
        _: u64,
        samples: &[i16],
        remaining: u64,
    ) -> Result<u64, AudioLifecycleError> {
        if self.fail_write {
            return Err(AudioLifecycleError::Writer);
        }
        let bytes = samples.len() as u64 * 2;
        if bytes > remaining {
            return Err(AudioLifecycleError::Budget);
        }
        self.calls.lock().unwrap().packets += 1;
        Ok(bytes)
    }
    fn finalize(&mut self) -> Result<(), AudioLifecycleError> {
        let mut c = self.calls.lock().unwrap();
        c.finalize += 1;
        c.events.push("mux_finalize");
        if self.fail_finalize {
            Err(AudioLifecycleError::Writer)
        } else {
            Ok(())
        }
    }
}
fn target() -> ProcessAudioTarget {
    ProcessAudioTarget {
        process_id: 42,
        process_creation_time_100ns: 123,
        window_handle: 7,
        window_thread_id: 9,
        window_class_hash: 11,
    }
}
fn grant() -> ProcessAudioGrant {
    ProcessAudioGrant {
        target: target(),
        max_duration_ms: 1_000,
        max_output_bytes: 16_000,
    }
}
fn auth() -> Arc<RecordingAuthorization> {
    Arc::new(RecordingAuthorization::with_clock(u64::MAX, Arc::new(|| 0)))
}
fn ports() -> (Source, Mux, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    (
        Source {
            calls: calls.clone(),
            effective: EffectiveAudioSource::ProcessTree(target()),
            fail_start: false,
            fail_stop: false,
        },
        Mux {
            calls: calls.clone(),
            fail_write: false,
            fail_finalize: false,
        },
        calls,
    )
}
fn start(
    source: Source,
    mux: Mux,
    g: ProcessAudioGrant,
    authorization: Arc<RecordingAuthorization>,
    clock: Arc<AtomicU64>,
) -> Result<ProcessAudioSession<Source, Mux>, AudioStartFailure> {
    ProcessAudioSession::start_with_clock(
        source,
        mux,
        g,
        target(),
        Some(20348),
        authorization,
        Arc::new(move || clock.load(Ordering::SeqCst)),
    )
}

#[rstest]
fn process_audio_normal_stop_joins_source_and_mux_exactly_once() {
    let (s, m, c) = ports();
    let mut session = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0))).unwrap();
    session.ingest(target(), 0, &[1; 960]).unwrap();
    let first = session.stop();
    let second = session.stop();
    assert_eq!(first, second);
    assert!(!first.cleanup_pending);
    assert_eq!(first.packets_accepted, 1);
    drop(session);
    let c = c.lock().unwrap();
    assert_eq!(c.stop, 1);
    assert_eq!(c.finalize, 1);
    assert_eq!(c.events, ["source_stop", "mux_finalize"]);
}

#[rstest]
fn process_audio_refuses_other_sources_before_starting_streams() {
    let (mut s, m, c) = ports();
    s.effective = EffectiveAudioSource::Other;
    let err = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0)))
        .err()
        .unwrap();
    assert_eq!(err.error, AudioLifecycleError::Identity);
    assert!(!err.cleanup.cleanup_pending);
    let c = c.lock().unwrap();
    assert_eq!(c.start, 0);
    assert_eq!(c.stop, 1);
    assert_eq!(c.finalize, 1);
}

#[rstest]
fn process_audio_revocation_stops_before_accepting_more_samples() {
    let (s, m, c) = ports();
    let a = auth();
    let mut session = start(s, m, grant(), a.clone(), Arc::new(AtomicU64::new(0))).unwrap();
    a.revoke();
    assert_eq!(
        session.ingest(target(), 0, &[0; 960]),
        Err(AudioLifecycleError::Authorization)
    );
    assert_eq!(c.lock().unwrap().packets, 0);
    assert_eq!(
        session.stop().terminal_error,
        Some(AudioLifecycleError::Authorization)
    );
}

#[rstest]
fn process_audio_expiry_and_byte_budget_stop_both_owners() {
    let (s, m, c) = ports();
    let clock = Arc::new(AtomicU64::new(0));
    let mut session = start(s, m, grant(), auth(), clock.clone()).unwrap();
    clock.store(1_000_000, Ordering::SeqCst);
    assert_eq!(
        session.ingest(target(), 0, &[0; 960]),
        Err(AudioLifecycleError::Budget)
    );
    assert_eq!(c.lock().unwrap().packets, 0);
    assert!(!session.stop().cleanup_pending);
    let (s, m, c) = ports();
    let mut g = grant();
    g.max_output_bytes = 1;
    let mut session = start(s, m, g, auth(), Arc::new(AtomicU64::new(0))).unwrap();
    assert_eq!(
        session.ingest(target(), 0, &[0; 960]),
        Err(AudioLifecycleError::Budget)
    );
    assert_eq!(c.lock().unwrap().packets, 0);
}

#[rstest]
fn process_audio_start_failure_keeps_unknown_cleanup_and_never_retries() {
    let (mut s, mut m, c) = ports();
    s.fail_start = true;
    s.fail_stop = true;
    m.fail_finalize = true;
    let err = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0)))
        .err()
        .unwrap();
    assert!(err.cleanup.cleanup_pending);
    assert!(!err.cleanup.source_stopped);
    assert!(!err.cleanup.mux_finalized);
    let c = c.lock().unwrap();
    assert_eq!(c.start, 1);
    assert_eq!(c.stop, 1);
    assert_eq!(c.finalize, 1);
}

#[rstest]
fn process_audio_identity_change_and_device_loss_are_terminal() {
    let (s, m, c) = ports();
    let mut session = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0))).unwrap();
    assert_eq!(
        session.ingest(
            ProcessAudioTarget {
                process_creation_time_100ns: 124,
                ..target()
            },
            0,
            &[0; 960]
        ),
        Err(AudioLifecycleError::Identity)
    );
    assert_eq!(c.lock().unwrap().packets, 0);
    assert!(session.ingest(target(), 0, &[0; 960]).is_err());
    let (s, m, c) = ports();
    let mut session = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0))).unwrap();
    let receipt = session.source_failed();
    assert_eq!(receipt.terminal_error, Some(AudioLifecycleError::Source));
    assert_eq!(c.lock().unwrap().start, 1);
    assert_eq!(session.stop(), receipt);
}

#[rstest]
fn process_audio_nonmonotonic_or_oversized_packets_cannot_reach_mux() {
    let (s, m, c) = ports();
    let mut session = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0))).unwrap();
    session.ingest(target(), 100_000, &[0; 960]).unwrap();
    assert_eq!(
        session.ingest(target(), 99_000, &[0; 960]),
        Err(AudioLifecycleError::Packet)
    );
    assert_eq!(c.lock().unwrap().packets, 1);
    let (s, m, c) = ports();
    let mut session = start(s, m, grant(), auth(), Arc::new(AtomicU64::new(0))).unwrap();
    assert_eq!(
        session.ingest(target(), 0, &vec![0; MAX_PROCESS_AUDIO_PACKET_SAMPLES + 2]),
        Err(AudioLifecycleError::Packet)
    );
    assert_eq!(c.lock().unwrap().packets, 0);
}
