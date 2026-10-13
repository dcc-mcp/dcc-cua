//! Conservative read-only X11 pixels. The X server and its same-user clients
//! are trusted, as in X11 itself: _NET_WM_PID is metadata, not an OS authority.
//! A retained connection observes destruction/owner changes and never rebinds.
//! Only unobscured, rectangular, non-composited 24-bit TrueColor windows are
//! supported. No desktop crop, accessibility, helper process or input fallback.
use super::*;
use std::sync::Mutex;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, ImageFormat, ImageOrder,
    MapState, VisualClass, WindowClass,
};
mod transport;
use transport::Connection;

const MAX_WINDOWS: usize = 4096;
const MAX_PIXELS: usize = 16_777_216;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessIdentity {
    start_ticks: u64,
    executable: std::path::PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
struct Evidence {
    target: WindowTarget,
    process: ProcessIdentity,
    visual: u32,
}

pub(super) struct LinuxPixels {
    connection: Connection,
    root: u32,
    window: u32,
    pid: u32,
    pid_atom: u32,
    title_atom: u32,
    utf8_atom: u32,
    compositor_atom: u32,
    expected_title: Option<String>,
    process: ProcessIdentity,
    generation: u64,
    usable: bool,
}

pub(super) type Handle = Arc<Mutex<LinuxPixels>>;

fn refused(code: ComputerUseErrorCode, message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(code, message)
}
fn native(error: impl std::fmt::Display) -> ComputerUseError {
    refused(
        ComputerUseErrorCode::CaptureFailed,
        format!("exact X11 capture failed: {error}"),
    )
}
fn process_identity(pid: u32) -> ComputerUseResult<ProcessIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(native)?;
    let fields = stat
        .rsplit_once(')')
        .ok_or_else(|| native("invalid /proc process identity"))?
        .1;
    let start_ticks = fields
        .split_whitespace()
        .nth(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| native("missing /proc process start time"))?;
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(native)?;
    Ok(ProcessIdentity {
        start_ticks,
        executable,
    })
}

