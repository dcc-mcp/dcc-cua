//! One owned WASAPI worker. Only the owned process handle/job can be terminated.
//! Audio never creates another screen recorder or receives an output directory.
use std::io;
use std::os::windows::{
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::CommandExt,
};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dcc_cua_protocol::recording_audio::{
    ProcessAudioGrant, ProcessAudioTarget,
    worker::{
        AUDIO_WORKER_PROTOCOL, AudioWorkerCommand, AudioWorkerEvent, AudioWorkerInitialization,
        MAX_AUDIO_WORKER_SAMPLES, read_message, write_message,
    },
};
use dcc_cua_showcase::{
    RecordingAuthorization,
    process_audio::{AudioLifecycleError, EffectiveAudioSource, ProcessAudioSource},
};
use serde::Serialize;

mod recorder;
mod worker;
pub use recorder::NativeRecorderAudioFactory;
pub use worker::run_owned_worker;

/// Metadata-only binding for the trusted issuer; never enumerates other windows.
pub fn observe_process_audio_target(
    process_id: u32,
    window_handle: u64,
) -> Result<ProcessAudioTarget, AudioLifecycleError> {
    crate::capture_identity::validate_exact_window_owner(process_id, window_handle)
        .map_err(|_| AudioLifecycleError::Identity)?;
    let actual = crate::visible_capture::exact_window_instance_evidence(process_id, window_handle)
        .map_err(|_| AudioLifecycleError::Identity)?;
    Ok(ProcessAudioTarget {
        process_id,
        window_handle,
        process_creation_time_100ns: actual.process_creation_time_100ns,
        window_thread_id: actual.window_thread_id,
        window_class_hash: actual.window_class_hash,
    })
}

const START_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_GRACE: Duration = Duration::from_millis(500);
const REAP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Default, Serialize)]
pub struct OwnedAudioWorkerEvidence {
    pub process_id: Option<u32>,
    pub process_creation_time_100ns: Option<u64>,
    pub job_bound: bool,
    pub started: bool,
    pub stop_acknowledged: bool,
    pub forced_termination: bool,
    pub reaped: bool,
    pub exit_code: Option<i32>,
    pub reader_joined: bool,
    pub failure: Option<dcc_cua_protocol::recording_audio::worker::AudioWorkerFailure>,
    pub media_epoch_qpc_100ns: Option<u64>,
    pub first_packet_qpc_100ns: Option<u64>,
    pub last_packet_qpc_100ns: Option<u64>,
    pub received_packets: u64,
    pub first_packet_flags: Option<u32>,
}

pub struct CapturedProcessAudioPacket {
    pub target: ProcessAudioTarget,
    pub pts_us: u64,
    pub samples: Vec<i16>,
    pub flags: u32,
}

struct OwnedWorkerJob {
    _handle: OwnedHandle,
}
impl OwnedWorkerJob {
    fn attach(child: &Child) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: unnamed, non-inheritable job owned only by this recorder.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateJobObjectW transfers one owned kernel handle.
        let owned = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: zero is valid for all limit fields before setting the closed flag.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: exact owned job/process handles and complete ABI-sized limits.
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { _handle: owned })
    }
}

fn child_birth(child: &Child) -> io::Result<u64> {
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};
    let mut birth = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: Child owns the exact process handle; this is not a PID lookup.
    if unsafe {
        GetProcessTimes(
            child.as_raw_handle(),
            &mut birth,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime))
}

pub fn qpc_for_media_epoch(epoch: Instant) -> Result<u64, AudioLifecycleError> {
    use windows_audio::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    let before = Instant::now();
    let mut ticks = 0_i64;
    let mut frequency = 0_i64;
    // SAFETY: valid integer outputs, no desktop or audio device query.
    unsafe {
        QueryPerformanceFrequency(&mut frequency).map_err(|_| AudioLifecycleError::Source)?;
        QueryPerformanceCounter(&mut ticks).map_err(|_| AudioLifecycleError::Source)?;
    }
    let after = Instant::now();
    if ticks < 0 || frequency <= 0 || after.duration_since(before) > Duration::from_millis(2) {
        return Err(AudioLifecycleError::Source);
    }
    let midpoint = before + after.duration_since(before) / 2;
    let elapsed = midpoint
        .checked_duration_since(epoch)
        .ok_or(AudioLifecycleError::Packet)?
        .as_nanos()
        / 100;
    let qpc = ticks as u128 * 10_000_000 / frequency as u128;
    u64::try_from(
        qpc.checked_sub(elapsed)
            .ok_or(AudioLifecycleError::Packet)?,
    )
    .map_err(|_| AudioLifecycleError::Packet)
}

