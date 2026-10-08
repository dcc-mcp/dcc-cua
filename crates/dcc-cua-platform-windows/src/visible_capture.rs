use crate::capture_identity::validate_exact_window_owner;
use serde::Serialize;
use thiserror::Error;
use windows::Win32::{
    Foundation::{BOOL, HWND, LPARAM, RECT},
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute},
        Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, RGBQUAD,
            ReleaseDC, SRCCOPY, SelectObject,
        },
    },
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            EnumWindows, GA_ROOT, GetAncestor, GetSystemMetrics, GetWindowRect,
            GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SM_CXVIRTUALSCREEN,
            SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        },
    },
};

pub(super) const MAX_ROOT_WINDOWS: usize = 4_096;

pub(super) struct ThreadDpiAwarenessGuard(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

impl ThreadDpiAwarenessGuard {
    pub(super) fn per_monitor_v2() -> Result<Self, VisibleWindowCaptureError> {
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };

        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.is_null() {
            return Err(capture_error(
                "enter a per-monitor-v2 physical desktop coordinate scope",
            ));
        }
        Ok(Self(previous))
    }
}

impl Drop for ThreadDpiAwarenessGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::HiDpi::SetThreadDpiAwarenessContext;

        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}

#[derive(Debug, Error)]
#[error("visible exact-window capture failed: {message}")]
pub struct VisibleWindowCaptureError {
    message: String,
    pub diagnostic: VisibleWindowCaptureDiagnostic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibleWindowCaptureReason {
    NativeReadFailed,
    TargetUnavailable,
    TargetNotVisible,
    TargetMinimized,
    TargetBoundsInvalid,
    TargetOutsideDesktop,
    RootCloakingUnavailable,
    RootBoundsUnavailable,
    RootBoundsInvalid,
    RootEnumerationIncomplete,
    TargetNotReached,
    TargetBoundsChanged,
    RootOverlap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct VisibleWindowCaptureDiagnostic {
    pub reason: VisibleWindowCaptureReason,
    pub target_bounds: Option<[i32; 4]>,
    pub blocker_process_id: Option<u32>,
    pub blocker_window_handle: Option<u64>,
    pub blocker_bounds: Option<[i32; 4]>,
    pub cloaked: Option<u32>,
    pub os_error: Option<i32>,
    pub root_bounds_failure: Option<RootBoundsFailureDiagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootBoundsRole {
    TargetRoot,
    AboveTargetRoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootBoundsClass {
    Positive,
    ZeroArea,
    Inverted,
    Overflow,
}

/// Failed proof metadata, never an authorization to ignore a root or capture it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RootBoundsFailureDiagnostic {
    pub root_role: RootBoundsRole,
    pub proof_target_root_window_handle: u64,
    pub dwm_raw_rect_edges: [i32; 4],
    pub dwm_classification: RootBoundsClass,
    pub visible: bool,
    pub cloaked: Option<u32>,
    /// The optional Win32 measurement follows the original DWM rejection; it
    /// does not establish simultaneous or stable geometry.
    pub win32_read_after_dwm_rejection: bool,
    pub win32_raw_rect_edges: Option<[i32; 4]>,
    pub win32_classification: Option<RootBoundsClass>,
    pub win32_os_error: Option<i32>,
    /// Only compares zero-area status for monotone, representable rectangles.
    /// Ordinary Win32/DWM border differences are not a geometry failure.
    pub zero_area_status_mismatch: Option<bool>,
}

impl VisibleWindowCaptureDiagnostic {
    fn new(reason: VisibleWindowCaptureReason) -> Self {
        Self {
            reason,
            target_bounds: None,
            blocker_process_id: None,
            blocker_window_handle: None,
            blocker_bounds: None,
            cloaked: None,
            os_error: None,
            root_bounds_failure: None,
        }
    }
}

#[derive(Debug)]
pub struct VisibleWindowCapture {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bounds: [i32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ExactWindowPixelInstanceEvidence {
    pub process_creation_time_100ns: u64,
    pub window_thread_id: u32,
    pub window_class_hash: u64,
    pub owner_window_handle: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactWindowPixelEvidence {
    pub process_id: u32,
    pub window_handle: u64,
    pub bounds: [i32; 4],
    pub visible_bounds: [i32; 4],
    pub dpi: u32,
    pub visible: bool,
    pub minimized: bool,
    pub unobscured: bool,
    pub instance: ExactWindowPixelInstanceEvidence,
}

fn capture_error(message: impl Into<String>) -> VisibleWindowCaptureError {
    proof_error(VisibleWindowCaptureReason::NativeReadFailed, message)
}

fn proof_error(
    reason: VisibleWindowCaptureReason,
    message: impl Into<String>,
) -> VisibleWindowCaptureError {
    VisibleWindowCaptureError {
        message: message.into(),
        diagnostic: VisibleWindowCaptureDiagnostic::new(reason),
    }
}

pub(super) fn exact_window_instance_evidence(
    process_id: u32,
    window_handle: u64,
) -> Result<ExactWindowPixelInstanceEvidence, VisibleWindowCaptureError> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_OWNER, GetClassNameW, GetWindow, GetWindowThreadProcessId,
    };

    let hwnd = window_handle as *mut core::ffi::c_void;
    let mut observed_pid = 0_u32;
    let window_thread_id = unsafe { GetWindowThreadProcessId(hwnd, &mut observed_pid) };
    if window_thread_id == 0 || observed_pid != process_id {
        return Err(capture_error(
            "the exact HWND thread/process identity is unavailable",
        ));
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(capture_error("the exact process instance cannot be opened"));
    }
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let times_ok =
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) != 0 };
    unsafe { CloseHandle(process) };
    if !times_ok {
        return Err(capture_error(
            "the exact process creation time is unavailable",
        ));
    }
    let process_creation_time_100ns =
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);

    let mut class_name = [0_u16; 256];
    let class_len =
        unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if class_len <= 0 {
        return Err(capture_error(
            "the exact HWND class identity is unavailable",
        ));
    }
    let window_class_hash = class_name[..class_len as usize]
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, unit| {
            (hash ^ u64::from(*unit)).wrapping_mul(0x100000001b3)
        });
    let owner_window_handle = unsafe { GetWindow(hwnd, GW_OWNER) } as usize as u64;
    Ok(ExactWindowPixelInstanceEvidence {
        process_creation_time_100ns,
        window_thread_id,
        window_class_hash,
        owner_window_handle,
    })
}

