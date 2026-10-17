//! Test-only renderer identity discovery. No synchronous window-title RPCs.
//! This locator grants no input authority; the real session owns its capability.
#[allow(unused_imports)]
use rstest::rstest;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const BANNER: &str = "DccCuaControlBanner";
const FRAME: &str = "DccCuaControlFrame";
const MAX_ROOTS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    hwnd: u64,
    pid: u32,
    thread: u32,
    class: String,
    cookie: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Root {
    identity: Identity,
    visible: bool,
    size: [i32; 2],
}

impl Identity {
    fn presenter(&self) -> bool {
        matches!(self.class.as_str(), BANNER | FRAME)
    }
}

pub(super) fn snapshot(pid: u32) -> Vec<Root> {
    struct Read {
        pid: u32,
        deadline: Instant,
        roots: Vec<Root>,
        error: Option<&'static str>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, opaque: LPARAM) -> i32 {
        let read = unsafe { &mut *(opaque as *mut Read) };
        if Instant::now() >= read.deadline {
            read.error = Some("native metadata deadline exceeded");
            return 0;
        }
        match inspect(read.pid, hwnd) {
            Ok(Some(root)) if read.roots.len() < MAX_ROOTS => read.roots.push(root),
            Ok(None) => {}
            Ok(Some(_)) => read.error = Some("native metadata root limit exceeded"),
            Err(error) => read.error = Some(error),
        }
        i32::from(read.error.is_none())
    }
    let mut read = Read {
        pid,
        deadline: Instant::now() + Duration::from_secs(2),
        roots: Vec::new(),
        error: None,
    };
    let complete = unsafe { EnumWindows(Some(visit), &mut read as *mut Read as isize) };
    eprintln!(
        "owned-presenter metadata pid={pid} complete={complete} error={:?} roots={:?}",
        read.error, read.roots
    );
    assert!(
        complete != 0 && read.error.is_none(),
        "owned presenter metadata failed: {:?}",
        read.error
    );
    // Recheck all identities after enumeration as well as around each sample.
    // No failed read, disappeared root or cookie/handle reuse becomes an empty
    // optional family, a retry, or permission to ignore a physical blocker.
    for root in &read.roots {
        assert!(
            Instant::now() < read.deadline,
            "native metadata deadline exceeded"
        );
        assert_eq!(
            identity(root.identity.hwnd as usize as HWND).unwrap(),
            root.identity,
            "native root identity changed after enumeration"
        );
    }
    read.roots
}

fn identity(hwnd: HWND) -> Result<Identity, &'static str> {
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || thread == 0 {
        return Err("native root ownership unavailable");
    }
    let mut class = [0_u16; 128];
    let length = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if length <= 0 || length as usize >= class.len() - 1 {
        return Err("native root class unavailable or truncated");
    }
    let property: Vec<_> = "DccCuaOwnedPresenterInstance"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let cookie = unsafe { GetPropW(hwnd, property.as_ptr()) } as usize;
    Ok(Identity {
        hwnd: hwnd as usize as u64,
        pid,
        thread,
        class: String::from_utf16_lossy(&class[..length as usize]),
        cookie,
    })
}

fn inspect(pid: u32, hwnd: HWND) -> Result<Option<Root>, &'static str> {
    let mut owner = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
    if owner != pid {
        return Ok(None);
    }
    let initial = identity(hwnd)?;
    if initial.pid != pid {
        return Err("native root process changed");
    }
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if initial.presenter() && unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err("presenter rectangle unavailable");
    }
    if initial != identity(hwnd)? {
        return Err("native root identity changed during sample");
    }
    Ok(Some(Root {
        identity: initial,
        visible,
        size: [rect.right - rect.left, rect.bottom - rect.top],
    }))
}

pub(super) fn new_family(
    pid: u32,
    before: &[Root],
    after: &[Root],
    bands: usize,
) -> Result<(Vec<u64>, Vec<u64>), &'static str> {
    let presenters: Vec<_> = after
        .iter()
        .filter(|root| root.identity.presenter())
        .collect();
    if presenters
        .iter()
        .any(|root| root.identity.pid != pid || root.identity.thread == 0)
    {
        return Err("presenter owner mismatch");
    }
    for previous in before.iter().filter(|root| root.identity.presenter()) {
        if !after.iter().any(|root| root.identity == previous.identity) {
            return Err("previous presenter disappeared or changed instance");
        }
    }
    let new: Vec<_> = presenters
        .iter()
        .copied()
        .filter(|root| {
            !before
                .iter()
                .any(|previous| previous.identity == root.identity)
        })
        .collect();
    let banners: Vec<_> = new
        .iter()
        .copied()
        .filter(|root| root.identity.class == BANNER)
        .collect();
    if banners.len() != 1 || banners[0].identity.cookie == 0 {
        return Err("exactly one new owned banner required");
    }
    let thread = banners[0].identity.thread;
    if before.iter().any(|root| root.identity.thread == thread)
        || new.iter().any(|root| root.identity.thread != thread)
    {
        return Err("new renderer must have its own exact thread");
    }
    let mut cookies = std::collections::HashSet::new();
    if presenters
        .iter()
        .filter(|root| root.identity.cookie != 0)
        .any(|root| !cookies.insert(root.identity.cookie))
    {
        return Err("duplicate presenter instance cookie");
    }
    let frames: Vec<_> = new
        .iter()
        .copied()
        .filter(|root| root.identity.class == FRAME && root.identity.cookie != 0)
        .collect();
    if frames.len() != bands {
        return Err("complete owned frame bands required");
    }
    let auxiliary: Vec<_> = new
        .iter()
        .copied()
        .filter(|root| root.identity.cookie == 0)
        .collect();
    // The optional DpiProbeWindow is a hidden, unbound 1x1 frame-class window.
    // It is never counted as a presentation band or accepted as input authority.
    if auxiliary.len() > 1
        || auxiliary
            .iter()
            .any(|root| root.identity.class != FRAME || root.visible || root.size != [1, 1])
    {
        return Err("unexpected unbound renderer surface");
    }
    eprintln!(
        "owned-presenter bound banner={} thread={thread} bands={} auxiliary={:?}",
        banners[0].identity.hwnd,
        frames.len(),
        auxiliary
    );
    Ok((
        new.iter()
            .filter(|root| root.identity.cookie != 0)
            .map(|root| root.identity.hwnd)
            .collect(),
        frames.iter().map(|root| root.identity.hwnd).collect(),
    ))
}

#[path = "owned_presenter/tests.rs"]
mod tests;
