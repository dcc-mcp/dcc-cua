//! Same-image independent supervisor and private inherited-handle transport.
use super::{
    native::{self, OwnedHandle},
    runtime::{self, Supervisor},
    *,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    ptr::{null, null_mut},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType, ReadFile, WriteFile},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        JobObjects::IsProcessInJob,
        Pipes::{CreatePipe, PeekNamedPipe},
        Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateEventW,
            CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
            GetCurrentProcess, GetCurrentProcessId, GetProcessId,
            InitializeProcThreadAttributeList, OpenProcess, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            ResumeThread, STARTUPINFOEXW, SetEvent, TerminateProcess, UpdateProcThreadAttribute,
            WaitForSingleObject,
        },
    },
};

const PRIVATE_COMMAND: &str = "__capture-preparation-supervisor-v1";
const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const PIPE_BYTES: u32 = (MAX_MESSAGE_BYTES * 2 + 4096) as u32;
type Result<T> = std::result::Result<T, PreparationError>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    spec: CapturePreparationSpec,
    nonce: [u8; 32],
    parent_id: u32,
    parent_birth: u64,
    issued_ms: u64,
    deadline_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    index: u64,
    nonce: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    index: u64,
    nonce: [u8; 32],
    status: Result<CapturePreparationStatus>,
}

struct ReplyWriteReturn {
    terminal: bool,
    result: Result<()>,
}

/// A stalled synchronous pipe write must never stall mutation supervision.
fn reply_writer(
    pipe: &mut Pipe,
) -> Result<(mpsc::SyncSender<Reply>, mpsc::Receiver<ReplyWriteReturn>)> {
    let outgoing = Pipe {
        read: OwnedHandle(0),
        write: std::mem::replace(&mut pipe.write, OwnedHandle(0)),
        buffered: vec![],
    };
    start_reply_writer(move |reply| outgoing.send(reply))
}

fn start_reply_writer(
    write_reply: impl Fn(&Reply) -> Result<()> + Send + 'static,
) -> Result<(mpsc::SyncSender<Reply>, mpsc::Receiver<ReplyWriteReturn>)> {
    let (sender, receiver) = mpsc::sync_channel::<Reply>(1);
    let (returned, returns) = mpsc::channel();
    std::thread::Builder::new()
        .name("dcc-cua-preparation-replies".into())
        .spawn(move || {
            while let Ok(reply) = receiver.recv() {
                let terminal = match &reply.status {
                    Ok(status) => status.cleanup_verified,
                    Err(_) => true,
                };
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write_reply(&reply)))
                        .unwrap_or_else(|_| {
                            Err(PreparationError::new(PreparationFailure::WorkerLost))
                        });
                let failed = result.is_err();
                if returned
                    .send(ReplyWriteReturn { terminal, result })
                    .is_err()
                    || terminal
                    || failed
                {
                    break;
                }
            }
        })
        .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?;
    Ok((sender, returns))
}

pub(super) fn accept_query_sequence(expected: &mut u64, actual: u64) -> bool {
    if actual != *expected {
        return false;
    }
    let Some(next) = expected.checked_add(1) else {
        return false;
    };
    *expected = next;
    true
}

