use super::*;
use dcc_cua_indicator::macos_frame::{ExecutionFrame, require_exposure};
use platform_macos::input::{keyboard, mouse};
use std::sync::atomic::AtomicBool;

fn unavailable(message: &str) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::TargetUnavailable, message)
}

fn frame_error(error: std::io::Error) -> ComputerUseError {
    if error.kind() == std::io::ErrorKind::Interrupted {
        dcc_cua_interrupt::broadcast_interrupt();
        ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, error.to_string())
    } else {
        ComputerUseError::new(ComputerUseErrorCode::BackendUnavailable, error.to_string())
    }
}

struct Chord<'a> {
    key: &'a str,
    modifiers: Vec<&'a str>,
}

impl<'a> Chord<'a> {
    fn parse(step: &'a str) -> ComputerUseResult<Self> {
        let mut modifiers = step.split('+').collect::<Vec<_>>();
        let key = modifiers.pop().unwrap_or_default();
        let supported = (key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric())
            || matches!(
                key,
                "return"
                    | "enter"
                    | "space"
                    | "tab"
                    | "left"
                    | "right"
                    | "up"
                    | "down"
                    | "home"
                    | "end"
                    | "backspace"
                    | "delete"
            )
            || key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=12).contains(&n));
        if !supported
            || modifiers.iter().enumerate().any(|(i, m)| {
                !matches!(*m, "ctrl" | "shift" | "alt" | "cmd") || modifiers[..i].contains(m)
            })
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::InvalidAction,
                "unsupported macOS key or modifier",
            ));
        }
        Ok(Self { key, modifiers })
    }
}