fn rectangles_intersect(left: [i32; 4], right: [i32; 4]) -> bool {
    if left[2] <= 0 || left[3] <= 0 || right[2] <= 0 || right[3] <= 0 {
        return false;
    }
    let left_edge = i64::from(left[0]);
    let left_top = i64::from(left[1]);
    let left_right = left_edge + i64::from(left[2]);
    let left_bottom = left_top + i64::from(left[3]);
    let right_edge = i64::from(right[0]);
    let right_top = i64::from(right[1]);
    let right_right = right_edge + i64::from(right[2]);
    let right_bottom = right_top + i64::from(right[3]);
    left_edge < right_right
        && right_edge < left_right
        && left_top < right_bottom
        && right_top < left_bottom
}

pub(super) fn physical_window_rect(window: HWND) -> Result<RECT, VisibleWindowCaptureError> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut rect).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .map_err(|error| {
        let mut failure = capture_error(format!("read exact physical DWM frame bounds: {error}"));
        failure.diagnostic.os_error = Some(error.code().0);
        failure
    })?;
    Ok(rect)
}

/// Root occluders may be smaller than a usable capture target. Keep every
/// positive-area rectangle in the overlap proof, including one-pixel roots.
pub(crate) fn physical_root_bounds(physical: RECT) -> Option<[i32; 4]> {
    let width = physical.right.checked_sub(physical.left)?;
    let height = physical.bottom.checked_sub(physical.top)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some([physical.left, physical.top, width, height])
}

pub(crate) fn classify_root_bounds(physical: RECT) -> RootBoundsClass {
    let (Some(width), Some(height)) = (
        physical.right.checked_sub(physical.left),
        physical.bottom.checked_sub(physical.top),
    ) else {
        return RootBoundsClass::Overflow;
    };
    if width < 0 || height < 0 {
        RootBoundsClass::Inverted
    } else if width == 0 || height == 0 {
        RootBoundsClass::ZeroArea
    } else {
        RootBoundsClass::Positive
    }
}

fn rect_edges(rect: RECT) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