struct Pipe {
    read: OwnedHandle,
    write: OwnedHandle,
    buffered: Vec<u8>,
}
impl Pipe {
    fn validate(&self) -> Result<()> {
        if self.read.0 == 0
            || self.write.0 == 0
            || self.read.0 == self.write.0
            || unsafe { GetFileType(self.read.raw()) } != FILE_TYPE_PIPE
            || unsafe { GetFileType(self.write.raw()) } != FILE_TYPE_PIPE
        {
            return Err(PreparationError::new(
                PreparationFailure::AuthorizationDenied,
            ));
        }
        Ok(())
    }
    fn send<T: Serialize>(&self, value: &T) -> Result<()> {
        let data = serde_json::to_vec(value)
            .map_err(|_| PreparationError::new(PreparationFailure::ProtocolMismatch))?;
        if data.len() > MAX_MESSAGE_BYTES {
            return Err(PreparationError::new(PreparationFailure::ProtocolMismatch));
        }
        let mut frame = (data.len() as u32).to_le_bytes().to_vec();
        frame.extend(data);
        let mut sent = 0;
        if unsafe {
            WriteFile(
                self.write.raw(),
                frame.as_ptr(),
                frame.len() as u32,
                &mut sent,
                null_mut(),
            )
        } == 0
            || sent as usize != frame.len()
        {
            return Err(native::os_error(PreparationFailure::Disconnected));
        }
        Ok(())
    }
    fn receive<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                self.read.raw(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        } == 0
        {
            return Err(native::os_error(PreparationFailure::Disconnected));
        }
        if available != 0 {
            let mut bytes = vec![0_u8; (available as usize).min(MAX_MESSAGE_BYTES + 4)];
            let mut count = 0;
            if unsafe {
                ReadFile(
                    self.read.raw(),
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    &mut count,
                    null_mut(),
                )
            } == 0
            {
                return Err(native::os_error(PreparationFailure::Disconnected));
            }
            bytes.truncate(count as usize);
            self.buffered.extend(bytes);
        }
        if self.buffered.len() > MAX_MESSAGE_BYTES + 4 {
            return Err(PreparationError::new(PreparationFailure::ProtocolMismatch));
        }
        if self.buffered.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_le_bytes(self.buffered[..4].try_into().unwrap()) as usize;
        if length == 0 || length > MAX_MESSAGE_BYTES {
            return Err(PreparationError::new(PreparationFailure::ProtocolMismatch));
        }
        if self.buffered.len() < length + 4 {
            return Ok(None);
        }
        let value = serde_json::from_slice(&self.buffered[4..length + 4])
            .map_err(|_| PreparationError::new(PreparationFailure::ProtocolMismatch))?;
        self.buffered.drain(..length + 4);
        Ok(Some(value))
    }
}

struct Attributes {
    words: Vec<usize>,
}
impl Attributes {
    fn new(handles: &mut [HANDLE]) -> Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        let mut words = vec![0; bytes.div_ceil(std::mem::size_of::<usize>())];
        if unsafe { InitializeProcThreadAttributeList(words.as_mut_ptr().cast(), 1, 0, &mut bytes) }
            == 0
        {
            return Err(native::os_error(
                PreparationFailure::SupervisorNotIndependent,
            ));
        }
        let attributes = Self { words };
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.words.as_ptr().cast_mut().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(native::os_error(
                PreparationFailure::SupervisorNotIndependent,
            ));
        }
        Ok(attributes)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.words.as_mut_ptr().cast());
        }
    }
}

fn pipe_pair(attributes: &SECURITY_ATTRIBUTES) -> Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut read, &mut write, attributes, PIPE_BYTES) } == 0 {
        return Err(native::os_error(PreparationFailure::Disconnected));
    }
    Ok((OwnedHandle(read as usize), OwnedHandle(write as usize)))
}

struct Client {
    spec: CapturePreparationSpec,
    process: OwnedHandle,
    cancel: OwnedHandle,
    pipe: Pipe,
    nonce: [u8; 32],
    index: u64,
    pending: Option<u64>,
    cached: CapturePreparationStatus,
    locally_revoked: bool,
}
impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            SetEvent(self.cancel.raw());
        }
    }
}