impl MacPixels {
    fn require_input_target(
        &mut self,
        observation: &ComputerUseObservation,
    ) -> ComputerUseResult<()> {
        let target = self.target()?;
        if target.bounds != observation.source_rect
            || target.pid != observation.process_id
            || target.window_id != observation.window_handle
            || observation.capture_provenance["capture_generation"].as_u64()
                != Some(self.generation)
        {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "macOS input observation changed",
            ));
        }
        if !platform_macos::permissions::status::accessibility_granted()
            || !platform_macos::permissions::status::screen_recording_granted()
        {
            return Err(unavailable(
                "macOS input requires Accessibility and Screen Recording permission",
            ));
        }
        let focused = platform_macos::ax::bindings::focused_window_id_of_pid(self.pid);
        if !focused.is_some_and(|id| id.own == self.window && id.root == self.window) {
            return Err(unavailable("the exact macOS window lacks AX focus"));
        }
        Ok(())
    }

    pub(in crate::runtime) fn guarded_action(
        &mut self,
        action: ComputerUseAction,
        observation: ComputerUseObservation,
        deadline: Instant,
        generation: u64,
        cancelled: Arc<AtomicBool>,
    ) -> ComputerUseResult<ComputerUseToolResult> {
        let lease = self
            .input_lease
            .clone()
            .ok_or_else(|| unavailable("missing macOS input lease"))?;
        let admit = || {
            super::super::pixel_input_policy::validate_pixel_interrupt(
                generation,
                dcc_cua_interrupt::interrupt_generation(),
                cancelled.load(Ordering::Acquire),
            )?;
            if Instant::now() >= deadline {
                return Err(unavailable("macOS input deadline expired"));
            }
            lease
                .admit()
                .map_err(|_| unavailable("macOS input lease expired or was revoked"))
        };
        drop(admit()?);
        self.require_input_target(&observation)?;
        let rect = observation.source_rect;
        require_exposure(self.pid, self.window, &rect, 0).map_err(unavailable)?;
        let normalized = action
            .keys
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let chords = normalized
            .iter()
            .map(|s| Chord::parse(s))
            .collect::<ComputerUseResult<Vec<_>>>()?;
        let click = if action.action == "click" {
            if !action.modifiers.is_empty() || action.button.as_deref().is_some_and(|b| b != "left")
            {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "macOS pixel click requires the unmodified primary button",
                ));
            }
            let coordinate = |value: Option<f64>, pixels: u32, points: i32| {
                value
                    .filter(|v| v.is_finite() && *v >= 0. && *v < f64::from(pixels))
                    .map(|v| v * f64::from(points) / f64::from(pixels))
                    .ok_or_else(|| unavailable("macOS click outside captured image"))
            };
            Some((
                coordinate(action.x, observation.width, rect[2])?,
                coordinate(action.y, observation.height, rect[3])?,
            ))
        } else {
            None
        };
        let mut frame = ExecutionFrame::start(self.pid, self.window, &rect).map_err(frame_error)?;
        let mut attempted = false;
        let mut wait_ms = 0;
        let start = Instant::now();
        let dispatch = (|| {
            if let Some((x, y)) = click {
                self.require_input_target(&observation)?;
                frame
                    .validate(self.pid, self.window, &rect)
                    .map_err(frame_error)?;
                let global_x = f64::from(rect[0]) + x;
                let global_y = f64::from(rect[1]) + y;
                if !dcc_cua_indicator::macos_frame::point_hits_window(
                    self.window,
                    global_x,
                    global_y,
                ) {
                    return Err(unavailable(
                        "macOS click point does not hit the bound window",
                    ));
                }
                let _admission = admit()?;
                attempted = true;
                mouse::click_at_xy_with_window_local(
                    self.pid,
                    global_x,
                    global_y,
                    x,
                    y,
                    self.window,
                    1,
                    &[],
                    mouse::WindowClickDelivery::Foreground,
                )
                .map_err(|_| unavailable("macOS click dispatch failed"))?;
            }
            for (index, chord) in chords.iter().enumerate() {
                if index > 0 {
                    let before = Instant::now();
                    let until = before + Duration::from_millis(action.delay_ms.unwrap_or(0));
                    while let Some(left) = until.checked_duration_since(Instant::now()) {
                        std::thread::sleep(left.min(Duration::from_millis(8)));
                        drop(admit()?);
                        frame.check_interrupt().map_err(frame_error)?;
                    }
                    wait_ms += before.elapsed().as_millis() as u64;
                }
                self.require_input_target(&observation)?;
                frame
                    .validate(self.pid, self.window, &rect)
                    .map_err(frame_error)?;
                let _admission = admit()?;
                attempted = true;
                keyboard::press_key(self.pid, chord.key, &chord.modifiers)
                    .map_err(|_| unavailable("macOS key dispatch failed"))?;
            }
            drop(admit()?);
            self.require_input_target(&observation)?;
            frame
                .validate(self.pid, self.window, &rect)
                .map_err(frame_error)
        })();
        if let Err(error) = dispatch {
            return Err(error.with_details(ComputerUseErrorDetails {
                action_attempted: Some(attempted),
                input_sent: Some(if attempted {
                    ComputerUseInputState::Unknown
                } else {
                    ComputerUseInputState::NotSent
                }),
                completion: Some(if attempted {
                    ComputerUseCompletionState::Unknown
                } else {
                    ComputerUseCompletionState::Known
                }),
                effect_unknown: Some(attempted),
                blind_retry: Some(false),
                fresh_observation_required: Some(true),
                ..Default::default()
            }));
        }
        Ok(ComputerUseToolResult {
            status: ComputerUseToolStatus::Succeeded,
            degraded: false,
            images: Vec::new(),
            text: "Submitted input to the exact foreground macOS window.".into(),
            value: json!({"success":true,"route":"macos_exact_foreground","target":{"process_id":self.pid,"window_handle":self.window},
                "effect":"unverifiable","verification_required":true,"fresh_observation_required":true,
                "delivery":{"delivery_completed":true,"post_dispatch_validated":true,
                    "key_sequence_timing":(!chords.is_empty()).then(||json!({"steps":chords.len(),"dispatch_ms":start.elapsed().as_millis() as u64,"inter_step_wait_ms":wait_ms}))}}),
        })
    }
}
