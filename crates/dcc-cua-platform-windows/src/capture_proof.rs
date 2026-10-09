//! Explicit content-free metadata diagnostic. Never reads pixels or sends UI input.
use serde::Serialize;
use windows::Win32::{
    Foundation::{HWND, POINT, RECT, SIZE},
    Graphics::{Dwm::DwmFlush, Gdi::*},
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

use crate::{
    capture_identity::validate_exact_window_owner,
    display_color::{NativeDisplayColorProof, exact_window_display_color},
    visible_capture::{
        ExactWindowPixelInstanceEvidence, MAX_ROOT_WINDOWS, RootProofTraceEntry,
        ThreadDpiAwarenessGuard, VisibleWindowCaptureDiagnostic, VisibleWindowCaptureReason,
        enumerate_target_root_proof, exact_window_instance_evidence,
        physical_rectangle_within_desktop, physical_root_bounds, physical_window_rect,
        root_or_self,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDpiContext {
    Unavailable,
    Unaware,
    UnawareGdiScaled,
    SystemAware,
    PerMonitorAware,
    PerMonitorAwareV2,
    Other,
}

fn dpi_context() -> NativeDpiContext {
    let context = unsafe { GetThreadDpiAwarenessContext() };
    if context.0.is_null() {
        return NativeDpiContext::Unavailable;
    }
    for (known, value) in [
        (DPI_AWARENESS_CONTEXT_UNAWARE, NativeDpiContext::Unaware),
        (
            DPI_AWARENESS_CONTEXT_UNAWARE_GDISCALED,
            NativeDpiContext::UnawareGdiScaled,
        ),
        (
            DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
            NativeDpiContext::SystemAware,
        ),
        (
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
            NativeDpiContext::PerMonitorAware,
        ),
        (
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            NativeDpiContext::PerMonitorAwareV2,
        ),
    ] {
        if unsafe { AreDpiAwarenessContextsEqual(context, known) }.as_bool() {
            return value;
        }
    }
    NativeDpiContext::Other
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeProofPhase {
    pub elapsed_us: u64,
    pub thread_dpi_context: NativeDpiContext,
    pub target_root_window_handle: u64,
    pub win32_bounds: Option<[i32; 4]>,
    pub win32_bounds_error: Option<i32>,
    pub dwm_bounds: Option<[i32; 4]>,
    pub dwm_bounds_error: Option<i32>,
    pub virtual_desktop: [i32; 4],
    pub target_inside_virtual_desktop: bool,
    pub target_visible: bool,
    pub target_minimized: bool,
    pub target_dpi: u32,
    pub instance: Option<ExactWindowPixelInstanceEvidence>,
    pub proof_passed: bool,
    pub error: Option<VisibleWindowCaptureDiagnostic>,
    pub roots_above_through_target: Vec<RootProofTraceEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeDesktopDcProof {
    pub thread_dpi_context: NativeDpiContext,
    pub acquired: bool,
    pub map_mode: i32,
    pub logical_dpi: [i32; 2],
    pub device_resolution: [i32; 2],
    pub desktop_resolution: [i32; 2],
    pub viewport_origin: Option<[i32; 2]>,
    pub viewport_extent: Option<[i32; 2]>,
    pub window_origin: Option<[i32; 2]>,
    pub window_extent: Option<[i32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeCaptureProof {
    pub schema_version: u32,
    pub process_id: u32,
    pub window_handle: u64,
    pub pixels_read: bool,
    pub input_sent: bool,
    /// A diagnostic is never an authorization or proof of presented pixel ownership.
    pub authorizes_capture_or_input: bool,
    pub max_root_entries_per_phase: usize,
    pub caller_dpi_context: NativeDpiContext,
    pub restored_dpi_context: NativeDpiContext,
    pub dpi_scope_error: Option<VisibleWindowCaptureDiagnostic>,
    pub before_flush: Option<NativeProofPhase>,
    pub flush_error: Option<i32>,
    pub desktop_dc_after_flush: Option<NativeDesktopDcProof>,
    pub after_flush: Option<NativeProofPhase>,
    pub target_evidence_stable: bool,
    pub display_color: Option<NativeDisplayColorProof>,
}

fn diagnostic(reason: VisibleWindowCaptureReason) -> VisibleWindowCaptureDiagnostic {
    VisibleWindowCaptureDiagnostic {
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

fn desktop_bounds() -> [i32; 4] {
    unsafe {
        [
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        ]
    }
}

fn phase(process_id: u32, hwnd: HWND, start: std::time::Instant) -> NativeProofPhase {
    let window_handle = hwnd.0 as usize as u64;
    let mut phase = NativeProofPhase {
        elapsed_us: start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        thread_dpi_context: dpi_context(),
        target_root_window_handle: 0,
        win32_bounds: None,
        win32_bounds_error: None,
        dwm_bounds: None,
        dwm_bounds_error: None,
        virtual_desktop: desktop_bounds(),
        target_inside_virtual_desktop: false,
        target_visible: false,
        target_minimized: false,
        target_dpi: 0,
        instance: None,
        proof_passed: false,
        error: None,
        roots_above_through_target: Vec::new(),
    };
    if validate_exact_window_owner(process_id, window_handle).is_err() {
        phase.error = Some(diagnostic(VisibleWindowCaptureReason::TargetUnavailable));
        return phase;
    }
    phase.target_root_window_handle = unsafe { root_or_self(hwnd) }.0 as usize as u64;
    phase.target_visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    phase.target_minimized = unsafe { IsIconic(hwnd) }.as_bool();
    phase.target_dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut rect = RECT::default();
    match unsafe { GetWindowRect(hwnd, &mut rect) } {
        Ok(()) => phase.win32_bounds = physical_root_bounds(rect),
        Err(error) => phase.win32_bounds_error = Some(error.code().0),
    }
    let rect = match physical_window_rect(hwnd) {
        Ok(rect) => rect,
        Err(error) => {
            phase.dwm_bounds_error = error.diagnostic.os_error;
            phase.error = Some(*error.diagnostic);
            return phase;
        }
    };
    phase.dwm_bounds = physical_root_bounds(rect);
    if let Some(bounds) = phase.dwm_bounds {
        phase.target_inside_virtual_desktop =
            physical_rectangle_within_desktop(bounds, phase.virtual_desktop);
    } else {
        phase.error = Some(diagnostic(VisibleWindowCaptureReason::TargetBoundsInvalid));
        return phase;
    }
    match exact_window_instance_evidence(process_id, window_handle) {
        Ok(instance) => phase.instance = Some(instance),
        Err(error) => {
            phase.error = Some(*error.diagnostic);
            return phase;
        }
    }
    let (proof, roots) = unsafe { enumerate_target_root_proof(hwnd, rect, true) };
    phase.roots_above_through_target = roots;
    phase.proof_passed = proof.is_ok();
    phase.error = proof.err().map(|error| *error.diagnostic);
    // Diagnostics still enumerate hidden/minimized/out-of-desktop targets, but never label them usable.
    if phase.error.is_none() {
        let reason = if phase.target_minimized {
            Some(VisibleWindowCaptureReason::TargetMinimized)
        } else if !phase.target_visible {
            Some(VisibleWindowCaptureReason::TargetNotVisible)
        } else if phase
            .dwm_bounds
            .is_some_and(|bounds| bounds[2] <= 4 || bounds[3] <= 4)
        {
            Some(VisibleWindowCaptureReason::TargetBoundsInvalid)
        } else if !phase.target_inside_virtual_desktop {
            Some(VisibleWindowCaptureReason::TargetOutsideDesktop)
        } else {
            None
        };
        if let Some(reason) = reason {
            phase.proof_passed = false;
            phase.error = Some(diagnostic(reason));
        }
    }
    phase
}

fn desktop_dc() -> NativeDesktopDcProof {
    let mut proof = NativeDesktopDcProof {
        thread_dpi_context: dpi_context(),
        acquired: false,
        map_mode: 0,
        logical_dpi: [0; 2],
        device_resolution: [0; 2],
        desktop_resolution: [0; 2],
        viewport_origin: None,
        viewport_extent: None,
        window_origin: None,
        window_extent: None,
    };
    unsafe {
        let dc = GetDC(HWND::default());
        if dc.0.is_null() {
            return proof;
        }
        proof.acquired = true;
        proof.thread_dpi_context = dpi_context();
        proof.map_mode = GetMapMode(dc).0;
        proof.logical_dpi = [GetDeviceCaps(dc, LOGPIXELSX), GetDeviceCaps(dc, LOGPIXELSY)];
        proof.device_resolution = [GetDeviceCaps(dc, HORZRES), GetDeviceCaps(dc, VERTRES)];
        proof.desktop_resolution = [
            GetDeviceCaps(dc, DESKTOPHORZRES),
            GetDeviceCaps(dc, DESKTOPVERTRES),
        ];
        let mut point = POINT::default();
        let mut size = SIZE::default();
        if GetViewportOrgEx(dc, &mut point).as_bool() {
            proof.viewport_origin = Some([point.x, point.y]);
        }
        if GetViewportExtEx(dc, &mut size).as_bool() {
            proof.viewport_extent = Some([size.cx, size.cy]);
        }
        if GetWindowOrgEx(dc, &mut point).as_bool() {
            proof.window_origin = Some([point.x, point.y]);
        }
        if GetWindowExtEx(dc, &mut size).as_bool() {
            proof.window_extent = Some([size.cx, size.cy]);
        }
        ReleaseDC(HWND::default(), dc);
    }
    proof
}

pub(crate) fn target_evidence_stable(before: &NativeProofPhase, after: &NativeProofPhase) -> bool {
    before.instance.is_some()
        && before.instance == after.instance
        && before.target_root_window_handle == after.target_root_window_handle
        && before.win32_bounds.is_some()
        && before.win32_bounds == after.win32_bounds
        && before.dwm_bounds.is_some()
        && before.dwm_bounds == after.dwm_bounds
        && before.target_dpi != 0
        && before.target_dpi == after.target_dpi
        && before.target_visible == after.target_visible
        && before.target_minimized == after.target_minimized
        && before.virtual_desktop == after.virtual_desktop
        && before.thread_dpi_context == NativeDpiContext::PerMonitorAwareV2
        && after.thread_dpi_context == NativeDpiContext::PerMonitorAwareV2
}

/// No UIA/Host startup, activation, GUI creation, input or pixel copying. Explicit diagnostic only.
pub fn native_capture_proof(process_id: u32, window_handle: u64) -> NativeCaptureProof {
    let caller = dpi_context();
    let mut report = NativeCaptureProof {
        schema_version: 1,
        process_id,
        window_handle,
        pixels_read: false,
        input_sent: false,
        authorizes_capture_or_input: false,
        max_root_entries_per_phase: MAX_ROOT_WINDOWS,
        caller_dpi_context: caller,
        restored_dpi_context: caller,
        dpi_scope_error: None,
        before_flush: None,
        flush_error: None,
        desktop_dc_after_flush: None,
        after_flush: None,
        target_evidence_stable: false,
        display_color: None,
    };
    let guard = match ThreadDpiAwarenessGuard::per_monitor_v2() {
        Ok(guard) => guard,
        Err(error) => {
            report.dpi_scope_error = Some(*error.diagnostic);
            return report;
        }
    };
    let start = std::time::Instant::now();
    let hwnd = HWND(window_handle as usize as *mut _);
    let before = phase(process_id, hwnd, start);
    // Mirror the current production ordering. No BitBlt/GetDIBits occurs in this diagnostic.
    report.flush_error = unsafe { DwmFlush() }.err().map(|error| error.code().0);
    report.desktop_dc_after_flush = Some(desktop_dc());
    let after = phase(process_id, hwnd, start);
    report.target_evidence_stable = target_evidence_stable(&before, &after);
    report.before_flush = Some(before);
    report.after_flush = Some(after);
    report.display_color = Some(exact_window_display_color(process_id, window_handle));
    drop(guard);
    report.restored_dpi_context = dpi_context();
    report
}

#[cfg(test)]
mod tests;
