//! Exact-scope Win32 reads and the sole synchronous, non-activating writer.
use super::*;
use std::{
    collections::BTreeSet,
    os::windows::ffi::OsStringExt,
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        OPEN_EXISTING,
    },
    System::Threading::{
        CreateMutexW, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW, ReleaseMutex, WaitForSingleObject,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GA_ROOT, GW_HWNDNEXT, GW_HWNDPREV, GW_OWNER, GWL_EXSTYLE, GetAncestor,
        GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_NOTOPMOST, HWND_TOP,
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SetWindowPos,
        WS_EX_TOPMOST,
    },
};

type Result<T> = std::result::Result<T, PreparationError>;

pub(super) fn os_error(reason: PreparationFailure) -> PreparationError {
    PreparationError {
        reason,
        os_error: std::io::Error::last_os_error().raw_os_error(),
    }
}

pub(super) struct OwnedHandle(pub usize);
impl OwnedHandle {
    pub fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}
unsafe impl Send for OwnedHandle {}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.0 != 0 && self.raw() != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.raw());
            }
        }
    }
}

pub(super) fn process_birth(handle: HANDLE) -> Result<u64> {
    let (mut birth, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    if unsafe { GetProcessTimes(handle, &mut birth, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(os_error(PreparationFailure::ParentUnavailable));
    }
    Ok((u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime))
}

pub(super) fn executable_identity(process: HANDLE) -> Result<PreparationExecutableIdentity> {
    let mut path = vec![0_u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0
        || length == 0
    {
        return Err(os_error(PreparationFailure::IdentityChanged));
    }
    path.truncate(length as usize);
    path.push(0);
    let file = OwnedHandle(unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    } as usize);
    if file.raw() == INVALID_HANDLE_VALUE {
        return Err(os_error(PreparationFailure::IdentityChanged));
    }
    let mut canonical = vec![0_u16; 32_768];
    let count = unsafe {
        GetFinalPathNameByHandleW(
            file.raw(),
            canonical.as_mut_ptr(),
            canonical.len() as u32,
            0,
        )
    };
    if count == 0 || count as usize >= canonical.len() {
        return Err(os_error(PreparationFailure::IdentityChanged));
    }
    canonical.truncate(count as usize);
    let canonical_image_path = std::ffi::OsString::from_wide(&canonical)
        .into_string()
        .map_err(|_| PreparationError::new(PreparationFailure::IdentityChanged))?;
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            file.raw(),
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(os_error(PreparationFailure::IdentityChanged));
    }
    Ok(PreparationExecutableIdentity {
        canonical_image_path,
        volume_serial_number: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
    })
}

pub fn read_capture_preparation_identity(
    process_id: u32,
    window_handle: u64,
) -> Result<PreparationWindowIdentity> {
    let before = crate::exact_window_native_state(process_id, window_handle)
        .map_err(|_| PreparationError::new(PreparationFailure::TargetUnavailable))?;
    let process =
        OwnedHandle(
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) } as usize,
        );
    if process.0 == 0
        || process_birth(process.raw())? != before.instance.process_creation_time_100ns
    {
        return Err(PreparationError::new(PreparationFailure::IdentityChanged));
    }
    let executable = executable_identity(process.raw())?;
    let after = crate::exact_window_native_state(process_id, window_handle)
        .map_err(|_| PreparationError::new(PreparationFailure::TargetUnavailable))?;
    if before.instance != after.instance {
        return Err(PreparationError::new(PreparationFailure::IdentityChanged));
    }
    Ok(PreparationWindowIdentity {
        process_id,
        window_handle,
        executable,
        native_instance: PreparationNativeInstance {
            process_creation_time_100ns: before.instance.process_creation_time_100ns,
            window_thread_id: before.instance.window_thread_id,
            window_class_hash: before.instance.window_class_hash,
            owner_window_handle: before.instance.owner_window_handle,
        },
    })
}

fn identity_at(hwnd: HANDLE) -> Result<Option<PreparationWindowIdentity>> {
    if hwnd.is_null() {
        return Ok(None);
    }
    let mut pid = 0;
    if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
        return Err(os_error(PreparationFailure::AnchorChanged));
    }
    read_capture_preparation_identity(pid, hwnd as usize as u64).map(Some)
}

