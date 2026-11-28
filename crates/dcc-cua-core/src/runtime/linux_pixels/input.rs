mod pointer;
use super::execution_frame::ExecutionFrame;
use super::*;
use std::sync::atomic::AtomicBool;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, KeyPressEvent,
};

fn invalid(message: &str) -> ComputerUseError {
    refused(ComputerUseErrorCode::InvalidAction, message)
}

#[derive(PartialEq)]
struct Keymap {
    first: u8,
    stride: usize,
    symbols: Vec<u32>,
    modifiers: Vec<u8>,
}

impl Keymap {
    fn read(bound: &LinuxPixels) -> ComputerUseResult<Self> {
        let setup = bound.connection.setup();
        let first = setup.min_keycode;
        let count = setup
            .max_keycode
            .checked_sub(first)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| invalid("invalid X11 keycode range"))?;
        let map = bound
            .connection
            .get_keyboard_mapping(first, count)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if map.keysyms_per_keycode == 0
            || map.keysyms.len() != usize::from(count) * usize::from(map.keysyms_per_keycode)
        {
            return Err(invalid("incomplete X11 keyboard mapping"));
        }
        let modifiers = bound
            .connection
            .get_modifier_mapping()
            .map_err(native)?
            .reply()
            .map_err(native)?
            .keycodes;
        Ok(Self {
            first,
            stride: usize::from(map.keysyms_per_keycode),
            symbols: map.keysyms,
            modifiers,
        })
    }

    fn code(&self, symbol: u32) -> ComputerUseResult<u8> {
        self.symbols
            .chunks_exact(self.stride)
            .position(|group| group[0] == symbol)
            .and_then(|index| self.first.checked_add(u8::try_from(index).ok()?))
            .ok_or_else(|| invalid("key is unavailable in the active X11 base layout"))
    }

    fn chord(&self, step: &str) -> ComputerUseResult<Vec<(u8, u16)>> {
        let mut parts = step.split('+').map(str::trim).collect::<Vec<_>>();
        let key = parts.pop().unwrap_or_default().to_ascii_uppercase();
        let sym = match key.as_str() {
            "ENTER" | "RETURN" => 0xff0d,
            "TAB" => 0xff09,
            "SPACE" => 0x20,
            "LEFT" => 0xff51,
            "UP" => 0xff52,
            "RIGHT" => 0xff53,
            "DOWN" => 0xff54,
            "HOME" => 0xff50,
            "END" => 0xff57,
            "DELETE" => 0xffff,
            "BACKSPACE" => 0xff08,
            s if s.len() == 1 && s.is_ascii() && s.as_bytes()[0].is_ascii_alphanumeric() => {
                u32::from(s.as_bytes()[0].to_ascii_lowercase())
            }
            s => s
                .strip_prefix('F')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=24).contains(n))
                .map(|n| 0xffbd + n)
                .ok_or_else(|| invalid("unsupported X11 sequence key"))?,
        };
        let mut keys = Vec::with_capacity(parts.len() + 1);
        for modifier in parts {
            let (symbol, mask) = match modifier.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => (0xffe3, 4),
                "SHIFT" => (0xffe1, 1),
                "ALT" => (0xffe9, 8),
                _ => return Err(invalid("unsupported X11 sequence modifier")),
            };
            if keys.iter().any(|&(_, m)| m == mask) {
                return Err(invalid("duplicate X11 modifier"));
            }
            let code = self.code(symbol)?;
            let slot = u16::trailing_zeros(mask) as usize;
            let width = self.modifiers.len() / 8;
            if width == 0 || !self.modifiers[slot * width..(slot + 1) * width].contains(&code) {
                return Err(invalid("unsupported X11 modifier mapping"));
            }
            keys.push((code, mask));
        }
        keys.push((self.code(sym)?, 0));
        Ok(keys)
    }
}

struct InputGuard<'a> {
    deadline: Instant,
    generation: u64,
    cancelled: &'a AtomicBool,
    observation: &'a ComputerUseObservation,
}

