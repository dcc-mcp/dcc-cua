//! Local authenticated X11 only. A transport deadline bounds real reads, not
//! merely the lifetime of the async caller waiting on a native worker.
use std::io::{self, IoSlice};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use socket2::{Domain, SockAddr, Socket, Type};
use x11rb::reexports::x11rb_protocol::xauth::get_auth;
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream};
use x11rb::utils::RawFdContainer;

pub(super) const OPERATION_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) type Connection = RustConnection<DeadlineStream>;

pub(super) struct DeadlineStream {
    inner: DefaultStream,
    deadline: Mutex<Instant>,
}

impl DeadlineStream {
    pub(super) fn renew(&self) {
        *self.deadline.lock().expect("X11 deadline mutex") = Instant::now() + OPERATION_TIMEOUT;
    }

    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .lock()
            .expect("X11 deadline mutex")
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "exact X11 capture deadline"))
    }
}

impl Stream for DeadlineStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        loop {
            let timeout = Timespec::try_from(self.remaining()?).expect("three second timeout");
            match poll(&mut [PollFd::new(&self.inner, flags)], Some(&timeout)) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::TimedOut, "X11 timeout")),
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn read(&self, buf: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.remaining()?;
        self.inner.read(buf, fds)
    }

    fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.remaining()?;
        self.inner.write(buf, fds)
    }

    fn write_vectored(
        &self,
        bufs: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.remaining()?;
        self.inner.write_vectored(bufs, fds)
    }
}

pub(super) fn connect() -> Result<(Connection, usize), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
    {
        return Err("exact pixels are unsupported on Wayland, including XWayland sessions".into());
    }
    let display = std::env::var("DISPLAY").map_err(|_| "local X11 DISPLAY is required")?;
    let display = display.strip_prefix("unix/").unwrap_or(&display);
    let local = display
        .strip_prefix(':')
        .ok_or("only local Unix X11 displays are supported")?;
    let (number, screen) = local.split_once('.').unwrap_or((local, "0"));
    let number: u16 = number.parse().map_err(|_| "invalid X11 display number")?;
    let screen: usize = screen.parse().map_err(|_| "invalid X11 screen number")?;
    // Nonblocking connect refuses a saturated listener instead of leaving an
    // unowned blocking task. No TCP, auth override, access-control changes or
    // desktop-discovery fallback is allowed on this route.
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None).map_err(|e| e.to_string())?;
    socket.set_nonblocking(true).map_err(|e| e.to_string())?;
    let address = SockAddr::unix(format!("/tmp/.X11-unix/X{number}")).map_err(|e| e.to_string())?;
    socket.connect(&address).map_err(|e| e.to_string())?;
    let (inner, (family, address)) =
        DefaultStream::from_unix_stream(UnixStream::from(socket)).map_err(|e| e.to_string())?;
    let (auth_name, auth_data) = get_auth(family, &address, number)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let stream = DeadlineStream {
        inner,
        deadline: Mutex::new(Instant::now() + OPERATION_TIMEOUT),
    };
    let connection =
        RustConnection::connect_to_stream_with_auth_info(stream, screen, auth_name, auth_data)
            .map_err(|e| e.to_string())?;
    Ok((connection, screen))
}
