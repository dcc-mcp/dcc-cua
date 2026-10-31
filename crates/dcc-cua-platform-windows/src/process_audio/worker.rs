//! Private worker: one immutable process-tree source; stdout contains framed IPC only.
use dcc_cua_protocol::recording_audio::{
    MIN_PROCESS_LOOPBACK_WINDOWS_BUILD, ProcessAudioTarget,
    worker::{
        AUDIO_WORKER_PROTOCOL, AudioWorkerCommand, AudioWorkerEvent, AudioWorkerFailure as Failure,
        AudioWorkerFailureDiagnostic, AudioWorkerFailureStage, AudioWorkerPacketClock,
        AudioWorkerPacketMetadata, MAX_AUDIO_WORKER_SAMPLES, read_message, write_message,
    },
};
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use windows_audio::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, IAudioCaptureClient,
};

const WAITING: u8 = 0;
const START: u8 = 1;
const STOP: u8 = 2;
const INVALID: u8 = 3;

#[repr(C)]
struct Version {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut Version) -> i32;
}

pub(super) fn supported_windows_build() -> Result<u32, Failure> {
    let mut version = Version {
        size: std::mem::size_of::<Version>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
    };
    // SAFETY: complete ABI-sized writable version structure.
    if unsafe { RtlGetVersion(&mut version) } < 0 {
        return Err(Failure::UnsupportedPlatform);
    }
    if version.major < 10 || version.build < MIN_PROCESS_LOOPBACK_WINDOWS_BUILD {
        return Err(Failure::UnsupportedPlatform);
    }
    Ok(version.build)
}

fn qpc() -> Result<u64, Failure> {
    let (mut count, mut frequency) = (0_i64, 0_i64);
    // SAFETY: valid output pointers; no desktop or device access.
    unsafe {
        windows_audio::Win32::System::Performance::QueryPerformanceCounter(&mut count)
            .map_err(|_| Failure::StreamFailed)?;
        windows_audio::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
            .map_err(|_| Failure::StreamFailed)?;
    }
    if count < 0 || frequency <= 0 {
        return Err(Failure::StreamFailed);
    }
    u64::try_from(count as u128 * 10_000_000 / frequency as u128).map_err(|_| Failure::StreamFailed)
}

fn birth(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<u64, Failure> {
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: caller supplies an owned live process handle or current-process pseudo handle.
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(Failure::IdentityUnavailable);
    }
    Ok(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}

fn exact_target(target: ProcessAudioTarget) -> Result<(), Failure> {
    crate::capture_identity::validate_exact_window_owner(target.process_id, target.window_handle)
        .map_err(|_| Failure::IdentityUnavailable)?;
    let actual = crate::visible_capture::exact_window_instance_evidence(
        target.process_id,
        target.window_handle,
    )
    .map_err(|_| Failure::IdentityUnavailable)?;
    if actual.process_creation_time_100ns != target.process_creation_time_100ns
        || actual.window_thread_id != target.window_thread_id
        || actual.window_class_hash != target.window_class_hash
    {
        return Err(Failure::IdentityChanged);
    }
    Ok(())
}

struct Target {
    handle: OwnedHandle,
    identity: ProcessAudioTarget,
}
impl Target {
    fn open(identity: ProcessAudioTarget) -> Result<Self, Failure> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };
        exact_target(identity)?;
        // SAFETY: only the immutable authorized PID, query/synchronize rights, non-inherited.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                identity.process_id,
            )
        };
        if raw.is_null() {
            return Err(Failure::IdentityUnavailable);
        }
        // SAFETY: successful OpenProcess transferred an owned handle.
        let result = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            identity,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), Failure> {
        // SAFETY: exact owned process handle; zero-time liveness test.
        if unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                self.handle.as_raw_handle(),
                0,
            )
        } != windows_sys::Win32::Foundation::WAIT_TIMEOUT
            || birth(self.handle.as_raw_handle())? != self.identity.process_creation_time_100ns
        {
            return Err(Failure::IdentityChanged);
        }
        exact_target(self.identity)
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