/// Keep the original strict proof outcome. Read one bounded follow-up only
/// after successful DWM metadata has failed the existing geometric predicate.
pub(crate) fn composited_root_entry(
    window_handle: u64,
    proof_target_root_window_handle: u64,
    physical: RECT,
    visible: bool,
    cloaked: Option<u32>,
    read_win32_after_failure: impl FnOnce() -> Result<RECT, i32>,
) -> Result<(u64, [i32; 4], bool), VisibleWindowCaptureError> {
    root_z_order_entry(window_handle, true, || physical_root_bounds(physical)).ok_or_else(|| {
        let dwm_classification = classify_root_bounds(physical);
        let (win32_raw_rect_edges, win32_classification, win32_os_error) =
            match read_win32_after_failure() {
                Ok(rect) => (
                    Some(rect_edges(rect)),
                    Some(classify_root_bounds(rect)),
                    None,
                ),
                Err(code) => (None, None, Some(code)),
            };
        let zero_area_status_mismatch = match (dwm_classification, win32_classification) {
            (
                RootBoundsClass::Positive | RootBoundsClass::ZeroArea,
                Some(RootBoundsClass::Positive | RootBoundsClass::ZeroArea),
            ) => Some(
                (dwm_classification == RootBoundsClass::ZeroArea)
                    != (win32_classification == Some(RootBoundsClass::ZeroArea)),
            ),
            _ => None,
        };
        let mut error = proof_error(
            VisibleWindowCaptureReason::RootBoundsInvalid,
            "a composited root has empty, inverted, or overflowing physical bounds",
        );
        error.diagnostic.root_bounds_failure = Some(RootBoundsFailureDiagnostic {
            root_role: if window_handle == proof_target_root_window_handle {
                RootBoundsRole::TargetRoot
            } else {
                RootBoundsRole::AboveTargetRoot
            },
            proof_target_root_window_handle,
            dwm_raw_rect_edges: rect_edges(physical),
            dwm_classification,
            visible,
            cloaked,
            win32_read_after_dwm_rejection: true,
            win32_raw_rect_edges,
            win32_classification,
            win32_os_error,
            zero_area_status_mismatch,
        });
        error
    })
}

pub(super) unsafe fn root_or_self(window: HWND) -> HWND {
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    if root.0.is_null() { window } else { root }
}

pub(crate) fn root_z_order_proof(
    target_window_handle: u64,
    target_bounds: [i32; 4],
    roots: &[(u64, [i32; 4], bool)],
) -> Result<(), VisibleWindowCaptureError> {
    for &(window_handle, bounds, visible) in roots {
        if window_handle == target_window_handle {
            if !visible {
                return Err(proof_error(
                    VisibleWindowCaptureReason::TargetNotVisible,
                    "the exact target is not composited on the visible desktop",
                ));
            }
            return if bounds == target_bounds {
                Ok(())
            } else {
                Err(proof_error(
                    VisibleWindowCaptureReason::TargetBoundsChanged,
                    "the exact target bounds changed during root-window enumeration",
                ))
            };
        }
        if visible && rectangles_intersect(bounds, target_bounds) {
            let mut error = proof_error(
                VisibleWindowCaptureReason::RootOverlap,
                "a higher composited root window overlaps the exact target",
            );
            error.diagnostic.blocker_window_handle = Some(window_handle);
            error.diagnostic.blocker_bounds = Some(bounds);
            error.diagnostic.cloaked = Some(0);
            return Err(error);
        }
    }
    Err(proof_error(
        VisibleWindowCaptureReason::TargetNotReached,
        "the exact target was not reached in the complete root-window z-order",
    ))
}