fn read_state(expected: &PreparationWindowIdentity) -> Result<PreparedWindowState> {
    if read_capture_preparation_identity(expected.process_id, expected.window_handle)? != *expected
    {
        return Err(PreparationError::new(PreparationFailure::IdentityChanged));
    }
    let state = crate::exact_window_native_state(expected.process_id, expected.window_handle)
        .map_err(|_| PreparationError::new(PreparationFailure::ReadbackFailed))?;
    let hwnd = expected.window_handle as usize as HANDLE;
    let result = PreparedWindowState {
        identity: expected.clone(),
        topmost: unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0,
        bounds: state
            .bounds
            .ok_or_else(|| PreparationError::new(PreparationFailure::ReadbackFailed))?,
        visible_bounds: state
            .visible_bounds
            .ok_or_else(|| PreparationError::new(PreparationFailure::ReadbackFailed))?,
        dpi: state.dpi,
        visible: state.visible,
        minimized: state.minimized,
        foreground: state.foreground,
        anchors: PreparationAnchors {
            above: identity_at(unsafe { GetWindow(hwnd, GW_HWNDPREV) })?,
            below: identity_at(unsafe { GetWindow(hwnd, GW_HWNDNEXT) })?,
        },
    };
    if read_capture_preparation_identity(expected.process_id, expected.window_handle)? != *expected
    {
        return Err(PreparationError::new(PreparationFailure::IdentityChanged));
    }
    Ok(result)
}

struct Census {
    windows: Vec<(u64, u64)>,
    complete: bool,
}
unsafe extern "system" fn collect_window(hwnd: HANDLE, parameter: LPARAM) -> i32 {
    let census = unsafe { &mut *(parameter as *mut Census) };
    if census.windows.len() >= 4096 || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
        census.complete = false;
        return 0;
    }
    census.windows.push((
        hwnd as usize as u64,
        unsafe { GetWindow(hwnd, GW_OWNER) } as usize as u64,
    ));
    1
}

fn affected_handles(root: u64) -> Result<Vec<u64>> {
    let mut census = Census {
        windows: vec![],
        complete: true,
    };
    if unsafe { EnumWindows(Some(collect_window), (&mut census as *mut Census) as LPARAM) } == 0
        || !census.complete
    {
        return Err(os_error(PreparationFailure::EnumerationIncomplete));
    }
    connected_scope(root, &census.windows)
}

pub(super) fn connected_scope(root: u64, windows: &[(u64, u64)]) -> Result<Vec<u64>> {
    if !windows.iter().any(|(hwnd, _)| *hwnd == root) {
        return Err(PreparationError::new(PreparationFailure::TargetUnavailable));
    }
    let mut group = BTreeSet::from([root]);
    loop {
        let old_len = group.len();
        for (hwnd, owner) in windows {
            if group.contains(hwnd) && *owner != 0 {
                group.insert(*owner);
            }
            if group.contains(owner) {
                group.insert(*hwnd);
            }
        }
        if group.len() == old_len {
            break;
        }
        if group.len() > MAX_AFFECTED_WINDOWS {
            return Err(PreparationError::new(
                PreparationFailure::AffectedScopeChanged,
            ));
        }
    }
    // Preserve native top-to-bottom order for relationship restoration.
    let ordered = windows
        .iter()
        .filter(|(hwnd, _)| group.contains(hwnd))
        .map(|(hwnd, _)| *hwnd)
        .collect::<Vec<_>>();
    if ordered.len() != group.len() {
        return Err(PreparationError::new(
            PreparationFailure::EnumerationIncomplete,
        ));
    }
    for hwnd in &ordered {
        let mut seen = BTreeSet::new();
        let mut cursor = *hwnd;
        while cursor != 0 {
            if !seen.insert(cursor) {
                return Err(PreparationError::new(
                    PreparationFailure::EnumerationIncomplete,
                ));
            }
            cursor = windows
                .iter()
                .find(|(window, _)| *window == cursor)
                .ok_or_else(|| PreparationError::new(PreparationFailure::EnumerationIncomplete))?
                .1;
        }
    }
    Ok(ordered)
}

pub(super) fn read_scope(spec: &CapturePreparationSpec) -> Result<Vec<PreparedWindowState>> {
    let _dpi = crate::visible_capture::ThreadDpiAwarenessGuard::per_monitor_v2()
        .map_err(|_| PreparationError::new(PreparationFailure::ReadbackFailed))?;
    let allowed = std::iter::once(&spec.target)
        .chain(&spec.allowed_affected_scope)
        .collect::<Vec<_>>();
    let expected = allowed
        .iter()
        .map(|id| id.window_handle)
        .collect::<BTreeSet<_>>();
    let handles = affected_handles(spec.target.window_handle)?;
    if handles.iter().copied().collect::<BTreeSet<_>>() != expected {
        return Err(PreparationError::new(
            PreparationFailure::AffectedScopeChanged,
        ));
    }
    let states = handles
        .into_iter()
        .map(|hwnd| read_state(allowed.iter().find(|id| id.window_handle == hwnd).unwrap()))
        .collect::<Result<Vec<_>>>()?;
    if affected_handles(spec.target.window_handle)?
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err(PreparationError::new(
            PreparationFailure::AffectedScopeChanged,
        ));
    }
    Ok(states)
}

