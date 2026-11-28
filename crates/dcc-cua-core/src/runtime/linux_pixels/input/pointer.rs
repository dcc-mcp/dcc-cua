use super::*;
use x11rb::protocol::xproto::MOTION_NOTIFY_EVENT;

impl LinuxPixels {
    pub(super) fn pointer_action(
        &mut self,
        action: &ComputerUseAction,
        base: KeyPressEvent,
        guard: &InputGuard<'_>,
        frame: &ExecutionFrame,
        attempted: &mut bool,
    ) -> ComputerUseResult<()> {
        if !action.modifiers.is_empty() || action.button.as_deref().is_some_and(|b| b != "left") {
            return Err(invalid(
                "X11 pointer actions require an unmodified primary button",
            ));
        }
        let points = crate::runtime::pixel_input_policy::pointer_samples(action, |x, y| {
            let coord = |v: f64, n: u32| {
                (0.0..f64::from(n))
                    .contains(&v)
                    .then(|| i16::try_from(v.floor() as i32).ok())
                    .flatten()
                    .ok_or_else(|| invalid("pointer outside exact X11 window"))
            };
            Ok((
                coord(x, guard.observation.width)?,
                coord(y, guard.observation.height)?,
            ))
        })?;
        let position = |(x, y): (i16, i16)| -> ComputerUseResult<KeyPressEvent> {
            let rect = guard.observation.source_rect;
            Ok(KeyPressEvent {
                event_x: x,
                event_y: y,
                root_x: i16::try_from(i64::from(rect[0]) + i64::from(x)).map_err(native)?,
                root_y: i16::try_from(i64::from(rect[1]) + i64::from(y)).map_err(native)?,
                ..base
            })
        };
        let dragging = action.action == "drag";
        let mut events = Vec::with_capacity(points.len());
        {
            let _fence = self.dispatch_fence(frame, guard)?;
            for point in points {
                let event = position(point)?;
                let destination = self.pointer_destination(event.root_x, event.root_y)?;
                // The retained identity and destruction subscription cover this
                // top-level client. Never release a held gesture to a new child.
                if dragging && destination.0 != self.window {
                    return Err(invalid(
                        "X11 drag requires a path on the bound client window",
                    ));
                }
                events.push((event, destination));
            }
        }
        let started = Instant::now();
        let mut held = None;
        let result = (|| {
            for (index, &(event, destination)) in events.iter().enumerate() {
                if dragging && index != 0 {
                    let until = started
                        + Duration::from_millis(
                            action.duration_ms.unwrap_or(500) * index as u64
                                / (events.len() - 1) as u64,
                        );
                    while let Some(left) = until.checked_duration_since(Instant::now()) {
                        std::thread::sleep(left.min(Duration::from_millis(8)));
                        drop(self.admit_input(guard)?);
                        frame.check_escape()?;
                    }
                }
                let _fence = self.dispatch_fence(frame, guard)?;
                if self.pointer_destination(event.root_x, event.root_y)? != destination {
                    return Err(invalid("X11 pointer destination changed"));
                }
                let admission = self.admit_input(guard)?;
                *attempted = true;
                self.connection
                    .warp_pointer(
                        x11rb::NONE,
                        self.window,
                        0,
                        0,
                        0,
                        0,
                        event.event_x,
                        event.event_y,
                    )
                    .map_err(native)?
                    .check()
                    .map_err(native)?;
                drop(admission);
                let landed = self
                    .connection
                    .query_pointer(self.window)
                    .map_err(native)?
                    .reply()
                    .map_err(native)?;
                if !landed.same_screen
                    || (landed.win_x, landed.win_y) != (event.event_x, event.event_y)
                    || u16::from(landed.mask) & !2 != 0
                    || self.pointer_destination(event.root_x, event.root_y)? != destination
                {
                    return Err(invalid("X11 pointer landing or button state changed"));
                }
                let addressed = KeyPressEvent {
                    time: frame.timestamp(),
                    event: destination.0,
                    event_x: destination.1,
                    event_y: destination.2,
                    ..event
                };
                let _admission = self.admit_input(guard)?;
                if action.action == "click" || dragging && index == 0 {
                    self.send_event(addressed, BUTTON_PRESS_EVENT, 1, 0)?;
                    held = Some(addressed);
                } else if dragging {
                    self.send_event(addressed, MOTION_NOTIFY_EVENT, 0, 1 << 8)?;
                    held = Some(addressed);
                }
                if held.is_some() && index + 1 == events.len() {
                    self.send_event(addressed, BUTTON_RELEASE_EVENT, 1, 1 << 8)?;
                    held = None;
                }
            }
            Ok(())
        })();
        if let Some(release) = held {
            // Only release the synthetic press on the original, surviving
            // client. There is no physical button held on the X server.
            self.connection.stream().renew();
            if let Ok(_fence) = DispatchFence::acquire(&self.connection)
                && release.event == self.window
                && self.usable
                && self.drain_events(false).is_ok()
                && process_identity(self.pid).is_ok_and(|process| process == self.process)
            {
                let _ = self.send_event(release, BUTTON_RELEASE_EVENT, 1, 1 << 8);
            }
        }
        result
    }
}
