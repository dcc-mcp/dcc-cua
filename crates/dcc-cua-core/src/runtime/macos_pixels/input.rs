use super::*;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CGMouseButton, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
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

    fn press(&self, pid: i32) -> ComputerUseResult<()> {
        let press_base = || {
            keyboard::press_key(pid, self.key, &self.modifiers)
                .map_err(|_| unavailable("macOS key dispatch failed"))
        };
        if self.modifiers.is_empty() {
            return press_base();
        }
        // Blender's Cocoa event loop updates held modifiers on flagsChanged.
        // Prepare both halves before posting so a failed allocation cannot
        // leave a modifier down. All transitions address the same task PID.
        let source = CGEventSource::new(CGEventSourceStateID::Private)
            .map_err(|_| unavailable("macOS modifier source unavailable"))?;
        let mut flags = CGEventFlags::empty();
        let mut transitions = Vec::with_capacity(self.modifiers.len());
        for modifier in &self.modifiers {
            let (code, flag) = match *modifier {
                "cmd" => (55, CGEventFlags::CGEventFlagCommand),
                "shift" => (56, CGEventFlags::CGEventFlagShift),
                "alt" => (58, CGEventFlags::CGEventFlagAlternate),
                "ctrl" => (59, CGEventFlags::CGEventFlagControl),
                _ => return Err(unavailable("unsupported macOS modifier")),
            };
            let transition = |down, active| {
                let event = CGEvent::new_keyboard_event(source.clone(), code, down)
                    .map_err(|_| unavailable("macOS modifier event unavailable"))?;
                event.set_type(CGEventType::FlagsChanged);
                event.set_flags(active);
                Ok::<_, ComputerUseError>(event)
            };
            let release = transition(false, flags)?;
            flags |= flag;
            transitions.push((transition(true, flags)?, release));
        }
        for (down, _) in &transitions {
            down.post_to_pid(pid);
        }
        let result = press_base();
        for (_, up) in transitions.iter().rev() {
            up.post_to_pid(pid);
        }
        result
    }
}

// Address one AppKit window and post once to its process. A desktop HID
// post can be delivered to another process if focus changes during a drag.
fn pointer_event(
    pid: i32,
    window: u32,
    point: CGPoint,
    kind: CGEventType,
) -> ComputerUseResult<CGEvent> {
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| unavailable("macOS pointer source unavailable"))?;
    let event = CGEvent::new_mouse_event(source, kind, point, CGMouseButton::Left)
        .map_err(|_| unavailable("macOS pointer event unavailable"))?;
    event.set_flags(CGEventFlags::empty());
    event.set_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID, i64::from(pid));
    // AppKit windowNumber and the two Quartz hit-test window fields.
    for field in [
        51,
        EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER,
        EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER_THAT_CAN_HANDLE_THIS_EVENT,
    ] {
        event.set_integer_value_field(field, i64::from(window));
    }
    if !matches!(kind, CGEventType::MouseMoved) {
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 1);
    }
    Ok(event)
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
        let points = if matches!(action.action.as_str(), "move" | "drag" | "click") {
            if !action.modifiers.is_empty() || action.button.as_deref().is_some_and(|b| b != "left")
            {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "macOS pointer input requires the unmodified primary button",
                ));
            }
            crate::runtime::pixel_input_policy::pointer_samples(&action, |x, y| {
                if !(0.0..f64::from(observation.width)).contains(&x)
                    || !(0.0..f64::from(observation.height)).contains(&y)
                {
                    return Err(unavailable("macOS point outside captured window"));
                }
                Ok((
                    x * f64::from(rect[2]) / f64::from(observation.width),
                    y * f64::from(rect[3]) / f64::from(observation.height),
                ))
            })?
        } else {
            Vec::new()
        };
        let mut frame = ExecutionFrame::start(self.pid, self.window, &rect).map_err(frame_error)?;
        let mut attempted = false;
        let mut wait_ms = 0;
        let start = Instant::now();
        let mut release: Option<CGEvent> = None;
        let dragging = action.action == "drag";
        let dispatch = (|| {
            for (index, &(x, y)) in points.iter().enumerate() {
                if dragging && index != 0 {
                    let until = start
                        + Duration::from_millis(
                            action.duration_ms.unwrap_or(500) * index as u64
                                / (points.len() - 1) as u64,
                        );
                    while let Some(left) = until.checked_duration_since(Instant::now()) {
                        std::thread::sleep(left.min(Duration::from_millis(8)));
                        drop(admit()?);
                        frame.check_interrupt().map_err(frame_error)?;
                    }
                }
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
                if action.action == "click" {
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
                    continue;
                }
                let point = CGPoint::new(global_x, global_y);
                if index == 0 {
                    pointer_event(self.pid, self.window, point, CGEventType::MouseMoved)?
                        .post_to_pid(self.pid);
                }
                if dragging {
                    let kind = if index == 0 {
                        CGEventType::LeftMouseDown
                    } else {
                        CGEventType::LeftMouseDragged
                    };
                    let event = pointer_event(self.pid, self.window, point, kind)?;
                    release = Some(pointer_event(
                        self.pid,
                        self.window,
                        point,
                        CGEventType::LeftMouseUp,
                    )?);
                    event.post_to_pid(self.pid);
                    if index + 1 == points.len()
                        && let Some(up) = release.take()
                    {
                        up.post_to_pid(self.pid);
                    }
                }
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
                chord.press(self.pid)?;
            }
            drop(admit()?);
            self.require_input_target(&observation)?;
            frame
                .validate(self.pid, self.window, &rect)
                .map_err(frame_error)
        })();
        // Release a partially delivered press on its original live instance.
        // The event carries that window number and never enters the HID stream.
        if let Some(up) = release
            && process_start(self.pid).is_ok_and(|stamp| stamp == self.process_start)
            && platform_macos::windows::window_info_by_id(self.window)
                .is_some_and(|w| w.pid == self.pid && w.window_id == self.window)
        {
            up.post_to_pid(self.pid);
        }
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