fn control(state: Arc<AtomicU8>) {
    let mut input = io::stdin().lock();
    loop {
        match read_message::<_, AudioWorkerCommand>(&mut input) {
            Ok(Some(AudioWorkerCommand::Start {}))
                if state
                    .compare_exchange(WAITING, START, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok() => {}
            Ok(Some(AudioWorkerCommand::Stop {})) | Ok(None) => {
                state.store(STOP, Ordering::Release);
                return;
            }
            _ => {
                state.store(INVALID, Ordering::Release);
                return;
            }
        }
    }
}

fn check_state(state: &AtomicU8, deadline: u64, duration_limited: bool) -> Result<bool, Failure> {
    check_state_at(
        state.load(Ordering::Acquire),
        qpc()?,
        deadline,
        duration_limited,
    )
}

fn check_state_at(
    state: u8,
    now: u64,
    deadline: u64,
    duration_limited: bool,
) -> Result<bool, Failure> {
    match state {
        INVALID => Err(Failure::ProtocolFailed),
        STOP => Ok(false),
        _ if now >= deadline && duration_limited => Ok(false),
        _ if now >= deadline => Err(Failure::AuthorizationExpired),
        _ => Ok(true),
    }
}

/// Virtual process loopback can report device_position=0 for every packet.
/// Use first-sample QPC for ordering. Forward gaps remain on the media timeline
/// and are explicitly disclosed by the mux; flags still identify capture glitches.
/// Reject overlap beyond the same one-PCM-frame quantization as the mux.
fn qpc_packet_has_valid_order(previous: AudioWorkerPacketClock, timestamp: u64) -> bool {
    let Some(delta) = timestamp.checked_sub(previous.qpc_100ns) else {
        return false;
    };
    delta != 0
        && previous.frames != 0
        && u128::from(delta) * 48_000 + 10_000_000 >= u128::from(previous.frames) * 10_000_000
}

/// Copy only an acquired WASAPI packet, after testing SILENT before dereferencing.
/// SAFETY: non-silent pointer must reference `frames * 2` initialized i16 samples
/// until the matching ReleaseBuffer; silent packets may supply a null pointer.
unsafe fn copy_packet(
    data: *const i16,
    frames: u32,
    flags: u32,
    timestamp: u64,
    previous: Option<AudioWorkerPacketClock>,
) -> Result<Vec<i16>, Failure> {
    let count = (frames as usize)
        .checked_mul(2)
        .ok_or(Failure::ContinuityLost)?;
    if flags & !7 != 0
        || count == 0
        || count > MAX_AUDIO_WORKER_SAMPLES
        || timestamp == 0
        || flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0
        || previous.is_some_and(|clock| !qpc_packet_has_valid_order(clock, timestamp))
        || (previous.is_some() && flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0)
    {
        return Err(Failure::ContinuityLost);
    }
    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
        return Ok(vec![0; count]);
    }
    if data.is_null() {
        return Err(Failure::StreamFailed);
    }
    // SAFETY: caller's acquired packet contract; bounds/flags checked above.
    Ok(unsafe { std::slice::from_raw_parts(data, count) }.to_vec())
}

/// SAFETY: identical acquired-buffer requirements to copy_packet.
unsafe fn copy_packet_with_diagnostic(
    data: *const i16,
    packet: AudioWorkerPacketMetadata,
    diagnostic: &mut AudioWorkerFailureDiagnostic,
) -> Result<Vec<i16>, Failure> {
    // SAFETY: caller retains the acquired buffer until ReleaseBuffer.
    let result = unsafe {
        copy_packet(
            data,
            packet.frames,
            packet.flags,
            packet.qpc_100ns,
            packet.previous_clock,
        )
    };
    if result.is_err() {
        diagnostic.stage = Some(AudioWorkerFailureStage::PacketValidation);
        diagnostic.packet = Some(packet);
    }
    result
}

