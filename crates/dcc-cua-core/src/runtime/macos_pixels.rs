//! Exact CGWindowID capture using the embedded Try CUA backend.
use super::unix_pixels::{CAPTURE_BACKEND, Handle};
use super::*;
use std::mem::{MaybeUninit, size_of};
use std::sync::Mutex;
mod input;

pub(super) struct MacPixels {
    scope: ComputerUseTargetScope,
    pid: i32,
    window: u32,
    process_start: (u64, u64),
    authorization: Option<(String, Arc<crate::NativeTaskAuthorization>)>,
    generation: u64,
    usable: bool,
    pub(super) input_lease: Option<Arc<crate::RecordingAuthorization>>,
}

fn invalid(message: &str) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, message)
}

fn process_start(pid: i32) -> ComputerUseResult<(u64, u64)> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = size_of::<libc::proc_bsdinfo>() as i32;
    // proc_pidinfo writes up to the supplied buffer size. Read its fields only
    // after the kernel reports a complete BSD info record for this PID.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return Err(invalid("macOS process instance is unavailable"));
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid as u32 {
        return Err(invalid("macOS process identity changed"));
    }
    Ok((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

impl MacPixels {
    pub(super) fn bind(
        scope: ComputerUseTargetScope,
        authorization: Option<(String, Arc<crate::NativeTaskAuthorization>)>,
    ) -> ComputerUseResult<(Handle, WindowTarget)> {
        let pid = scope
            .process_id
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| invalid("macOS capture requires an explicit PID"))?;
        let window = scope
            .window_handle
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| invalid("macOS capture requires an explicit CGWindowID"))?;
        let bound = Self {
            scope,
            pid,
            window,
            process_start: process_start(pid)?,
            authorization,
            generation: 0,
            usable: true,
            input_lease: None,
        };
        let target = bound.evidence()?;
        Ok((Arc::new(Mutex::new(bound)), target))
    }

    fn evidence(&self) -> ComputerUseResult<WindowTarget> {
        if !self.usable || process_start(self.pid)? != self.process_start {
            return Err(invalid("the retained macOS process instance changed"));
        }
        let window = platform_macos::windows::window_info_by_id(self.window)
            .filter(|window| window.pid == self.pid && window.window_id == self.window)
            .ok_or_else(|| {
                invalid("the selected CGWindowID is no longer owned by the bound PID")
            })?;
        if !window.is_on_screen || window.on_current_space == Some(false) || window.layer != 0 {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::TargetUnavailable,
                "exact capture requires a normal on-screen macOS application window",
            ));
        }
        let rect = [
            window.bounds.x,
            window.bounds.y,
            window.bounds.width,
            window.bounds.height,
        ];
        if rect.iter().any(|v| {
            !v.is_finite()
                || *v < f64::from(i32::MIN)
                || *v > f64::from(i32::MAX)
                || v.fract() != 0.
        }) || rect[2] <= 0.
            || rect[3] <= 0.
            || rect[2] * rect[3] > 16_777_216.
        {
            return Err(invalid("unsupported macOS window bounds"));
        }
        let target = WindowTarget {
            pid: self.pid as u32,
            window_id: u64::from(self.window),
            title: window.title,
            app_name: window.app_name,
            bounds: rect.map(|v| v as i32),
            is_on_screen: true,
            is_minimized: false,
            z_index: None,
            is_foreground: false,
        };
        if !self.scope.matches(&target) {
            return Err(invalid(
                "the macOS window no longer matches its original scope",
            ));
        }
        if let Some((session, authorization)) = &self.authorization {
            authorization.validate(session, target.pid, target.window_id)?;
        }
        crate::window_target::validate_target_policy_with_authorization(
            &target,
            self.authorization
                .as_ref()
                .map_or("", |(session, _)| session.as_str()),
            self.authorization
                .as_ref()
                .map(|(_, authorization)| authorization.as_ref()),
        )?;
        Ok(target)
    }

    pub(super) fn target(&mut self) -> ComputerUseResult<WindowTarget> {
        let result = self.evidence();
        if result.is_err() {
            self.usable = false;
        }
        result
    }

    pub(super) fn capture(&mut self) -> ComputerUseResult<(Vec<u8>, WindowTarget, Value)> {
        let before = self.target()?;
        if !platform_macos::permissions::status::screen_recording_granted() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "macOS Screen Recording permission is required for exact-window capture",
            ));
        }
        if let Some(reason) = platform_macos::capture_consent::capture_blocked_reason() {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                reason,
            ));
        }
        let data = platform_macos::capture::screenshot_window_bytes(self.window).map_err(|_| {
            ComputerUseError::new(
                ComputerUseErrorCode::CaptureFailed,
                "macOS exact-window capture returned no image",
            )
        })?;
        let after = self.target()?;
        if before != after {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "macOS window changed during capture",
            ));
        }
        self.generation += 1;
        let mut provenance = json!({"backend":CAPTURE_BACKEND,"scope":"window","observation_mode":"pixels_only",
            "pixels_captured":true,"read_only":self.input_lease.is_none(),
            "input_available":self.input_lease.is_some(),"activation_available":false,
            "accessibility_available":false,
            "accessibility_granted":platform_macos::permissions::status::accessibility_granted(),
            "degraded":false,"whole_desktop_capture":false,
            "process_id":after.pid,"window_handle":after.window_id,"native_window_bounds":after.bounds,
            "native_bounds_units":"display_points","coordinate_units":"image_pixels",
            "process_start":self.process_start,"capture_generation":self.generation});
        if self.input_lease.is_some() {
            let (exposure, blocker) = dcc_cua_indicator::macos_frame::exposure_diagnostic(
                self.pid,
                self.window,
                &after.bounds,
            );
            provenance["input_exposure"] = json!(exposure);
            if let Some(blocker) = blocker {
                provenance["input_blocker"] = json!(blocker);
            }
            provenance["exact_ax_focus"] = json!(
                platform_macos::ax::bindings::focused_window_id_of_pid(self.pid)
                    .is_some_and(|id| id.own == self.window && id.root == self.window)
            );
        }
        Ok((data, after, provenance))
    }
}