// Fence only a single press/release group. Delays, Escape polling on its
// own client, and application state readback run outside the server grab.
struct DispatchFence(Arc<Connection>);

impl Drop for DispatchFence {
    fn drop(&mut self) {
        let _ = self.0.ungrab_server();
        let _ = self.0.flush();
    }
}

impl LinuxPixels {
    fn dispatch_fence(
        &mut self,
        frame: &ExecutionFrame,
        guard: &InputGuard<'_>,
    ) -> ComputerUseResult<DispatchFence> {
        frame.check_escape()?;
        let cookie = self.connection.grab_server().map_err(native)?;
        let fence = DispatchFence(Arc::clone(&self.connection));
        cookie.check().map_err(native)?;
        self.validate_input(guard)?;
        frame.require_exposed(self)?;
        Ok(fence)
    }

    fn pointer_destination(&self, root_x: i16, root_y: i16) -> ComputerUseResult<(u32, i16, i16)> {
        // Hit-test the stacked X11 window hierarchy at the requested screen
        // point before moving the physical cursor. An input-only overlay can
        // intercept the pointer without contributing any captured pixels.
        let mut parent = self.root;
        let mut in_target = false;
        let client_bits = !self.connection.setup().resource_id_mask;
        let owner = self.window & client_bits;
        for _ in 0..32 {
            in_target |= parent == self.window;
            let location = self
                .connection
                .translate_coordinates(self.root, parent, root_x, root_y)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            if !location.same_screen {
                break;
            }
            if location.child == 0 {
                if in_target {
                    return Ok((parent, location.dst_x, location.dst_y));
                }
                break;
            }
            // The server assigns each client a disjoint XID range. A native
            // child must belong to the exact target's creating connection.
            if in_target && (owner == 0 || location.child & client_bits != owner) {
                break;
            }
            parent = location.child;
        }
        Err(refused(
            ComputerUseErrorCode::TargetUnavailable,
            "X11 pointer destination is outside the exact client",
        ))
    }