impl Client {
    fn poll(&mut self) -> Result<CapturePreparationStatus> {
        if self.cached.cleanup_verified {
            return Ok(self.cached.clone());
        }
        if self.pending.is_none() {
            self.index = self
                .index
                .checked_add(1)
                .ok_or_else(|| PreparationError::new(PreparationFailure::ProtocolMismatch))?;
            self.pipe.send(&Query {
                index: self.index,
                nonce: self.nonce,
            })?;
            self.pending = Some(self.index);
        }
        let until = Instant::now() + Duration::from_millis(5);
        loop {
            if let Some(reply) = self.pipe.receive::<Reply>()? {
                if Some(reply.index) != self.pending || reply.nonce != self.nonce {
                    return Err(PreparationError::new(PreparationFailure::ProtocolMismatch));
                }
                self.pending = None;
                let status = reply.status?;
                if status.preparation_id != self.spec.preparation_id {
                    return Err(PreparationError::new(PreparationFailure::ProtocolMismatch));
                }
                self.cached = status;
                return Ok(self.cached.clone());
            }
            if unsafe { WaitForSingleObject(self.process.raw(), 0) } == WAIT_OBJECT_0 {
                return Err(PreparationError::new(PreparationFailure::WorkerLost));
            }
            if Instant::now() >= until {
                return Ok(self.cached.clone());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[derive(Clone)]
pub struct CapturePreparationHandle {
    client: Arc<Mutex<Client>>,
}
impl CapturePreparationHandle {
    /// The trusted Host consumes its grant before any child or window is created.
    pub fn begin(
        spec: CapturePreparationSpec,
        authorize: impl FnOnce(&CapturePreparationSpec) -> Result<()>,
    ) -> Result<Self> {
        spec.validate()?;
        authorize(&spec)?;
        let issued_ms = runtime::ticks();
        let deadline_ms = super::state::effective_deadline(
            issued_ms,
            spec.lifetime_ms,
            spec.authorization_deadline_ms,
        )?;
        let parent_id = unsafe { GetCurrentProcessId() };
        let parent = OwnedHandle(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                1,
                parent_id,
            )
        } as usize);
        if parent.0 == 0 {
            return Err(native::os_error(PreparationFailure::ParentUnavailable));
        }
        let parent_birth = native::process_birth(parent.raw())?;
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let cancel = OwnedHandle(unsafe { CreateEventW(&security, 1, 0, null()) } as usize);
        if cancel.0 == 0 {
            return Err(native::os_error(
                PreparationFailure::SupervisorNotIndependent,
            ));
        }
        let (child_read, parent_write) = pipe_pair(&security)?;
        let (parent_read, child_write) = pipe_pair(&security)?;
        for handle in [parent_read.raw(), parent_write.raw()] {
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(native::os_error(
                    PreparationFailure::SupervisorNotIndependent,
                ));
            }
        }
        let mut inherited = [
            child_read.raw(),
            child_write.raw(),
            parent.raw(),
            cancel.raw(),
        ];
        let attributes = Attributes::new(&mut inherited)?;
        use std::os::windows::ffi::OsStrExt;
        let image = std::env::current_exe()
            .map_err(|_| PreparationError::new(PreparationFailure::SupervisorNotIndependent))?
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut command = format!(
            "dcc-cua {PRIVATE_COMMAND} --parent {} --read {} --write {} --cancel {}",
            parent.0, child_read.0, child_write.0, cancel.0
        )
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes.words.as_ptr().cast_mut().cast();
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            CreateProcessW(
                image.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW
                    | CREATE_BREAKAWAY_FROM_JOB
                    | CREATE_SUSPENDED
                    | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                null(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(native::os_error(
                PreparationFailure::SupervisorNotIndependent,
            ));
        }
        let process = OwnedHandle(info.hProcess as usize);
        let thread = OwnedHandle(info.hThread as usize);
        let verified = (|| {
            let mut in_job = 1;
            if unsafe { IsProcessInJob(process.raw(), null_mut(), &mut in_job) } == 0 || in_job != 0
            {
                return Err(PreparationError::new(
                    PreparationFailure::SupervisorNotIndependent,
                ));
            }
            if native::executable_identity(process.raw())?
                != native::executable_identity(unsafe { GetCurrentProcess() })?
            {
                return Err(PreparationError::new(
                    PreparationFailure::AuthorizationDenied,
                ));
            }
            native::process_birth(process.raw())?;
            Ok(())
        })();
        if let Err(error) = verified {
            unsafe {
                TerminateProcess(process.raw(), 2);
                WaitForSingleObject(process.raw(), 1000);
            }
            return Err(error);
        }
        if unsafe { ResumeThread(thread.raw()) } == u32::MAX {
            unsafe {
                TerminateProcess(process.raw(), 2);
                WaitForSingleObject(process.raw(), 1000);
            }
            return Err(native::os_error(
                PreparationFailure::SupervisorNotIndependent,
            ));
        }
        drop(child_read);
        drop(child_write);
        let mut nonce = [0; 32];
        nonce[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        nonce[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let pipe = Pipe {
            read: parent_read,
            write: parent_write,
            buffered: vec![],
        };
        pipe.validate()?;
        pipe.send(&Bootstrap {
            spec: spec.clone(),
            nonce,
            parent_id,
            parent_birth,
            issued_ms,
            deadline_ms,
        })?;
        let cached =
            super::state::PreparationState::new(spec.preparation_id, deadline_ms, vec![]).status;
        Ok(Self {
            client: Arc::new(Mutex::new(Client {
                spec,
                process,
                cancel,
                pipe,
                nonce,
                index: 0,
                pending: None,
                cached,
                locally_revoked: false,
            })),
        })
    }
    pub fn state(&self) -> Result<CapturePreparationStatus> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?;
        let mut state = client.poll()?;
        state.capture_revoked |= client.locally_revoked;
        Ok(state)
    }
    pub fn stop(&self, _reason: PreparationFailure) -> Result<CapturePreparationStatus> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?;
        client.locally_revoked = true;
        if unsafe { SetEvent(client.cancel.raw()) } == 0 {
            return Err(native::os_error(PreparationFailure::Disconnected));
        }
        let mut state = client.poll()?;
        state.capture_revoked = true;
        Ok(state)
    }
    pub fn prepared_guard(&self) -> Result<PreparedEvidenceGuard> {
        let guard = PreparedEvidenceGuard {
            handle: self.clone(),
        };
        guard.validate()?;
        Ok(guard)
    }
}

#[derive(Clone)]
pub struct PreparedEvidenceGuard {
    handle: CapturePreparationHandle,
}
impl PreparedEvidenceGuard {
    pub fn preparation_id(&self) -> Result<[u8; 16]> {
        self.validate()?;
        let client = self
            .handle
            .client
            .lock()
            .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?;
        if client.locally_revoked || runtime::ticks() >= client.cached.deadline_ms {
            return Err(PreparationError::new(PreparationFailure::NotActive));
        }
        Ok(client.spec.preparation_id)
    }
    pub fn validate(&self) -> Result<PreparedWindowState> {
        let desktop = native::require_capture_desktop()?;
        let status = self.handle.state()?;
        if status.phase != PreparationPhase::Active
            || status.capture_revoked
            || runtime::ticks() >= status.deadline_ms
        {
            return Err(PreparationError::new(PreparationFailure::NotActive));
        }
        let spec = self
            .handle
            .client
            .lock()
            .map_err(|_| PreparationError::new(PreparationFailure::WorkerLost))?
            .spec
            .clone();
        let current = native::read_scope(&spec)?;
        if !native::geometry_matches(&status.original, &current) {
            return Err(PreparationError::new(PreparationFailure::GeometryChanged));
        }
        let target = current
            .into_iter()
            .find(|state| state.identity == spec.target)
            .ok_or_else(|| PreparationError::new(PreparationFailure::IdentityChanged))?;
        if !target.topmost || !target.visible || target.minimized {
            return Err(PreparationError::new(PreparationFailure::NotActive));
        }
        native::require_inside_desktop(target.visible_bounds)?;
        native::require_inside_desktop(target.bounds)?;
        let final_status = self.handle.state()?;
        if native::require_capture_desktop()? != desktop {
            return Err(PreparationError::new(
                PreparationFailure::DesktopUnavailable,
            ));
        }
        if final_status.phase != PreparationPhase::Active
            || final_status.capture_revoked
            || runtime::ticks() >= final_status.deadline_ms
        {
            return Err(PreparationError::new(PreparationFailure::NotActive));
        }
        Ok(target)
    }
    pub fn capture_frame(&self) -> Result<PassivePreparedFrame> {
        let desktop = native::require_capture_desktop()?;
        let before_state = self.validate()?;
        let target = &before_state.identity;
        let before = crate::exact_window_pixel_evidence(target.process_id, target.window_handle)
            .map_err(|_| PreparationError::new(PreparationFailure::CaptureFailed))?;
        if !evidence_matches(&before_state, &before) || !before.unobscured {
            return Err(PreparationError::new(PreparationFailure::CaptureFailed));
        }
        let capture = crate::capture_visible_window(target.process_id, target.window_handle)
            .map_err(|_| PreparationError::new(PreparationFailure::CaptureFailed))?;
        let after = crate::exact_window_pixel_evidence(target.process_id, target.window_handle)
            .map_err(|_| PreparationError::new(PreparationFailure::CaptureFailed))?;
        let after_state = self.validate()?;
        if before != after
            || !after.unobscured
            || !evidence_matches(&after_state, &after)
            || before_state.identity != after_state.identity
            || capture.bounds != after.visible_bounds
            || capture.width != after.visible_bounds[2] as u32
            || capture.height != after.visible_bounds[3] as u32
        {
            return Err(PreparationError::new(PreparationFailure::CaptureFailed));
        }
        let status = self.handle.state()?;
        if native::require_capture_desktop()? != desktop {
            return Err(PreparationError::new(
                PreparationFailure::DesktopUnavailable,
            ));
        }
        if status.phase != PreparationPhase::Active
            || status.capture_revoked
            || runtime::ticks() >= status.deadline_ms
        {
            return Err(PreparationError::new(PreparationFailure::NotActive));
        }
        Ok(PassivePreparedFrame {
            capture,
            evidence_before: before,
            evidence_after: after,
            actual_foreground: after_state.foreground,
            preparation_id: status.preparation_id,
            captured_at_ms: runtime::ticks(),
        })
    }
    pub fn open_frame_source(&self) -> Result<PreparedEvidenceFrameSource> {
        self.validate()?;
        Ok(PreparedEvidenceFrameSource {
            guard: self.clone(),
        })
    }
}

pub(super) fn evidence_matches(
    state: &PreparedWindowState,
    evidence: &crate::ExactWindowPixelEvidence,
) -> bool {
    let expected = &state.identity;
    expected.process_id == evidence.process_id
        && expected.window_handle == evidence.window_handle
        && expected.native_instance.process_creation_time_100ns
            == evidence.instance.process_creation_time_100ns
        && expected.native_instance.window_thread_id == evidence.instance.window_thread_id
        && expected.native_instance.window_class_hash == evidence.instance.window_class_hash
        && expected.native_instance.owner_window_handle == evidence.instance.owner_window_handle
        && state.bounds == evidence.bounds
        && state.visible_bounds == evidence.visible_bounds
        && state.dpi == evidence.dpi
        && state.visible
        && evidence.visible
        && !state.minimized
        && !evidence.minimized
}
pub struct PreparedEvidenceFrameSource {
    guard: PreparedEvidenceGuard,
}
impl PreparedEvidenceFrameSource {
    pub fn next_frame(&mut self) -> Result<PassivePreparedFrame> {
        self.guard.capture_frame()
    }
}

fn actual_parent_id() -> Result<u32> {
    let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } as usize);
    if snapshot.raw() == INVALID_HANDLE_VALUE {
        return Err(native::os_error(PreparationFailure::ParentUnavailable));
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    while more != 0 {
        if entry.th32ProcessID == unsafe { GetCurrentProcessId() } {
            return Ok(entry.th32ParentProcessID);
        }
        more = unsafe { Process32NextW(snapshot.raw(), &mut entry) };
    }
    Err(PreparationError::new(PreparationFailure::ParentUnavailable))
}

fn run_child(handles: [usize; 4]) -> Result<()> {
    let [parent, read, write, cancel] = handles.map(OwnedHandle);
    let parent_id = unsafe { GetProcessId(parent.raw()) };
    if parent_id == 0
        || parent_id != actual_parent_id()?
        || native::executable_identity(parent.raw())?
            != native::executable_identity(unsafe { GetCurrentProcess() })?
    {
        return Err(PreparationError::new(
            PreparationFailure::AuthorizationDenied,
        ));
    }
    let parent_birth = native::process_birth(parent.raw())?;
    let mut in_job = 1;
    if unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) } == 0 || in_job != 0 {
        return Err(PreparationError::new(
            PreparationFailure::SupervisorNotIndependent,
        ));
    }
    for handle in [&parent, &read, &write, &cancel] {
        if unsafe { SetHandleInformation(handle.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(native::os_error(PreparationFailure::AuthorizationDenied));
        }
    }
    let mut pipe = Pipe {
        read,
        write,
        buffered: vec![],
    };
    pipe.validate()?;
    let timeout = Instant::now() + Duration::from_secs(5);
    let bootstrap = loop {
        if let Some(value) = pipe.receive::<Bootstrap>()? {
            break value;
        }
        if unsafe { WaitForSingleObject(parent.raw(), 0) } != WAIT_TIMEOUT
            || Instant::now() >= timeout
        {
            return Err(PreparationError::new(PreparationFailure::ParentUnavailable));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    if bootstrap.parent_id != parent_id
        || bootstrap.parent_birth != parent_birth
        || bootstrap.nonce == [0; 32]
        || bootstrap.issued_ms > runtime::ticks()
        || super::state::effective_deadline(
            bootstrap.issued_ms,
            bootstrap.spec.lifetime_ms,
            bootstrap.spec.authorization_deadline_ms,
        ) != Ok(bootstrap.deadline_ms)
        || runtime::ticks() >= bootstrap.deadline_ms
    {
        return Err(PreparationError::new(
            PreparationFailure::AuthorizationDenied,
        ));
    }
    let (replies, write_returns) = reply_writer(&mut pipe)?;
    let mut supervisor = Supervisor::start(bootstrap.spec, bootstrap.deadline_ms);
    let mut expected_index = 1;
    loop {
        let parent_alive = unsafe { WaitForSingleObject(parent.raw(), 0) } == WAIT_TIMEOUT;
        if let Ok(runtime) = &mut supervisor {
            if !parent_alive {
                runtime.revoke(PreparationFailure::ParentDied);
            }
            if unsafe { WaitForSingleObject(cancel.raw(), 0) } == WAIT_OBJECT_0 {
                runtime.revoke(PreparationFailure::Stopped);
            }
            runtime.tick();
            if !parent_alive && runtime.state.status.cleanup_verified {
                return Ok(());
            }
        } else if !parent_alive {
            return Err(supervisor.err().unwrap());
        }
        // The main loop never calls synchronous WriteFile after bootstrap.
        loop {
            match write_returns.try_recv() {
                Ok(returned) => {
                    if let Err(error) = returned.result
                        && let Ok(runtime) = &mut supervisor
                    {
                        runtime.revoke(error.reason);
                    }
                    if returned.terminal {
                        return supervisor.as_ref().map(|_| ()).map_err(Clone::clone);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if let Ok(runtime) = &mut supervisor {
                        runtime.revoke(PreparationFailure::Disconnected);
                        if runtime.state.status.cleanup_verified {
                            return Ok(());
                        }
                    } else {
                        return Err(supervisor.err().unwrap());
                    }
                    break;
                }
            }
        }
        match pipe.receive::<Query>() {
            Ok(Some(query)) => {
                if query.nonce != bootstrap.nonce
                    || !accept_query_sequence(&mut expected_index, query.index)
                {
                    if let Ok(runtime) = &mut supervisor {
                        runtime.revoke(PreparationFailure::ProtocolMismatch);
                        // A malformed message revokes capture, never exits a live mutation.
                        continue;
                    }
                    return Err(PreparationError::new(PreparationFailure::ProtocolMismatch));
                }
                let status = supervisor
                    .as_ref()
                    .map(|runtime| runtime.state.status.clone())
                    .map_err(Clone::clone);
                let done = match &status {
                    Ok(status) => status.cleanup_verified,
                    Err(_) => true,
                };
                if replies
                    .try_send(Reply {
                        index: query.index,
                        nonce: query.nonce,
                        status,
                    })
                    .is_err()
                {
                    if let Ok(runtime) = &mut supervisor {
                        runtime.revoke(PreparationFailure::Disconnected);
                    }
                    if done {
                        return supervisor.as_ref().map(|_| ()).map_err(Clone::clone);
                    }
                }
            }
            Ok(None) => {}
            Err(_) => {
                if let Ok(runtime) = &mut supervisor {
                    runtime.revoke(PreparationFailure::Disconnected);
                    if runtime.state.status.cleanup_verified {
                        return Ok(());
                    }
                } else {
                    return Err(supervisor.err().unwrap());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Private early CLI delegate; `arguments` excludes the executable name.
pub fn dispatch_capture_preparation(arguments: &[String]) -> Option<i32> {
    if arguments.first().map(String::as_str) != Some(PRIVATE_COMMAND) {
        return None;
    }
    let parse = || -> Result<[usize; 4]> {
        if arguments.len() != 9 {
            return Err(PreparationError::new(
                PreparationFailure::AuthorizationDenied,
            ));
        }
        let mut handles = [0; 4];
        for (index, name) in ["--parent", "--read", "--write", "--cancel"]
            .iter()
            .enumerate()
        {
            if arguments[index * 2 + 1] != *name {
                return Err(PreparationError::new(
                    PreparationFailure::AuthorizationDenied,
                ));
            }
            handles[index] = arguments[index * 2 + 2]
                .parse()
                .map_err(|_| PreparationError::new(PreparationFailure::AuthorizationDenied))?;
        }
        if handles.contains(&0)
            || handles
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != 4
        {
            return Err(PreparationError::new(
                PreparationFailure::AuthorizationDenied,
            ));
        }
        Ok(handles)
    };
    Some(match parse().and_then(run_child) {
        Ok(()) => 0,
        Err(_) => 2,
    })
}

#[cfg(test)]
mod tests;