impl LinuxPixels {
    pub(super) fn bind(scope: ComputerUseTargetScope) -> ComputerUseResult<(Handle, WindowTarget)> {
        let pid = scope.process_id.filter(|pid| *pid != 0).ok_or_else(|| {
            refused(
                ComputerUseErrorCode::InvalidTarget,
                "X11 pixels require an explicit PID",
            )
        })?;
        let window = scope
            .window_handle
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id != 0)
            .ok_or_else(|| {
                refused(
                    ComputerUseErrorCode::InvalidTarget,
                    "X11 pixels require an explicit nonzero XID",
                )
            })?;
        let (connection, screen) = transport::connect()
            .map_err(|e| refused(ComputerUseErrorCode::BackendUnavailable, e))?;
        let root = connection.setup().roots[screen].root;
        let atom = |name: &[u8]| -> ComputerUseResult<u32> {
            Ok(connection
                .intern_atom(false, name)
                .map_err(native)?
                .reply()
                .map_err(native)?
                .atom)
        };
        let pid_atom = atom(b"_NET_WM_PID")?;
        let title_atom = atom(b"_NET_WM_NAME")?;
        let utf8_atom = atom(b"UTF8_STRING")?;
        let compositor = atom(format!("_NET_WM_CM_S{screen}").as_bytes())?;
        if connection
            .get_selection_owner(compositor)
            .map_err(native)?
            .reply()
            .map_err(native)?
            .owner
            != 0
        {
            return Err(refused(
                ComputerUseErrorCode::BackendUnavailable,
                "this first X11 pixels route requires a non-composited display",
            ));
        }
        // Subscribe before the first ownership read. Closing/reusing this XID
        // or changing its advertised owner cannot silently become a new bind.
        connection
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
            )
            .map_err(native)?
            .check()
            .map_err(native)?;
        connection
            .shape_select_input(window, true)
            .map_err(native)?
            .check()
            .map_err(native)?;
        let process = process_identity(pid)?;
        let mut bound = Self {
            connection,
            root,
            window,
            pid,
            pid_atom,
            title_atom,
            utf8_atom,
            compositor_atom: compositor,
            expected_title: scope.window_title,
            process,
            generation: 0,
            usable: true,
        };
        let evidence = bound.evidence()?;
        bound.drain_events(false)?;
        validate_target_policy(&evidence.target)?;
        Ok((Arc::new(Mutex::new(bound)), evidence.target))
    }

    fn drain_events(&mut self, capturing: bool) -> ComputerUseResult<()> {
        for _ in 0..MAX_WINDOWS {
            let Some(event) = self.connection.poll_for_event().map_err(native)? else {
                return Ok(());
            };
            let identity_lost = matches!(&event, Event::DestroyNotify(event) if event.window == self.window)
                || matches!(&event, Event::PropertyNotify(event) if event.window == self.window && event.atom == self.pid_atom);
            if identity_lost {
                self.usable = false;
                return Err(refused(
                    ComputerUseErrorCode::InvalidTarget,
                    "the bound X11 window was destroyed or its owner changed; explicitly bind a new session",
                ));
            }
            if capturing {
                // Conservative: even an unrelated root-stack change discards
                // this attempt. Never admit a cached frame after a layout race.
                return Err(refused(
                    ComputerUseErrorCode::StaleObservation,
                    "X11 window properties or stacking changed during capture",
                ));
            }
        }
        Err(native("X11 event inventory exceeded its bound"))
    }

    fn evidence(&self) -> ComputerUseResult<Evidence> {
        if self
            .connection
            .get_selection_owner(self.compositor_atom)
            .map_err(native)?
            .reply()
            .map_err(native)?
            .owner
            != 0
        {
            return Err(refused(
                ComputerUseErrorCode::BackendUnavailable,
                "composited X11 capture is not supported",
            ));
        }
        let owner = self
            .connection
            .get_property(false, self.window, self.pid_atom, AtomEnum::CARDINAL, 0, 2)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        let owners: Vec<_> = owner.value32().into_iter().flatten().collect();
        if owners != [self.pid] || owner.bytes_after != 0 {
            return Err(refused(
                ComputerUseErrorCode::InvalidTarget,
                "XID does not advertise the explicitly selected PID",
            ));
        }
        let process = process_identity(self.pid)?;
        if process != self.process {
            return Err(refused(
                ComputerUseErrorCode::InvalidTarget,
                "the bound Linux process instance changed",
            ));
        }
        let attributes = self
            .connection
            .get_window_attributes(self.window)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if attributes.map_state != MapState::VIEWABLE
            || attributes.class != WindowClass::INPUT_OUTPUT
        {
            return Err(refused(
                ComputerUseErrorCode::TargetUnavailable,
                "the X11 target is not a viewable pixel window",
            ));
        }
        let shape = self
            .connection
            .shape_query_extents(self.window)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if shape.bounding_shaped || shape.clip_shaped {
            return Err(refused(
                ComputerUseErrorCode::BackendUnavailable,
                "shaped X11 windows are not supported by this exact pixels route",
            ));
        }
        let geometry = self
            .connection
            .get_geometry(self.window)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if geometry.depth != 24
            || usize::from(geometry.width) * usize::from(geometry.height) > MAX_PIXELS
            || geometry.width == 0
            || geometry.height == 0
        {
            return Err(refused(
                ComputerUseErrorCode::BackendUnavailable,
                "exact X11 pixels require bounded 24-bit TrueColor dimensions",
            ));
        }
        let translated = self
            .connection
            .translate_coordinates(self.window, self.root, 0, 0)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        let bounds = [
            i32::from(translated.dst_x),
            i32::from(translated.dst_y),
            i32::from(geometry.width),
            i32::from(geometry.height),
        ];
        let root_geometry = self
            .connection
            .get_geometry(self.root)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        if !translated.same_screen
            || bounds[0] < 0
            || bounds[1] < 0
            || bounds[0] + bounds[2] > i32::from(root_geometry.width)
            || bounds[1] + bounds[3] > i32::from(root_geometry.height)
        {
            return Err(refused(
                ComputerUseErrorCode::TargetUnavailable,
                "the exact X11 target is outside its screen",
            ));
        }
        self.require_unobscured(bounds)?;
        let title = self
            .connection
            .get_property(false, self.window, self.title_atom, self.utf8_atom, 0, 4096)
            .map_err(native)?
            .reply()
            .map_err(native)?;
        let title = String::from_utf8_lossy(&title.value).into_owned();
        if self
            .expected_title
            .as_ref()
            .is_some_and(|expected| expected != &title)
        {
            return Err(refused(
                ComputerUseErrorCode::InvalidTarget,
                "the exact X11 title selector changed",
            ));
        }
        let app_name = process
            .executable
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Ok(Evidence {
            target: WindowTarget {
                pid: self.pid,
                window_id: u64::from(self.window),
                title,
                app_name,
                bounds,
                is_on_screen: true,
                is_minimized: false,
                z_index: None,
                is_foreground: false,
            },
            process,
            visual: attributes.visual,
        })
    }

    fn require_unobscured(&self, bounds: [i32; 4]) -> ComputerUseResult<()> {
        let mut child = self.window;
        let mut remaining = MAX_WINDOWS;
        for _ in 0..32 {
            let tree = self
                .connection
                .query_tree(child)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            if tree.parent == 0 {
                return Err(native("X11 target ancestry ended before its root"));
            }
            let siblings = self
                .connection
                .query_tree(tree.parent)
                .map_err(native)?
                .reply()
                .map_err(native)?
                .children;
            if siblings.len() > remaining {
                return Err(native("X11 stacking inventory exceeded its bound"));
            }
            remaining -= siblings.len();
            let index = siblings
                .iter()
                .position(|id| *id == child)
                .ok_or_else(|| native("target missing from X11 stack"))?;
            for &window in &siblings[index + 1..] {
                let attrs = self
                    .connection
                    .get_window_attributes(window)
                    .map_err(native)?
                    .reply()
                    .map_err(native)?;
                if attrs.map_state != MapState::VIEWABLE || attrs.class != WindowClass::INPUT_OUTPUT
                {
                    continue;
                }
                let geometry = self
                    .connection
                    .get_geometry(window)
                    .map_err(native)?
                    .reply()
                    .map_err(native)?;
                let position = self
                    .connection
                    .translate_coordinates(window, self.root, 0, 0)
                    .map_err(native)?
                    .reply()
                    .map_err(native)?;
                let border = i32::from(geometry.border_width);
                let other = [
                    i32::from(position.dst_x) - border,
                    i32::from(position.dst_y) - border,
                    i32::from(geometry.width) + 2 * border,
                    i32::from(geometry.height) + 2 * border,
                ];
                if bounds[0] < other[0] + other[2]
                    && other[0] < bounds[0] + bounds[2]
                    && bounds[1] < other[1] + other[3]
                    && other[1] < bounds[1] + bounds[3]
                {
                    return Err(refused(
                        ComputerUseErrorCode::CaptureFailed,
                        "the exact X11 target is occluded; pixels were discarded",
                    ));
                }
            }
            if tree.parent == self.root {
                return Ok(());
            }
            // Reparenting window managers may clip/shape a client. Check the
            // entire ancestry and observe changes before admitting GetImage.
            let parent = tree.parent;
            self.connection
                .change_window_attributes(
                    parent,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_NOTIFY),
                )
                .map_err(native)?
                .check()
                .map_err(native)?;
            self.connection
                .shape_select_input(parent, true)
                .map_err(native)?
                .check()
                .map_err(native)?;
            let attributes = self
                .connection
                .get_window_attributes(parent)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let geometry = self
                .connection
                .get_geometry(parent)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let position = self
                .connection
                .translate_coordinates(parent, self.root, 0, 0)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            let shape = self
                .connection
                .shape_query_extents(parent)
                .map_err(native)?
                .reply()
                .map_err(native)?;
            if attributes.map_state != MapState::VIEWABLE
                || shape.bounding_shaped
                || shape.clip_shaped
                || bounds[0] < i32::from(position.dst_x)
                || bounds[1] < i32::from(position.dst_y)
                || bounds[0] + bounds[2] > i32::from(position.dst_x) + i32::from(geometry.width)
                || bounds[1] + bounds[3] > i32::from(position.dst_y) + i32::from(geometry.height)
            {
                return Err(refused(
                    ComputerUseErrorCode::TargetUnavailable,
                    "X11 target is clipped or shaped by an ancestor",
                ));
            }
            child = parent;
        }
        Err(native("X11 ancestry exceeded its bound"))
    }

    pub(super) fn target(&mut self) -> ComputerUseResult<WindowTarget> {
        self.begin()?;
        self.drain_events(false)?;
        let target = self.evidence()?.target;
        self.drain_events(false)?;
        Ok(target)
    }

    fn begin(&self) -> ComputerUseResult<()> {
        if !self.usable {
            return Err(refused(
                ComputerUseErrorCode::InvalidTarget,
                "X11 pixel binding is no longer usable",
            ));
        }
        self.connection.stream().renew();
        Ok(())
    }

    pub(super) fn capture(&mut self) -> ComputerUseResult<(Vec<u8>, WindowTarget, Value)> {
        let result = self.capture_inner();
        if result.is_err() {
            self.usable = false;
        }
        result
    }

    fn capture_inner(&mut self) -> ComputerUseResult<(Vec<u8>, WindowTarget, Value)> {
        self.begin()?;
        self.drain_events(false)?;
        let before = self.evidence()?;
        let [_, _, width, height] = before.target.bounds;
        let image = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                0,
                0,
                width as u16,
                height as u16,
                u32::MAX,
            )
            .map_err(native)?
            .reply()
            .map_err(native)?;
        let setup = self.connection.setup();
        let format = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == 24)
            .ok_or_else(|| native("X11 24-bit pixmap format missing"))?;
        let visual = setup
            .roots
            .iter()
            .flat_map(|root| &root.allowed_depths)
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == before.visual)
            .ok_or_else(|| native("X11 visual missing"))?;
        if image.depth != 24
            || image.visual != before.visual
            || format.bits_per_pixel != 32
            || format.scanline_pad != 32
            || visual.class != VisualClass::TRUE_COLOR
            || (visual.red_mask, visual.green_mask, visual.blue_mask) != (0xff0000, 0xff00, 0xff)
            || image.data.len() != width as usize * height as usize * 4
        {
            return Err(refused(
                ComputerUseErrorCode::BackendUnavailable,
                "unsupported exact X11 pixel layout",
            ));
        }
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for pixel in image.data.chunks_exact(4) {
            if setup.image_byte_order == ImageOrder::LSB_FIRST {
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            } else {
                rgb.extend_from_slice(&pixel[1..4]);
            }
        }
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, width as u32, height as u32);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .map_err(native)?
                .write_image_data(&rgb)
                .map_err(native)?;
        }
        // Encoding and all native reads precede the publication fence. This is
        // a fresh GetImage request, never an upstream cached screenshot.
        let after = self.evidence()?;
        self.drain_events(true)?;
        if before != after {
            return Err(refused(
                ComputerUseErrorCode::StaleObservation,
                "the exact X11 target changed during capture",
            ));
        }
        self.generation += 1;
        let provenance = json!({"backend":"x11-get-image-exact", "scope":"window", "observation_mode":"pixels_only",
            "pixels_captured":true, "read_only":true, "input_available":false, "activation_available":false,
            "accessibility_available":false, "degraded":false, "whole_desktop_capture":false,
            "fallback":null, "process_id":self.pid, "window_handle":self.window,
            "native_window_bounds":after.target.bounds, "capture_generation":self.generation,
            "process_start_ticks":self.process.start_ticks, "identity_source":"local_proc_and_x11_owner_metadata",
            "window_lifecycle_subscription":true, "occlusion_checked":true, "coordinate_units":"x11_pixels"});
        Ok((png, after.target, provenance))
    }
}

pub(super) async fn operate<T: Send + 'static>(
    handle: Handle,
    operation: impl FnOnce(&mut LinuxPixels) -> ComputerUseResult<T> + Send + 'static,
) -> ComputerUseResult<T> {
    tokio::task::spawn_blocking(move || {
        let mut bound = handle
            .lock()
            .map_err(|_| native("X11 capture worker panicked"))?;
        operation(&mut bound)
    })
    .await
    .map_err(native)?
}