pub(crate) fn root_is_composited(
    visible: bool,
    read_cloaked: impl FnOnce() -> Result<u32, i32>,
) -> Result<bool, VisibleWindowCaptureError> {
    if !visible {
        return Ok(false);
    }
    match read_cloaked() {
        // A successfully measured cloaked window contributes no desktop pixels.
        // Neither process identity nor transparent/no-activate styles prove this.
        Ok(cloaked) => Ok(cloaked == 0),
        Err(code) => {
            let mut error = proof_error(
                VisibleWindowCaptureReason::RootCloakingUnavailable,
                "DWM could not prove whether a visible root is cloaked",
            );
            error.diagnostic.os_error = Some(code);
            Err(error)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootProofDecision {
    Hidden,
    Cloaked,
    Composited,
    Disjoint,
    Overlap,
    TargetReached,
    RejectCloakingQuery,
    RejectBoundsQuery,
    RejectBounds,
}

/// Numeric metadata only. No title, class text, executable path or backend error text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RootProofTraceEntry {
    pub window_handle: u64,
    pub process_id: u32,
    pub thread_id: u32,
    pub root_window_handle: u64,
    pub owner_window_handle: u64,
    pub extended_style: i64,
    pub visible: bool,
    pub minimized: bool,
    pub dpi: u32,
    pub win32_bounds: Option<[i32; 4]>,
    pub win32_bounds_error: Option<i32>,
    pub cloaked: Option<u32>,
    pub cloak_query_error: Option<i32>,
    pub dwm_bounds: Option<[i32; 4]>,
    pub dwm_bounds_error: Option<i32>,
    pub decision: RootProofDecision,
}

impl RootProofTraceEntry {
    unsafe fn read(window: HWND, visible: bool) -> Self {
        use windows::Win32::UI::WindowsAndMessaging::{
            GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW,
        };
        let mut process_id = 0;
        let thread_id = unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
        let mut bounds = RECT::default();
        let bounds_result = unsafe { GetWindowRect(window, &mut bounds) };
        Self {
            window_handle: window.0 as usize as u64,
            process_id,
            thread_id,
            root_window_handle: unsafe { root_or_self(window) }.0 as usize as u64,
            owner_window_handle: unsafe { GetWindow(window, GW_OWNER) }.unwrap_or_default().0
                as usize as u64,
            extended_style: unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as i64,
            visible,
            minimized: unsafe { IsIconic(window) }.as_bool(),
            dpi: unsafe { GetDpiForWindow(window) },
            win32_bounds: bounds_result
                .as_ref()
                .ok()
                .and_then(|_| physical_root_bounds(bounds)),
            win32_bounds_error: bounds_result.err().map(|error| error.code().0),
            cloaked: None,
            cloak_query_error: None,
            dwm_bounds: None,
            dwm_bounds_error: None,
            decision: if visible {
                RootProofDecision::Composited
            } else {
                RootProofDecision::Hidden
            },
        }
    }
}

#[derive(Default)]
pub(super) struct RootZOrderEnumeration {
    target_window_handle: u64,
    target_bounds: [i32; 4],
    roots: Vec<(u64, [i32; 4], bool)>,
    failure: Option<VisibleWindowCaptureError>,
    pub(super) trace: Option<Vec<RootProofTraceEntry>>,
}

unsafe extern "system" fn collect_root_z_order(window: HWND, context: LPARAM) -> BOOL {
    let enumeration = unsafe { &mut *(context.0 as *mut RootZOrderEnumeration) };
    if enumeration.roots.len() >= MAX_ROOT_WINDOWS {
        enumeration.failure = Some(proof_error(
            VisibleWindowCaptureReason::RootEnumerationIncomplete,
            "the bounded root-window enumeration did not reach the exact target",
        ));
        return BOOL(0);
    }
    let visible = unsafe { IsWindowVisible(window) }.as_bool();
    let window_handle = window.0 as usize as u64;
    let mut trace = enumeration
        .trace
        .as_ref()
        .map(|_| unsafe { RootProofTraceEntry::read(window, visible) });
    let measured = (|| {
        let mut measured_cloaked = None;
        let composited = root_is_composited(visible, || {
            let mut cloaked = 0_u32;
            let result = unsafe {
                DwmGetWindowAttribute(
                    window,
                    DWMWA_CLOAKED,
                    (&raw mut cloaked).cast(),
                    std::mem::size_of::<u32>() as u32,
                )
            }
            .map(|()| cloaked)
            .map_err(|error| error.code().0);
            if let Ok(cloaked) = result {
                measured_cloaked = Some(cloaked);
            }
            if let Some(trace) = trace.as_mut() {
                match result {
                    Ok(cloaked) => trace.cloaked = Some(cloaked),
                    Err(error) => {
                        trace.cloak_query_error = Some(error);
                        trace.decision = RootProofDecision::RejectCloakingQuery;
                    }
                }
            }
            result
        })?;
        if !composited {
            if let Some(trace) = trace.as_mut() {
                trace.decision = if visible {
                    RootProofDecision::Cloaked
                } else {
                    RootProofDecision::Hidden
                };
            }
            return Ok((window_handle, [0; 4], false));
        }
        let rect = physical_window_rect(window).map_err(|mut error| {
            error.diagnostic.reason = VisibleWindowCaptureReason::RootBoundsUnavailable;
            if let Some(trace) = trace.as_mut() {
                trace.dwm_bounds_error = error.diagnostic.os_error;
                trace.decision = RootProofDecision::RejectBoundsQuery;
            }
            error
        })?;
        if let Some(trace) = trace.as_mut() {
            trace.dwm_bounds = physical_root_bounds(rect);
            trace.decision = match trace.dwm_bounds {
                None => RootProofDecision::RejectBounds,
                Some(_) if window_handle == enumeration.target_window_handle => {
                    RootProofDecision::TargetReached
                }
                Some(bounds) if rectangles_intersect(bounds, enumeration.target_bounds) => {
                    RootProofDecision::Overlap
                }
                Some(_) => RootProofDecision::Disjoint,
            };
        }
        composited_root_entry(
            window_handle,
            enumeration.target_window_handle,
            rect,
            visible,
            measured_cloaked,
            || {
                let mut win32 = RECT::default();
                unsafe { GetWindowRect(window, &mut win32) }
                    .map(|()| win32)
                    .map_err(|error| error.code().0)
            },
        )
    })();
    if let (Some(entries), Some(trace)) = (enumeration.trace.as_mut(), trace) {
        entries.push(trace);
    }
    let entry = match measured {
        Ok(entry) => entry,
        Err(mut error) => {
            let mut process_id = 0;
            unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
            error.diagnostic.blocker_window_handle = Some(window_handle);
            error.diagnostic.blocker_process_id = (process_id != 0).then_some(process_id);
            enumeration.failure = Some(error);
            return BOOL(0);
        }
    };
    enumeration.roots.push(entry);
    if window_handle == enumeration.target_window_handle {
        BOOL(0)
    } else {
        BOOL(1)
    }
}

pub(crate) fn root_z_order_entry<ReadBounds>(
    window_handle: u64,
    visible: bool,
    read_visible_bounds: ReadBounds,
) -> Option<(u64, [i32; 4], bool)>
where
    ReadBounds: FnOnce() -> Option<[i32; 4]>,
{
    if !visible {
        return Some((window_handle, [0; 4], false));
    }
    let bounds = read_visible_bounds()?;
    (bounds[2] > 0 && bounds[3] > 0).then_some((window_handle, bounds, true))
}

pub(super) unsafe fn enumerate_target_root_proof(
    target: HWND,
    rect: RECT,
    trace: bool,
) -> (
    Result<(), VisibleWindowCaptureError>,
    Vec<RootProofTraceEntry>,
) {
    let target_root = unsafe { root_or_self(target) };
    let target_window_handle = target_root.0 as usize as u64;
    let mut enumeration = RootZOrderEnumeration {
        target_window_handle,
        target_bounds: physical_root_bounds(rect).unwrap_or_default(),
        trace: trace.then(Vec::new),
        ..Default::default()
    };
    let result = unsafe {
        EnumWindows(
            Some(collect_root_z_order),
            LPARAM(&mut enumeration as *mut RootZOrderEnumeration as isize),
        )
    };
    let target_was_reached = enumeration
        .roots
        .last()
        .is_some_and(|(window_handle, _, _)| *window_handle == target_window_handle);
    let proof = if let Some(error) = enumeration.failure {
        Err(error)
    } else if let Err(error) = result
        && !target_was_reached
    {
        let mut failure = proof_error(
            VisibleWindowCaptureReason::RootEnumerationIncomplete,
            "Windows could not enumerate the complete root-window z-order",
        );
        failure.diagnostic.os_error = Some(error.code().0);
        Err(failure)
    } else {
        root_z_order_proof(
            target_window_handle,
            [
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ],
            &enumeration.roots,
        )
    };
    let proof = proof.map_err(|mut error| {
        error.diagnostic.target_bounds = physical_root_bounds(rect);
        if let Some(blocker) = error.diagnostic.blocker_window_handle {
            let mut process_id = 0;
            unsafe {
                GetWindowThreadProcessId(HWND(blocker as usize as *mut _), Some(&mut process_id))
            };
            error.diagnostic.blocker_process_id = (process_id != 0).then_some(process_id);
        }
        error
    });
    (proof, enumeration.trace.unwrap_or_default())
}

unsafe fn prove_target_unobscured(
    target: HWND,
    rect: RECT,
) -> Result<(), VisibleWindowCaptureError> {
    unsafe { enumerate_target_root_proof(target, rect, false) }.0
}

unsafe fn target_is_unobscured(target: HWND, rect: RECT) -> bool {
    unsafe { prove_target_unobscured(target, rect) }.is_ok()
}

pub(crate) fn physical_capture_rect(physical: RECT) -> Result<RECT, VisibleWindowCaptureError> {
    if !physical_root_bounds(physical).is_some_and(|bounds| bounds[2] > 4 && bounds[3] > 4) {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetBoundsInvalid,
            "the exact HWND physical desktop rectangle is invalid",
        ));
    }
    Ok(physical)
}