pub(super) fn geometry_matches(
    original: &[PreparedWindowState],
    current: &[PreparedWindowState],
) -> bool {
    original.len() == current.len()
        && original.iter().all(|old| {
            current.iter().any(|now| {
                old.identity == now.identity
                    && old.bounds == now.bounds
                    && old.visible_bounds == now.visible_bounds
                    && old.dpi == now.dpi
                    && old.visible == now.visible
                    && old.minimized == now.minimized
            })
        })
}

pub(super) fn restored_matches(
    original: &[PreparedWindowState],
    current: &[PreparedWindowState],
) -> bool {
    geometry_matches(original, current)
        && original.iter().all(|old| {
            current.iter().any(|now| {
                old.identity == now.identity
                    && old.topmost == now.topmost
                    && old.anchors == now.anchors
            })
        })
}

pub(super) struct TargetGate {
    handle: OwnedHandle,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for TargetGate {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle.raw());
        }
    }
}
pub(super) fn acquire_gate(identity: &PreparationWindowIdentity) -> Result<TargetGate> {
    let name = format!(
        "Local\\dcc-cua-capture-preparation-{}-{}-{}",
        identity.process_id,
        identity.native_instance.process_creation_time_100ns,
        identity.window_handle
    )
    .encode_utf16()
    .chain(Some(0))
    .collect::<Vec<_>>();
    let handle = OwnedHandle(unsafe { CreateMutexW(null(), 0, name.as_ptr()) } as usize);
    if handle.0 == 0 {
        return Err(os_error(PreparationFailure::GateBusy));
    }
    let result = unsafe { WaitForSingleObject(handle.raw(), 0) };
    if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
        return Err(PreparationError::new(PreparationFailure::GateBusy));
    }
    Ok(TargetGate {
        handle,
        _thread_bound: std::marker::PhantomData,
    })
}

pub(super) fn require_inside_desktop(bounds: [i32; 4]) -> Result<()> {
    let _dpi = crate::visible_capture::ThreadDpiAwarenessGuard::per_monitor_v2()
        .map_err(|_| PreparationError::new(PreparationFailure::ReadbackFailed))?;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    let desktop = unsafe {
        [
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        ]
    };
    if !crate::visible_capture::physical_rectangle_within_desktop(bounds, desktop) {
        return Err(PreparationError::new(
            PreparationFailure::TargetOutsideDesktop,
        ));
    }
    Ok(())
}

pub(super) fn require_capture_desktop() -> Result<String> {
    use windows_sys::Win32::System::{
        StationsAndDesktops::{GetThreadDesktop, GetUserObjectInformationW, UOI_NAME},
        Threading::GetCurrentThreadId,
    };
    let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    if desktop.is_null() {
        return Err(os_error(PreparationFailure::DesktopUnavailable));
    }
    let mut name = vec![0_u16; 512];
    let mut needed = 0;
    if unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * std::mem::size_of::<u16>()) as u32,
            &mut needed,
        )
    } == 0
        || needed == 0
        || needed as usize > name.len() * 2
        || !needed.is_multiple_of(2)
    {
        return Err(os_error(PreparationFailure::DesktopUnavailable));
    }
    name.truncate(needed as usize / 2);
    if name.last() == Some(&0) {
        name.pop();
    }
    let name = String::from_utf16(&name)
        .map_err(|_| PreparationError::new(PreparationFailure::DesktopUnavailable))?;
    let input = crate::desktop_state();
    if name.is_empty()
        || input.input_desktop_error.is_some()
        || input.input_desktop_name.as_deref() != Some(name.as_str())
    {
        return Err(PreparationError::new(
            PreparationFailure::DesktopUnavailable,
        ));
    }
    Ok(name)
}

fn adjacent_guard(
    spec: &CapturePreparationSpec,
    original: &[PreparedWindowState],
    original_desktop: &str,
) -> Result<()> {
    let current = read_scope(spec)?;
    if !geometry_matches(original, &current) {
        return Err(PreparationError::new(PreparationFailure::GeometryChanged));
    }
    let desktop = crate::desktop_state();
    if desktop.input_desktop_error.is_some()
        || desktop.input_desktop_name.as_deref() != Some(original_desktop)
    {
        return Err(PreparationError::new(
            PreparationFailure::DesktopUnavailable,
        ));
    }
    Ok(())
}

