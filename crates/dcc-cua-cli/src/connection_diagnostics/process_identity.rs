use super::{ProcessIdentity, ProcessStatus};

pub(super) fn current_process() -> ProcessIdentity {
    capture(std::process::id())
}

pub(super) fn parent_process(bridge: &ProcessIdentity) -> Option<ProcessIdentity> {
    let pid = parent_pid()?;
    Some(fence_parent_identity(capture(pid), bridge))
}

pub(super) fn fence_parent_identity(
    mut parent: ProcessIdentity,
    bridge: &ProcessIdentity,
) -> ProcessIdentity {
    // The original parent can exit and its PID can be reused before inspection.
    if parent
        .creation_time_unix_ms
        .zip(bridge.creation_time_unix_ms)
        .is_some_and(|(parent, child)| parent > child)
    {
        parent.creation_id = None;
        parent.creation_time_unix_ms = None;
    }
    parent
}

fn capture(pid: u32) -> ProcessIdentity {
    let (creation_time_unix_ms, creation_id) = creation(pid).unwrap_or_default();
    ProcessIdentity {
        pid,
        creation_time_unix_ms,
        creation_id,
    }
}

pub(super) fn status(expected: &ProcessIdentity) -> ProcessStatus {
    let Some(expected_creation) = expected.creation_id.as_deref() else {
        return ProcessStatus::Unknown;
    };
    match creation(expected.pid) {
        Ok((_, Some(observed))) if observed == expected_creation => ProcessStatus::Live,
        Ok((_, Some(_))) => ProcessStatus::Ended,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProcessStatus::Ended,
        _ => ProcessStatus::Unknown,
    }
}

#[cfg(windows)]
fn creation(pid: u32) -> std::io::Result<(Option<u64>, Option<String>)> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenProcess accepts a value-only PID; the returned handle is
    // checked and closed exactly once after querying exclusively local outputs.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        let error = std::io::Error::last_os_error();
        return Err(
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                std::io::Error::from(std::io::ErrorKind::NotFound)
            } else {
                error
            },
        );
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let success =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    let error = std::io::Error::last_os_error();
    unsafe { CloseHandle(process) };
    if success == 0 {
        return Err(error);
    }
    if exited.dwLowDateTime != 0 || exited.dwHighDateTime != 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let unix_ms = ticks
        .checked_sub(116_444_736_000_000_000)
        .map(|ticks| ticks / 10_000);
    Ok((unix_ms, Some(format!("windows-filetime:{ticks}"))))
}

#[cfg(windows)]
fn parent_pid() -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: the snapshot is owned and closed below; each API receives one
    // correctly sized, live local PROCESSENTRY32W buffer.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    if unsafe { Process32FirstW(snapshot, &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == std::process::id() {
                found = (entry.th32ParentProcessID > 0).then_some(entry.th32ParentProcessID);
                break;
            }
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snapshot) };
    found
}

#[cfg(target_os = "linux")]
fn creation(pid: u32) -> std::io::Result<(Option<u64>, Option<String>)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // comm is parenthesized and can itself contain spaces and parentheses.
    let tail = stat
        .rsplit_once(')')
        .ok_or(std::io::ErrorKind::InvalidData)?
        .1;
    let fields = tail.split_whitespace().collect::<Vec<_>>();
    if fields
        .first()
        .is_some_and(|state| matches!(*state, "Z" | "X"))
    {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let start_ticks: u64 = fields
        .get(19)
        .ok_or(std::io::ErrorKind::InvalidData)?
        .parse()
        .map_err(|_| std::io::ErrorKind::InvalidData)?;
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let boot_id = boot_id.trim();
    if boot_id.len() != 36
        || !boot_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let boot_seconds = std::fs::read_to_string("/proc/stat").ok().and_then(|stat| {
        stat.lines().find_map(|line| {
            line.strip_prefix("btime ")
                .and_then(|time| time.parse::<u64>().ok())
        })
    });
    // SAFETY: sysconf reads a system constant and does not dereference pointers.
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let unix_ms = boot_seconds.and_then(|boot| {
        let rate = u64::try_from(ticks_per_second)
            .ok()
            .filter(|rate| *rate > 0)?;
        boot.checked_mul(1000)?
            .checked_add(start_ticks.checked_mul(1000)? / rate)
    });
    Ok((
        unix_ms,
        Some(format!("linux-start-ticks:{boot_id}:{start_ticks}")),
    ))
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn creation(_pid: u32) -> std::io::Result<(Option<u64>, Option<String>)> {
    // An unsupported creation-identity probe cannot certify liveness or PID reuse.
    Ok((None, None))
}

#[cfg(unix)]
fn parent_pid() -> Option<u32> {
    // SAFETY: getppid has no preconditions and does not dereference pointers.
    u32::try_from(unsafe { libc::getppid() })
        .ok()
        .filter(|pid| *pid > 0)
}

#[cfg(not(any(unix, windows)))]
fn parent_pid() -> Option<u32> {
    None
}