pub(crate) fn physical_rectangle_within_desktop(target: [i32; 4], desktop: [i32; 4]) -> bool {
    target[2] > 0
        && target[3] > 0
        && desktop[2] > 0
        && desktop[3] > 0
        && target[0] >= desktop[0]
        && target[1] >= desktop[1]
        && i64::from(target[0]) + i64::from(target[2])
            <= i64::from(desktop[0]) + i64::from(desktop[2])
        && i64::from(target[1]) + i64::from(target[3])
            <= i64::from(desktop[1]) + i64::from(desktop[3])
}

/// GetDIBits requires the bitmap to be deselected. Restore even after a failed
/// copy; never publish a partial scanline readback as a complete exact frame.
pub(crate) fn finish_bitmap_readback(
    copied: Result<(), i32>,
    expected_rows: u32,
    restore_bitmap: impl FnOnce() -> bool,
    read_rows: impl FnOnce() -> i32,
    cleanup: impl FnOnce(),
) -> Result<(), VisibleWindowCaptureError> {
    let result = (|| {
        let restored = restore_bitmap();
        if let Err(code) = copied {
            let mut error = capture_error("copy visible window pixels");
            error.diagnostic.os_error = Some(code);
            return Err(error);
        }
        if !restored {
            return Err(capture_error(
                "deselect the exact capture bitmap before readback",
            ));
        }
        if expected_rows == 0 || i64::from(read_rows()) != i64::from(expected_rows) {
            return Err(capture_error("read every exact capture bitmap scanline"));
        }
        Ok(())
    })();
    cleanup();
    result
}