fn capture_packets(
    output: &mut impl io::Write,
    capture: &IAudioCaptureClient,
    target: &Target,
    state: &AtomicU8,
    deadline: u64,
    duration_limited: bool,
    diagnostic: &mut AudioWorkerFailureDiagnostic,
) -> Result<(), Failure> {
    let mut previous_end = None;
    let mut previous_clock = None;
    while check_state(state, deadline, duration_limited)? {
        target.validate()?;
        // SAFETY: initialized capture client owned by this MTA thread.
        let size = unsafe { capture.GetNextPacketSize() }.map_err(|error| {
            diagnostic.stage = Some(AudioWorkerFailureStage::GetNextPacketSize);
            diagnostic.hresult = Some(error.code().0);
            Failure::StreamFailed
        })?;
        if size == 0 {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        }
        let (mut data, mut frames, mut flags, mut position, mut timestamp) =
            (std::ptr::null_mut(), 0, 0, 0, 0);
        // SAFETY: all writable outputs live until this call returns.
        unsafe {
            capture.GetBuffer(
                &mut data,
                &mut frames,
                &mut flags,
                Some(&mut position),
                Some(&mut timestamp),
            )
        }
        .map_err(|error| {
            diagnostic.stage = Some(AudioWorkerFailureStage::GetBuffer);
            diagnostic.hresult = Some(error.code().0);
            Failure::StreamFailed
        })?;
        // SAFETY: acquired PCM16 stereo buffer is held until ReleaseBuffer below.
        let samples = unsafe {
            copy_packet_with_diagnostic(
                data.cast(),
                AudioWorkerPacketMetadata {
                    frames,
                    flags,
                    qpc_100ns: timestamp,
                    device_position: position,
                    expected_device_position: previous_end,
                    previous_clock,
                    null_data: data.is_null(),
                },
                diagnostic,
            )
        };
        // SAFETY: exactly the frames from the successful GetBuffer, including error paths.
        unsafe { capture.ReleaseBuffer(frames) }.map_err(|error| {
            diagnostic.stage = Some(AudioWorkerFailureStage::ReleaseBuffer);
            diagnostic.hresult = Some(error.code().0);
            Failure::StreamFailed
        })?;
        let samples = samples?;
        target.validate()?;
        if !check_state(state, deadline, duration_limited)? {
            break;
        }
        if timestamp >= deadline {
            return if duration_limited {
                Ok(())
            } else {
                Err(Failure::AuthorizationExpired)
            };
        }
        previous_clock = Some(AudioWorkerPacketClock {
            qpc_100ns: timestamp,
            frames,
        });
        previous_end = Some(
            position
                .checked_add(u64::from(frames))
                .ok_or(Failure::ContinuityLost)?,
        );
        write_message(
            output,
            &AudioWorkerEvent::Packet {
                qpc_100ns: timestamp,
                device_position: position,
                flags,
                samples,
            },
        )
        .map_err(|_| Failure::ProtocolFailed)?;
    }
    Ok(())
}