pub struct NativeProcessAudioSource {
    executable: PathBuf,
    grant: ProcessAudioGrant,
    authorization: Arc<RecordingAuthorization>,
    media_epoch: Instant,
    cancelled: Arc<AtomicBool>,
    fault: Arc<AtomicBool>,
    packets: Option<mpsc::SyncSender<CapturedProcessAudioPacket>>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    job: Option<OwnedWorkerJob>,
    reader: Option<JoinHandle<()>>,
    prepared: Option<Receiver<Result<AudioWorkerEvent, AudioLifecycleError>>>,
    ready: Option<Receiver<Result<(), AudioLifecycleError>>>,
    evidence: Arc<Mutex<OwnedAudioWorkerEvidence>>,
    spent: bool,
    start_spent: bool,
}

impl NativeProcessAudioSource {
    pub fn new(
        executable: PathBuf,
        grant: ProcessAudioGrant,
        authorization: Arc<RecordingAuthorization>,
        media_epoch: Instant,
        cancelled: Arc<AtomicBool>,
    ) -> (
        Self,
        Receiver<CapturedProcessAudioPacket>,
        Arc<Mutex<OwnedAudioWorkerEvidence>>,
    ) {
        let (tx, rx) = mpsc::sync_channel(8);
        let evidence = Arc::new(Mutex::new(OwnedAudioWorkerEvidence::default()));
        (
            Self {
                executable,
                grant,
                authorization,
                media_epoch,
                cancelled,
                fault: Arc::new(AtomicBool::new(false)),
                packets: Some(tx),
                child: None,
                stdin: None,
                job: None,
                reader: None,
                prepared: None,
                ready: None,
                evidence: evidence.clone(),
                spent: false,
                start_spent: false,
            },
            rx,
            evidence,
        )
    }
    pub fn faulted(&self) -> bool {
        self.fault.load(Ordering::Acquire)
    }
    pub fn evidence(&self) -> Arc<Mutex<OwnedAudioWorkerEvidence>> {
        self.evidence.clone()
    }