/// Snapshot the native evidence used to fence one exact-window pixel frame.
pub fn exact_window_pixel_evidence(
    process_id: u32,
    window_handle: u64,
) -> Result<ExactWindowPixelEvidence, VisibleWindowCaptureError> {
    let _dpi_scope = ThreadDpiAwarenessGuard::per_monitor_v2()?;
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;
    let raw = usize::try_from(window_handle)
        .map_err(|error| capture_error(format!("convert window handle: {error}")))?;
    let hwnd = HWND(raw as *mut _);
    if hwnd.0.is_null() || !unsafe { IsWindow(hwnd) }.as_bool() {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetUnavailable,
            "the exact HWND no longer exists",
        ));
    }
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|error| capture_error(format!("read exact PMv2 window bounds: {error}")))?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 4 || height <= 4 {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetBoundsInvalid,
            format!("the exact HWND has invalid bounds {width}x{height}"),
        ));
    }
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        return Err(capture_error("the exact HWND DPI is unavailable"));
    }
    let visible_rect = physical_capture_rect(physical_window_rect(hwnd)?)?;
    Ok(ExactWindowPixelEvidence {
        process_id,
        window_handle,
        bounds: [rect.left, rect.top, width, height],
        visible_bounds: [
            visible_rect.left,
            visible_rect.top,
            visible_rect.right - visible_rect.left,
            visible_rect.bottom - visible_rect.top,
        ],
        dpi,
        visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
        minimized: unsafe { IsIconic(hwnd) }.as_bool(),
        unobscured: unsafe { target_is_unobscured(hwnd, visible_rect) },
        instance: exact_window_instance_evidence(process_id, window_handle)?,
    })
}