fn run(
    output: &mut impl io::Write,
    diagnostic: &mut AudioWorkerFailureDiagnostic,
) -> Result<bool, Failure> {
    let request = {
        let mut input = io::stdin().lock();
        match read_message::<_, AudioWorkerCommand>(&mut input)
            .map_err(|_| Failure::ProtocolFailed)?
        {
            Some(AudioWorkerCommand::Initialize { request }) => request,
            _ => return Err(Failure::InvalidGrant),
        }
    };
    request
        .grant
        .validate()
        .map_err(|_| Failure::InvalidGrant)?;
    let now = qpc()?;
    if request.protocol != AUDIO_WORKER_PROTOCOL
        || request.deadline_qpc_100ns <= now
        || request.deadline_qpc_100ns - now > request.grant.max_duration_ms * 10_000
    {
        return Err(Failure::InvalidGrant);
    }
    let windows_build = supported_windows_build()?;
    let target = Target::open(request.grant.target)?;
    let state = Arc::new(AtomicU8::new(WAITING));
    let reader = state.clone();
    std::thread::Builder::new()
        .name("audio-control".into())
        .spawn(move || control(reader))
        .map_err(|_| Failure::ProtocolFailed)?;
    wasapi::initialize_mta()
        .ok()
        .map_err(|_| Failure::ActivationFailed)?;
    let _apartment = Apartment;
    // This API can block in activation; the parent owns the kill-on-close job.
    // true includes ONLY the granted target tree. No device/microphone fallback.
    let mut client =
        wasapi::AudioClient::new_application_loopback_client(request.grant.target.process_id, true)
            .map_err(|_| Failure::ActivationFailed)?;
    if !check_state(&state, request.deadline_qpc_100ns, request.duration_limited)? {
        return Ok(false);
    }
    target.validate()?;
    let format = wasapi::WaveFormat::new(16, 16, &wasapi::SampleType::Int, 48_000, 2, None);
    client
        .initialize_client(
            &format,
            &wasapi::Direction::Capture,
            &wasapi::StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: 200_000,
            },
        )
        .map_err(|_| Failure::ActivationFailed)?;
    // SAFETY: initialized AudioClient exposes this capture service; thread-local COM.
    let capture: IAudioCaptureClient =
        unsafe { client.as_iaudioclient().GetService() }.map_err(|_| Failure::ActivationFailed)?;
    // SAFETY: current-process pseudo handle is valid without ownership transfer.
    let worker_creation_time_100ns =
        birth(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() })?;
    write_message(
        output,
        &AudioWorkerEvent::Prepared {
            target: request.grant.target,
            worker_process_id: std::process::id(),
            worker_creation_time_100ns,
            windows_build,
        },
    )
    .map_err(|_| Failure::ProtocolFailed)?;
    while state.load(Ordering::Acquire) == WAITING {
        if !check_state(&state, request.deadline_qpc_100ns, request.duration_limited)? {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    if !check_state(&state, request.deadline_qpc_100ns, request.duration_limited)? {
        return Ok(false);
    }
    target.validate()?;
    client.start_stream().map_err(|_| Failure::StreamFailed)?;
    let result = write_message(output, &AudioWorkerEvent::Started {})
        .map_err(|_| Failure::ProtocolFailed)
        .and_then(|()| {
            capture_packets(
                output,
                &capture,
                &target,
                &state,
                request.deadline_qpc_100ns,
                request.duration_limited,
                diagnostic,
            )
        });
    let stop = client.stop_stream().map_err(|_| Failure::StreamFailed);
    diagnostic.stream_stop_succeeded = Some(stop.is_ok());
    if result.is_ok() && stop.is_err() {
        diagnostic.stage = Some(AudioWorkerFailureStage::StopStream);
    }
    result.and(stop)?;
    Ok(request.duration_limited
        && state.load(Ordering::Acquire) != STOP
        && qpc()? >= request.deadline_qpc_100ns)
}

pub fn run_owned_worker() -> i32 {
    let mut output = io::stdout().lock();
    let mut diagnostic = AudioWorkerFailureDiagnostic::default();
    match run(&mut output, &mut diagnostic) {
        Ok(duration_exhausted) => {
            if write_message(
                &mut output,
                &AudioWorkerEvent::Stopped { duration_exhausted },
            )
            .is_ok()
            {
                0
            } else {
                2
            }
        }
        Err(code) => {
            let _ = write_message(
                &mut output,
                &AudioWorkerEvent::Failed {
                    code,
                    diagnostic: Some(diagnostic),
                },
            );
            2
        }
    }
}

#[cfg(test)]
mod tests;