    fn stop_owned(&mut self) -> Result<(), AudioLifecycleError> {
        self.cancelled.store(true, Ordering::Release);
        if let Some(mut input) = self.stdin.take() {
            let _ = write_message(&mut input, &AudioWorkerCommand::Stop {});
        }
        if let Some(child) = self.child.as_mut() {
            let deadline = Instant::now() + STOP_GRACE;
            let mut status = None;
            while Instant::now() < deadline {
                status = child.try_wait().map_err(|_| AudioLifecycleError::Source)?;
                if status.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if status.is_none() {
                // Child::kill uses our process handle, never a numeric PID search.
                if child.kill().is_err()
                    && child
                        .try_wait()
                        .map_err(|_| AudioLifecycleError::Source)?
                        .is_none()
                {
                    return Err(AudioLifecycleError::Source);
                }
                if let Ok(mut evidence) = self.evidence.lock() {
                    evidence.forced_termination = true;
                }
                let deadline = Instant::now() + REAP_TIMEOUT;
                while Instant::now() < deadline {
                    status = child.try_wait().map_err(|_| AudioLifecycleError::Source)?;
                    if status.is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let status = status.ok_or(AudioLifecycleError::Source)?;
            if let Ok(mut evidence) = self.evidence.lock() {
                evidence.reaped = true;
                evidence.exit_code = status.code();
            }
            self.child.take();
            self.job.take();
        }
        if let Some(reader) = self.reader.take() {
            let deadline = Instant::now() + REAP_TIMEOUT;
            while !reader.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if !reader.is_finished() {
                self.reader = Some(reader);
                return Err(AudioLifecycleError::Source);
            }
            reader.join().map_err(|_| AudioLifecycleError::Source)?;
        }
        if let Ok(mut evidence) = self.evidence.lock() {
            evidence.reader_joined = true;
        }
        Ok(())
    }
}

impl ProcessAudioSource for NativeProcessAudioSource {
    fn prepare(
        &mut self,
        target: ProcessAudioTarget,
    ) -> Result<EffectiveAudioSource, AudioLifecycleError> {
        if self.spent || self.cancelled.load(Ordering::Acquire) || target != self.grant.target {
            return Err(AudioLifecycleError::Identity);
        }
        self.spent = true;
        self.grant.validate()?;
        let executable =
            std::fs::canonicalize(&self.executable).map_err(|_| AudioLifecycleError::Source)?;
        if !executable
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("dcc-cua.exe"))
        {
            return Err(AudioLifecycleError::Source);
        }
        let epoch = qpc_for_media_epoch(self.media_epoch)?;
        if let Ok(mut evidence) = self.evidence.lock() {
            evidence.media_epoch_qpc_100ns = Some(epoch);
        }
        let mut command = Command::new(executable);
        command
            .arg("__process-audio-worker")
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear();
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        self.child = Some(command.spawn().map_err(|_| AudioLifecycleError::Source)?);
        let child = self.child.as_mut().ok_or(AudioLifecycleError::Source)?;
        let pid = child.id();
        let birth = child_birth(child).map_err(|_| AudioLifecycleError::Source)?;
        if let Ok(mut evidence) = self.evidence.lock() {
            evidence.process_id = Some(pid);
            evidence.process_creation_time_100ns = Some(birth);
        }
        self.job = Some(OwnedWorkerJob::attach(child).map_err(|_| AudioLifecycleError::Source)?);
        if let Ok(mut evidence) = self.evidence.lock() {
            evidence.job_bound = true;
        }
        let mut input = child.stdin.take().ok_or(AudioLifecycleError::Source)?;
        let mut output = child.stdout.take().ok_or(AudioLifecycleError::Source)?;
        let (hello_tx, hello_rx) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let packets = self.packets.take().ok_or(AudioLifecycleError::Source)?;
        let grant = self.grant.clone();
        let cancelled = self.cancelled.clone();
        let fault = self.fault.clone();
        let authorization = self.authorization.clone();
        let evidence = self.evidence.clone();
        self.reader = Some(
            std::thread::Builder::new()
                .name("dcc-cua-audio-pipe".into())
                .spawn(move || {
                    let result = (|| {
                        let mut started = false;
                        loop {
                            match read_message::<_, AudioWorkerEvent>(&mut output)
                                .map_err(|_| AudioLifecycleError::Source)?
                            {
                                Some(event @ AudioWorkerEvent::Prepared { .. }) => {
                                    hello_tx
                                        .try_send(Ok(event))
                                        .map_err(|_| AudioLifecycleError::Source)?;
                                }
                                Some(AudioWorkerEvent::Started {}) => {
                                    if started {
                                        return Err(AudioLifecycleError::Source);
                                    }
                                    started = true;
                                    if let Ok(mut e) = evidence.lock() {
                                        e.started = true;
                                    }
                                    let _ = ready_tx.try_send(Ok(()));
                                }
                                Some(AudioWorkerEvent::Packet {
                                    qpc_100ns,
                                    device_position: _,
                                    flags,
                                    samples,
                                }) => {
                                    if let Ok(mut e) = evidence.lock() {
                                        e.first_packet_flags.get_or_insert(flags);
                                        e.first_packet_qpc_100ns.get_or_insert(qpc_100ns);
                                        e.last_packet_qpc_100ns = Some(qpc_100ns);
                                        e.received_packets = e.received_packets.saturating_add(1);
                                    }
                                    if !started
                                        || samples.is_empty()
                                        || samples.len() > MAX_AUDIO_WORKER_SAMPLES
                                        || !samples.len().is_multiple_of(2)
                                    {
                                        return Err(AudioLifecycleError::Packet);
                                    }
                                    let pts_us = qpc_100ns
                                        .checked_sub(epoch)
                                        .ok_or(AudioLifecycleError::Packet)?
                                        / 10;
                                    let event = CapturedProcessAudioPacket {
                                        target: grant.target,
                                        pts_us,
                                        samples,
                                        flags,
                                    };
                                    match authorization.with_active(|| {
                                        packets
                                            .try_send(event)
                                            .map_err(|_| AudioLifecycleError::Source)
                                    }) {
                                        Ok(result) => result?,
                                        // Refuse samples on revocation, but drain the final control ack.
                                        Err(_) => {
                                            fault.store(true, Ordering::Release);
                                        }
                                    }
                                }
                                Some(AudioWorkerEvent::Stopped {}) => {
                                    if let Ok(mut e) = evidence.lock() {
                                        e.stop_acknowledged = true;
                                    }
                                    return Ok(());
                                }
                                Some(AudioWorkerEvent::Failed { code }) => {
                                    if let Ok(mut e) = evidence.lock() {
                                        e.failure = Some(code);
                                    }
                                    return Err(AudioLifecycleError::Source);
                                }
                                None => {
                                    return if cancelled.load(Ordering::Acquire) {
                                        Ok(())
                                    } else {
                                        Err(AudioLifecycleError::Source)
                                    };
                                }
                            }
                        }
                    })();
                    if let Err(error) = result {
                        fault.store(true, Ordering::Release);
                        let _ = hello_tx.try_send(Err(error));
                        let _ = ready_tx.try_send(Err(error));
                        authorization.revoke();
                    }
                })
                .map_err(|_| AudioLifecycleError::Source)?,
        );
        let lease_epoch = Instant::now();
        let lease_ticks = u64::try_from(
            self.authorization
                .remaining_lifetime()
                .min(Duration::from_millis(self.grant.max_duration_ms))
                .as_nanos()
                / 100,
        )
        .map_err(|_| AudioLifecycleError::Budget)?;
        let lease_deadline = qpc_for_media_epoch(lease_epoch)?
            .checked_add(lease_ticks)
            .ok_or(AudioLifecycleError::Budget)?;
        write_message(
            &mut input,
            &AudioWorkerCommand::Initialize {
                request: AudioWorkerInitialization {
                    protocol: AUDIO_WORKER_PROTOCOL,
                    grant: self.grant.clone(),
                    deadline_qpc_100ns: epoch
                        .checked_add(self.grant.max_duration_ms * 10_000)
                        .ok_or(AudioLifecycleError::Budget)?
                        .min(lease_deadline),
                },
            },
        )
        .map_err(|_| AudioLifecycleError::Source)?;
        self.stdin = Some(input);
        self.prepared = Some(hello_rx);
        self.ready = Some(ready_rx);
        let hello = self
            .prepared
            .take()
            .ok_or(AudioLifecycleError::Source)?
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| AudioLifecycleError::Source)??;
        match hello {
            AudioWorkerEvent::Prepared {
                target: actual,
                worker_process_id,
                worker_creation_time_100ns,
                windows_build,
            } if actual == target
                && worker_process_id == pid
                && worker_creation_time_100ns == birth
                && windows_build >= 20_348 =>
            {
                Ok(EffectiveAudioSource::ProcessTree(target))
            }
            _ => Err(AudioLifecycleError::Identity),
        }
    }
    fn start(&mut self) -> Result<(), AudioLifecycleError> {
        if self.start_spent || self.cancelled.load(Ordering::Acquire) {
            return Err(AudioLifecycleError::Authorization);
        }
        self.start_spent = true;
        write_message(
            self.stdin.as_mut().ok_or(AudioLifecycleError::Source)?,
            &AudioWorkerCommand::Start {},
        )
        .map_err(|_| AudioLifecycleError::Source)?;
        self.ready
            .take()
            .ok_or(AudioLifecycleError::Source)?
            .recv_timeout(START_TIMEOUT)
            .map_err(|_| AudioLifecycleError::Source)?
    }
    fn stop(&mut self) -> Result<(), AudioLifecycleError> {
        self.stop_owned()
    }
}
impl Drop for NativeProcessAudioSource {
    fn drop(&mut self) {
        let _ = self.stop_owned();
    }
}
