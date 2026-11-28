use super::*;
use x11rb::protocol::shape::{SK, SO};
use x11rb::protocol::xproto::{ClipOrdering, CreateWindowAux};
use x11rb::wrapper::ConnectionExt as _;

/// A separate connection owns the exterior bands. Closing it also releases
/// the target's Escape grab, including after partial startup or cancellation.
pub(super) struct ExecutionFrame {
    connection: Connection,
    bands: Vec<(u32, [i32; 4])>,
    escape: u8,
    server_time: u32,
    sampled_at: Instant,
}

impl ExecutionFrame {
    pub(super) fn show(
        bound: &LinuxPixels,
        bounds: [i32; 4],
        escape: u8,
    ) -> ComputerUseResult<Self> {
        let (connection, screen) = transport::connect().map_err(native)?;
        let root = &connection.setup().roots[screen];
        if root.root != bound.root || root.root_depth != 24 {
            return Err(native("execution frame requires the bound 24-bit display"));
        }
        let [x, y, w, h] = bounds;
        let color = BannerActivity::Operating.color();
        let pixel = u32::from_be_bytes([0, color.red, color.green, color.blue]);
        let attrs = CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(pixel);
        let mut bands = Vec::with_capacity(4);
        for rect in [
            [x - 3, y, 3, h],
            [x + w, y, 3, h],
            [x, y - 3, w, 3],
            [x, y + h, w, 3],
        ] {
            let [x, y, w, h] = rect;
            if x < 0
                || y < 0
                || x + w > i32::from(root.width_in_pixels)
                || y + h > i32::from(root.height_in_pixels)
            {
                continue;
            }
            let id = connection.generate_id().map_err(native)?;
            connection
                .create_window(
                    root.root_depth,
                    id,
                    root.root,
                    i16::try_from(x).map_err(native)?,
                    i16::try_from(y).map_err(native)?,
                    w as u16,
                    h as u16,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    root.root_visual,
                    &attrs,
                )
                .map_err(native)?
                .check()
                .map_err(native)?;
            connection
                .shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, id, 0, 0, &[])
                .map_err(native)?
                .check()
                .map_err(native)?;
            connection
                .map_window(id)
                .map_err(native)?
                .check()
                .map_err(native)?;
            bands.push((id, rect));
        }
        if bands.is_empty() {
            return Err(native(
                "no exterior edge is available for the execution frame",
            ));
        }
        // Native clients compare input timestamps to their X server clock.
        // CURRENT_TIME in an event payload is zero, which can turn distinct
        // taps into autorepeats and lose characters in Blender search fields.
        use x11rb::protocol::xproto::{GrabMode, ModMask, PropMode};
        let band = bands[0].0;
        connection
            .change_window_attributes(
                band,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        connection
            .change_property8(
                PropMode::REPLACE,
                band,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"DCC-CUA",
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        let Event::PropertyNotify(event) = connection.wait_for_event().map_err(native)? else {
            return Err(native(
                "execution frame did not receive an X server timestamp",
            ));
        };
        let sampled_at = Instant::now();
        connection
            .grab_key(
                false,
                bound.window,
                ModMask::ANY,
                escape,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        let frame = Self {
            connection,
            bands,
            escape,
            server_time: event.time,
            sampled_at,
        };
        frame.validate(bound)?;
        Ok(frame)
    }

    pub(super) fn timestamp(&self) -> u32 {
        self.server_time
            .wrapping_add(self.sampled_at.elapsed().as_millis() as u32)
    }

    pub(super) fn check_escape(&self) -> ComputerUseResult<()> {
        self.connection.stream().renew();
        // A tap between two chords is retained in the grab connection's
        // event queue. QueryKeymap also catches Escape held before the grab.
        let bits = self
            .connection
            .query_keymap()
            .map_err(native)?
            .reply()
            .map_err(native)?
            .keys;
        let mut interrupted = bits[usize::from(self.escape / 8)] & (1 << (self.escape % 8)) != 0;
        for _ in 0..MAX_WINDOWS {
            match self.connection.poll_for_event().map_err(native)? {
                None => break,
                Some(Event::KeyPress(key)) if key.detail == self.escape => interrupted = true,
                Some(_) => {}
            }
        }
        if interrupted {
            dcc_cua_interrupt::broadcast_interrupt();
            return Err(refused(
                ComputerUseErrorCode::UserInterrupted,
                "Escape interrupted the X11 sequence",
            ));
        }
        Ok(())
    }

    pub(super) fn validate(&self, bound: &LinuxPixels) -> ComputerUseResult<()> {
        self.check_escape()?;
        for &(id, rect) in &self.bands {
            let attrs = self
                .connection
                .get_window_attributes(id)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let geometry = self
                .connection
                .get_geometry(id)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let actual = [
                i32::from(geometry.x),
                i32::from(geometry.y),
                i32::from(geometry.width),
                i32::from(geometry.height),
            ];
            if attrs.map_state == MapState::VIEWABLE
                && actual == rect
                && bound.require_unobscured(id, rect).is_ok()
            {
                return Ok(());
            }
        }
        Err(refused(
            ComputerUseErrorCode::BackendUnavailable,
            "no exposed X11 execution frame band",
        ))
    }
}