pub(super) struct MutationContext<'a> {
    pub spec: &'a CapturePreparationSpec,
    pub original: &'a [PreparedWindowState],
    pub original_desktop: &'a str,
    pub journal: &'a super::journal::Journal,
    pub sequence: u64,
    pub promotion_revoked: &'a AtomicBool,
    pub deadline: u64,
}

fn write(
    context: &MutationContext<'_>,
    calls: &mut Vec<PreparationNativeCallReceipt>,
    identity: &PreparationWindowIdentity,
    after: HANDLE,
    promoting: bool,
) -> Result<()> {
    let promotion_fence = promoting.then_some((context.promotion_revoked, context.deadline));
    let sequence = context.sequence;
    require_promotion_permission(promotion_fence, super::runtime::ticks())?;
    adjacent_guard(context.spec, context.original, context.original_desktop)?;
    let call_index = calls.len();
    context.journal.record(
        &format!("sequence-{sequence}-call-{call_index}-dispatch"),
        &(identity, after as isize),
    )?;
    // Repeat finite identity/scope checks adjacent to the actual write after disk I/O.
    adjacent_guard(context.spec, context.original, context.original_desktop)?;
    require_promotion_permission(promotion_fence, super::runtime::ticks())?;
    let success = unsafe {
        SetWindowPos(
            identity.window_handle as usize as HANDLE,
            after,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
        )
    } != 0;
    let error = (!success).then(|| os_error(PreparationFailure::MutationFailed));
    let call = PreparationNativeCallReceipt {
        window_handle: identity.window_handle,
        insert_after: after as isize as i64,
        api_success: success,
        os_error: error.as_ref().and_then(|item| item.os_error),
        returned_at_ms: super::runtime::ticks(),
    };
    calls.push(call.clone());
    context.journal.record(
        &format!("sequence-{sequence}-call-{call_index}-returned"),
        &call,
    )?;
    if let Some(error) = error {
        return Err(error);
    }
    Ok(())
}

pub(super) fn require_promotion_permission(
    fence: Option<(&AtomicBool, u64)>,
    now_ms: u64,
) -> Result<()> {
    if let Some((revoked, deadline)) = fence {
        if revoked.load(Ordering::Acquire) {
            return Err(PreparationError::new(PreparationFailure::Stopped));
        }
        if now_ms >= deadline {
            return Err(PreparationError::new(PreparationFailure::Expired));
        }
    }
    Ok(())
}

pub(super) fn mutate(
    context: &MutationContext<'_>,
    kind: PreparationMutationKind,
) -> PreparationMutationReceipt {
    let spec = context.spec;
    let original = context.original;
    let mut native_calls = vec![];
    let result = (|| {
        match kind {
            PreparationMutationKind::Promote => {
                write(context, &mut native_calls, &spec.target, HWND_TOPMOST, true)?
            }
            PreparationMutationKind::Restore => {
                // Owners first: a root tier change can propagate to owned windows.
                let mut tiers = original.iter().collect::<Vec<_>>();
                tiers.sort_by_key(|state| owner_depth(state, original));
                for state in tiers {
                    write(
                        context,
                        &mut native_calls,
                        &state.identity,
                        if state.topmost {
                            HWND_TOPMOST
                        } else {
                            HWND_NOTOPMOST
                        },
                        false,
                    )?;
                }
                for state in original {
                    let after = if let Some(anchor) = &state.anchors.above {
                        if read_capture_preparation_identity(
                            anchor.process_id,
                            anchor.window_handle,
                        )? != *anchor
                        {
                            return Err(PreparationError::new(PreparationFailure::AnchorChanged));
                        }
                        anchor.window_handle as usize as HANDLE
                    } else {
                        HWND_TOP
                    };
                    write(context, &mut native_calls, &state.identity, after, false)?;
                }
            }
        }
        Ok(())
    })();
    let (api_success, os_error_code, failure) = match result {
        Ok(()) => (true, None, None),
        Err(error) => (false, error.os_error, Some(error)),
    };
    let readback = read_scope(spec);
    let (states, failure) = match readback {
        Ok(states) => (states, failure),
        Err(error) => (vec![], failure.or(Some(error))),
    };
    PreparationMutationReceipt {
        sequence: context.sequence,
        kind,
        returned_at_ms: super::runtime::ticks(),
        api_success,
        os_error: os_error_code,
        readback: states,
        failure,
        native_calls,
    }
}

fn owner_depth(state: &PreparedWindowState, original: &[PreparedWindowState]) -> usize {
    let mut depth = 0;
    let mut owner = state.identity.native_instance.owner_window_handle;
    while owner != 0 && depth < original.len() {
        depth += 1;
        owner = original
            .iter()
            .find(|item| item.identity.window_handle == owner)
            .map_or(0, |item| item.identity.native_instance.owner_window_handle);
    }
    depth
}