    fn admit_input(
        &self,
        guard: &InputGuard<'_>,
    ) -> ComputerUseResult<dcc_cua_showcase::RecordingAdmission<'_>> {
        self.begin()?;
        if Instant::now() >= guard.deadline {
            return Err(invalid("the retained X11 input lease is unavailable"));
        }
        super::super::pixel_input_policy::validate_pixel_interrupt(
            guard.generation,
            dcc_cua_interrupt::interrupt_generation(),
            guard.cancelled.load(Ordering::Acquire),
        )?;
        self.input_lease
            .as_ref()
            .and_then(|lease| lease.admit().ok())
            .ok_or_else(|| invalid("the retained X11 input lease is unavailable"))
    }

    fn validate_input(&mut self, guard: &InputGuard<'_>) -> ComputerUseResult<()> {
        drop(self.admit_input(guard)?);
        let target = self.target()?;
        if target.bounds != guard.observation.source_rect
            || target.pid != guard.observation.process_id
            || target.window_id != guard.observation.window_handle
            || guard.observation.capture_provenance["capture_generation"].as_u64()
                != Some(self.generation)
        {
            return Err(refused(
                ComputerUseErrorCode::StaleObservation,
                "X11 input geometry or instance changed",
            ));
        }
        crate::window_target::validate_target_policy_with_authorization(
            &target,
            self.native_task_authorization
                .as_ref()
                .map_or("", |(s, _)| s.as_str()),
            self.native_task_authorization
                .as_ref()
                .map(|(_, a)| a.as_ref()),
        )?;
        if !target.is_foreground {
            return Err(refused(
                ComputerUseErrorCode::TargetUnavailable,
                "the exact X11 window lacks keyboard focus",
            ));
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
        let guard = InputGuard {
            deadline,
            generation,
            cancelled: &cancelled,
            observation: &observation,
        };
        self.validate_input(&guard)?;
        let map = Keymap::read(self)?;
        let steps = action
            .keys
            .iter()
            .map(|s| map.chord(s))
            .collect::<ComputerUseResult<Vec<_>>>()?;
        let mut attempted = false;
        let mut wait_ms = 0;
        let start = Instant::now();
        let dispatch = (|| {
            let frame = ExecutionFrame::show(self, observation.source_rect, map.code(0xff1b)?)?;
            self.validate_input(&guard)?;
            let pointer = self
                .connection
                .query_pointer(self.window)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let key_state = self
                .connection
                .query_keymap()
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let held_modifier = map
                .modifiers
                .iter()
                .copied()
                .filter(|&key| key != 0)
                .any(|key| key_state.keys[usize::from(key / 8)] & (1 << (key % 8)) != 0);
            // A latched core Lock modifier does not imply a physically held key.
            if !pointer.same_screen || u16::from(pointer.mask) & !2 != 0 || held_modifier {
                return Err(invalid(
                    "X11 input requires an unmodified pointer on the same display",
                ));
            }
            let coords = (pointer.win_x, pointer.win_y);
            let [x, y, _, _] = observation.source_rect;
            let base = KeyPressEvent {
                time: frame.timestamp(),
                event: self.window,
                root: self.root,
                child: 0,
                event_x: coords.0,
                event_y: coords.1,
                root_x: i16::try_from(x + i32::from(coords.0)).map_err(native)?,
                root_y: i16::try_from(y + i32::from(coords.1)).map_err(native)?,
                same_screen: true,
                ..Default::default()
            };
            if matches!(action.action.as_str(), "move" | "click" | "drag") {
                self.pointer_action(&action, base, &guard, &frame, &mut attempted)?;
            } else {
                for (index, chord) in steps.iter().enumerate() {
                    if index != 0 {
                        let before = Instant::now();
                        let until = before + Duration::from_millis(action.delay_ms.unwrap_or(0));
                        while let Some(remaining) = until.checked_duration_since(Instant::now()) {
                            std::thread::sleep(remaining.min(Duration::from_millis(8)));
                            drop(self.admit_input(&guard)?);
                            frame.check_escape()?;
                        }
                        wait_ms += before.elapsed().as_millis() as u64;
                    }
                    let _fence = self.dispatch_fence(&frame, &guard)?;
                    if Keymap::read(self)? != map {
                        return Err(invalid(
                            "the X11 keyboard mapping changed during the sequence",
                        ));
                    }
                    let mut state = 0;
                    let _admission = self.admit_input(&guard)?;
                    attempted = true;
                    for &(code, mask) in chord {
                        self.send_event(
                            KeyPressEvent {
                                time: frame.timestamp(),
                                ..base
                            },
                            KEY_PRESS_EVENT,
                            code,
                            state,
                        )?;
                        state |= mask;
                    }
                    for &(code, mask) in chord.iter().rev() {
                        self.send_event(
                            KeyPressEvent {
                                time: frame.timestamp(),
                                ..base
                            },
                            KEY_RELEASE_EVENT,
                            code,
                            state,
                        )?;
                        state &= !mask;
                    }
                }
            }
            self.validate_input(&guard)?;
            frame.validate(self)
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
            text: "Submitted input to the exact foreground X11 window.".into(),
            value: json!({"success":true,"route":"x11_exact_window_events","target":{"process_id":self.pid,"window_handle":self.window},
                "effect":"unverifiable","verification_required":true,"fresh_observation_required":true,
                "delivery":{"delivery_completed":true,"post_dispatch_validated":true,
                    "key_sequence_timing":(!steps.is_empty()).then(||json!({"steps":steps.len(),"dispatch_ms":start.elapsed().as_millis() as u64,
                        "inter_step_wait_ms":wait_ms}))}}),
        })
    }

    // NO_EVENT targets the exact window creator. Simulated modifiers never
    // become physical desktop key state.
    fn send_event(
        &self,
        base: KeyPressEvent,
        kind: u8,
        code: u8,
        state: u16,
    ) -> ComputerUseResult<()> {
        self.connection
            .send_event(
                false,
                base.event,
                EventMask::NO_EVENT,
                KeyPressEvent {
                    response_type: kind,
                    detail: code,
                    state: state.into(),
                    ..base
                },
            )
            .map_err(native)?
            .check()
            .map_err(native)
    }
}
