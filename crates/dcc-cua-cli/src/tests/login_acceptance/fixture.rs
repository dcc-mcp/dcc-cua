//! Loopback-only login oracle. It never retains submitted password bytes.
#[allow(unused_imports)]
use rstest::rstest;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) const SECRET: &str = "synthetic-login-password-v1";
#[cfg(windows)]
pub(super) const HANDLE: &str = "fixture.login-password";
const COOKIE: &str = "dcc_fixture_session=authenticated-v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub submissions: usize,
    pub accepted_logins: usize,
    pub authenticated_requests: usize,
    pub boundary_submissions: usize,
}

#[derive(Default)]
struct State {
    counts: Counts,
    boundary: Option<&'static str>,
}

pub(super) struct LoginFixture {
    address: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LoginFixture {
    pub fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind controlled login fixture");
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut connections: Vec<std::thread::JoinHandle<()>> = Vec::new();
            while !worker_stop.load(Ordering::Relaxed) {
                let mut index = 0;
                while index < connections.len() {
                    if connections[index].is_finished() {
                        connections.swap_remove(index).join().unwrap();
                    } else {
                        index += 1;
                    }
                }
                match listener.accept() {
                    Ok((stream, peer)) if peer.ip().is_loopback() && connections.len() < 16 => {
                        let state = Arc::clone(&worker_state);
                        connections.push(std::thread::spawn(move || {
                            let _ = serve(stream, &state);
                        }));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            address,
            state,
            stop,
            worker: Some(worker),
        }
    }

    #[cfg(windows)]
    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }
    #[cfg(windows)]
    pub fn page_url(&self) -> String {
        format!("{}/fixture", self.origin())
    }
    pub fn idle_connection(&self) -> TcpStream {
        TcpStream::connect(self.address).unwrap()
    }

    pub fn counts(&self) -> Counts {
        self.state.lock().unwrap().counts
    }
    pub fn boundary(&self, reason: &'static str) {
        assert!(["two_factor", "captcha", "new_legal_terms"].contains(&reason));
        self.state.lock().unwrap().boundary = Some(reason);
    }

    pub fn request(&self, method: &str, path: &str, body: &str, cookie: Option<&str>) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let cookie = cookie
            .map(|value| format!("Cookie: {value}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n{cookie}Connection: close\r\n\r\n{body}",
            self.address,
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
}

impl Drop for LoginFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.address);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    // Winsock accepted sockets inherit the listener's nonblocking mode.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 2048];
    let header_end;
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > 16 * 1024 {
            return Ok(());
        }
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            header_end = end + 4;
            break;
        }
    }
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = headers.lines();
    let first = lines.next().unwrap_or_default();
    let mut words = first.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let path = words.next().unwrap_or_default().to_owned();
    let mut content_length = 0;
    let mut authenticated = false;
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().unwrap_or(usize::MAX);
            }
            if key.eq_ignore_ascii_case("cookie") {
                authenticated = value.split(';').any(|part| part.trim() == COOKIE);
            }
        }
    }
    if content_length > 4096 {
        return Ok(());
    }
    while bytes.len() - header_end < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > 16 * 1024 {
            return Ok(());
        }
    }
    let (status, extra, page) = {
        let mut state = state.lock().unwrap();
        match (method.as_str(), path.as_str()) {
            ("GET", "/fixture") => {
                let content = if let Some(reason) = state.boundary {
                    format!("<p data-cua-id=challenge-state>needs_user_action:{reason}</p>")
                } else if authenticated && state.counts.accepted_logins != 0 {
                    state.counts.authenticated_requests += 1;
                    "<p data-cua-id=session-state>fixture-authenticated</p>".to_owned()
                } else {
                    "<form action=/login method=post><label for=user>fixture-user</label><input name=user id=user autocomplete=off><label for=password>fixture-password</label><input type=password name=password id=password autocomplete=off><button type=submit>fixture-submit</button></form><p data-cua-id=session-state>fixture-signed-out</p>".to_owned()
                };
                ("200 OK", "", html(&content))
            }
            ("POST", "/login") => {
                state.counts.submissions += 1;
                if state.boundary.is_some() {
                    state.counts.boundary_submissions += 1;
                    (
                        "409 Conflict",
                        "",
                        html("human boundary remains unresolved"),
                    )
                } else if bytes[header_end..header_end + content_length]
                    == format!("user=fixture-user&password={SECRET}").as_bytes()[..]
                {
                    state.counts.accepted_logins += 1;
                    (
                        "303 See Other",
                        "Location: /fixture\r\nSet-Cookie: dcc_fixture_session=authenticated-v1; HttpOnly; SameSite=Strict; Path=/\r\n",
                        String::new(),
                    )
                } else {
                    ("403 Forbidden", "", html("fixture-credentials-rejected"))
                }
            }
            _ => ("404 Not Found", "", html("fixture-not-found")),
        }
    };
    // Submitted bytes are never retained in the oracle, journal, or response.
    bytes.fill(0);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{extra}\r\n{page}",
        page.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn html(content: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><title>DCC controlled login fixture</title></head><body>{content}</body></html>"
    )
}
