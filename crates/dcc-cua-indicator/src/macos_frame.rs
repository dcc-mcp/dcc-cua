//! Exact WindowServer exposure and an operation-owned AppKit renderer.
use crate::{BannerActivity, TARGET_FRAME_THICKNESS_DIP};
use std::io;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Default, serde::Serialize)]
pub struct FrameBlocker {
    pid: i32,
    layer: i32,
    sharing: i32,
    system_dock: i32,
    window: u32,
    center_hit: i32,
    bounds: [f64; 4],
}

unsafe extern "C" {
    fn dcc_frame_point_hits(window: u32, x: f64, y: f64) -> i32;
    fn dcc_frame_exposed(
        owner: i32,
        window: u32,
        rect: *const i32,
        renderer: i32,
        width: i32,
        blocker: *mut FrameBlocker,
    ) -> i32;
    fn dcc_run_frame(rect: *const i32, width: i32, red: i32, green: i32, blue: i32) -> i32;
}

pub fn point_hits_window(window: u32, x: f64, y: f64) -> bool {
    unsafe { dcc_frame_point_hits(window, x, y) == 1 }
}

pub fn require_exposure(
    owner: i32,
    window: u32,
    rect: &[i32; 4],
    renderer: i32,
) -> Result<(), &'static str> {
    check_exposure(owner, window, rect, renderer, None)
}

pub fn exposure_diagnostic(
    owner: i32,
    window: u32,
    rect: &[i32; 4],
) -> (&'static str, Option<FrameBlocker>) {
    let mut blocker = FrameBlocker::default();
    let status = check_exposure(owner, window, rect, 0, Some(&mut blocker));
    (
        status.err().unwrap_or("exposed"),
        (blocker.window != 0).then_some(blocker),
    )
}

fn check_exposure(
    owner: i32,
    window: u32,
    rect: &[i32; 4],
    renderer: i32,
    blocker: Option<&mut FrameBlocker>,
) -> Result<(), &'static str> {
    // WindowServer only accesses the fixed input and optional output during this call.
    match unsafe {
        dcc_frame_exposed(
            owner,
            window,
            rect.as_ptr(),
            renderer,
            TARGET_FRAME_THICKNESS_DIP,
            blocker.map_or(std::ptr::null_mut(), std::ptr::from_mut),
        )
    } {
        1 => Ok(()),
        -1 => Err("macOS application is not frontmost"),
        -2 => Err("macOS window does not fit its display"),
        -3 => Err("macOS WindowServer bounds changed"),
        -4 => Err("macOS execution frame has no visible band"),
        -5 => Err("macOS window is covered by another on-screen surface"),
        _ => Err("macOS WindowServer exposure could not be verified"),
    }
}

pub fn run_renderer(args: &[String]) -> i32 {
    let Ok(rect) = args
        .iter()
        .map(|v| v.parse::<i32>())
        .collect::<Result<Vec<_>, _>>()
    else {
        return 2;
    };
    let Ok(rect): Result<[i32; 4], _> = rect.try_into() else {
        return 2;
    };
    if rect[2] <= 0 || rect[3] <= 0 {
        return 2;
    }
    let color = BannerActivity::Operating.color();
    unsafe {
        dcc_run_frame(
            rect.as_ptr(),
            TARGET_FRAME_THICKNESS_DIP,
            color.red.into(),
            color.green.into(),
            color.blue.into(),
        )
    }
}

pub struct ExecutionFrame {
    child: Child,
}

impl ExecutionFrame {
    pub fn start(owner: i32, window: u32, rect: &[i32; 4]) -> io::Result<Self> {
        let child = Command::new(std::env::current_exe()?)
            .arg("__macos-execution-frame")
            .args(rect.map(|v| v.to_string()))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut frame = Self { child };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            frame.check_interrupt()?;
            match require_exposure(owner, window, rect, frame.child.id() as i32) {
                Ok(()) => return Ok(frame),
                Err(reason) if Instant::now() >= deadline => return Err(io::Error::other(reason)),
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    pub fn check_interrupt(&mut self) -> io::Result<()> {
        match self.child.try_wait()? {
            None => Ok(()),
            Some(exit) if exit.code() == Some(3) => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Escape interrupted the macOS sequence",
            )),
            Some(_) => Err(io::Error::other(
                "macOS execution frame or Escape monitor unavailable",
            )),
        }
    }

    pub fn validate(&mut self, pid: i32, window: u32, rect: &[i32; 4]) -> io::Result<()> {
        self.check_interrupt()?;
        let mut blocker = FrameBlocker::default();
        check_exposure(
            pid,
            window,
            rect,
            self.child.id() as i32,
            Some(&mut blocker),
        )
        .map_err(|reason| {
            if blocker.window == 0 {
                io::Error::other(reason)
            } else {
                io::Error::other(format!(
                    "{reason} (window {}, layer {}, system_dock {}, center_hit {})",
                    blocker.window, blocker.layer, blocker.system_dock, blocker.center_hit
                ))
            }
        })
    }
}

impl Drop for ExecutionFrame {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
