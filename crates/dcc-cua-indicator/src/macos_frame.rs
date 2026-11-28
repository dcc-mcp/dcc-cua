//! Exact WindowServer exposure and an operation-owned AppKit renderer.
use crate::{BannerActivity, TARGET_FRAME_THICKNESS_DIP};
use std::io;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn dcc_frame_exposed(
        owner: i32,
        window: u32,
        rect: *const i32,
        renderer: i32,
        width: i32,
    ) -> i32;
    fn dcc_run_frame(rect: *const i32, width: i32, red: i32, green: i32, blue: i32) -> i32;
}

pub fn exposed(owner: i32, window: u32, rect: &[i32; 4], renderer: i32) -> bool {
    // WindowServer only reads the fixed array during this call.
    unsafe {
        dcc_frame_exposed(
            owner,
            window,
            rect.as_ptr(),
            renderer,
            TARGET_FRAME_THICKNESS_DIP,
        ) != 0
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
            if exposed(owner, window, rect, frame.child.id() as i32) {
                return Ok(frame);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("no exposed macOS execution frame"));
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
        if exposed(pid, window, rect, self.child.id() as i32) {
            Ok(())
        } else {
            Err(io::Error::other(
                "macOS foreground window or execution frame lost exposure",
            ))
        }
    }
}

impl Drop for ExecutionFrame {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