/// Capture only an exact HWND's currently visible screen rectangle.
///
/// This fallback never asks the target process to paint. It is allowed only
/// when complete root-window z-order proof shows that no higher root covers the
/// target, preventing a desktop crop from being mislabeled as target pixels.
pub fn capture_visible_window(
    process_id: u32,
    window_handle: u64,
) -> Result<VisibleWindowCapture, VisibleWindowCaptureError> {
    let _dpi_scope = ThreadDpiAwarenessGuard::per_monitor_v2()?;
    validate_exact_window_owner(process_id, window_handle)
        .map_err(|error| capture_error(error.to_string()))?;
    let raw = usize::try_from(window_handle)
        .map_err(|error| capture_error(format!("convert window handle: {error}")))?;
    let hwnd = HWND(raw as *mut _);
    if hwnd.0.is_null() || !unsafe { IsWindow(hwnd) }.as_bool() {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetUnavailable,
            "the exact HWND no longer exists",
        ));
    }
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetMinimized,
            "the exact HWND is minimized",
        ));
    }
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetNotVisible,
            "the exact HWND is hidden",
        ));
    }

    let rect = physical_capture_rect(physical_window_rect(hwnd)?)?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 4 || height <= 4 {
        return Err(proof_error(
            VisibleWindowCaptureReason::TargetBoundsInvalid,
            format!("the exact HWND has invalid bounds {width}x{height}"),
        ));
    }
    let bounds = [rect.left, rect.top, width, height];
    let desktop = unsafe {
        [
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        ]
    };
    if !physical_rectangle_within_desktop(bounds, desktop) {
        let mut error = proof_error(
            VisibleWindowCaptureReason::TargetOutsideDesktop,
            "the complete exact HWND rectangle is not inside the physical virtual desktop",
        );
        error.diagnostic.target_bounds = Some(bounds);
        return Err(error);
    }
    unsafe { prove_target_unobscured(hwnd, rect) }?;

    // DWM extended-frame bounds are physical desktop pixels and are not
    // virtualized for the caller's or target's DPI-awareness context. The
    // target and every z-order root above use this same coordinate source, so
    // applying a target-relative conversion would double-scale the crop.
    let physical_rect = physical_capture_rect(rect)?;
    let physical_width = physical_rect.right - physical_rect.left;
    let physical_height = physical_rect.bottom - physical_rect.top;

    unsafe {
        DwmFlush()
            .map_err(|error| capture_error(format!("synchronize desktop compositor: {error}")))?;
        let screen_dc = GetDC(HWND(std::ptr::null_mut()));
        if screen_dc.0.is_null() {
            return Err(capture_error("acquire desktop device context"));
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.0.is_null() {
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(capture_error("create compatible device context"));
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, physical_width, physical_height);
        if bitmap.0.is_null() {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(capture_error("create compatible bitmap"));
        }
        let previous = SelectObject(memory_dc, bitmap);
        if previous.0.is_null() || previous.0 as isize == -1 {
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(memory_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(capture_error("select the exact capture bitmap"));
        }
        let copied = BitBlt(
            memory_dc,
            0,
            0,
            physical_width,
            physical_height,
            screen_dc,
            physical_rect.left,
            physical_rect.top,
            SRCCOPY,
        );
        let mut bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: physical_width,
                biHeight: -physical_height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (physical_width * physical_height * 4) as u32,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let mut bgra = vec![0_u8; (physical_width * physical_height * 4) as usize];
        // https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-getdibits
        // The bitmap must not be selected into a DC when GetDIBits is called.
        let readback = finish_bitmap_readback(
            copied.map_err(|error| error.code().0),
            physical_height as u32,
            || SelectObject(memory_dc, previous).0 == bitmap.0,
            || {
                GetDIBits(
                    memory_dc,
                    bitmap,
                    0,
                    physical_height as u32,
                    Some(bgra.as_mut_ptr().cast()),
                    &mut bitmap_info,
                    DIB_RGB_COLORS,
                )
            },
            || {
                // Destroy the private DC first: a failed restore must not leave
                // the bitmap selected when its object is deleted.
                let _ = DeleteDC(memory_dc);
                let _ = DeleteObject(bitmap);
                ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            },
        );
        readback?;
        validate_exact_window_owner(process_id, window_handle)
            .map_err(|error| capture_error(error.to_string()))?;
        Ok(VisibleWindowCapture {
            bgra,
            width: physical_width as u32,
            height: physical_height as u32,
            bounds: [
                physical_rect.left,
                physical_rect.top,
                physical_width,
                physical_height,
            ],
        })
    }
}
