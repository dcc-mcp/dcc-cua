use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

fn option_value(name: &str) -> String {
    let arguments = std::env::args().collect::<Vec<_>>();
    let index = arguments
        .iter()
        .position(|argument| argument == name)
        .unwrap_or_else(|| panic!("missing {name}"));
    arguments[index + 1].clone()
}

fn read_frame(reader: &mut impl Read) -> Vec<u8> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).unwrap();
    let mut body = vec![0_u8; u32::from_be_bytes(prefix) as usize];
    reader.read_exact(&mut body).unwrap();
    body
}

fn request_id(body: &[u8]) -> String {
    let text = std::str::from_utf8(body).unwrap();
    let marker = "\"request_id\":\"";
    let start = text.find(marker).unwrap() + marker.len();
    let end = text[start..].find('"').unwrap() + start;
    text[start..end].to_owned()
}

fn write_hello(writer: &mut impl Write, request_id: &str) {
    let body = format!(
        "{{\"type\":\"hello\",\"request_id\":\"{request_id}\",\"capabilities\":[]}}"
    );
    writer.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    writer.write_all(body.as_bytes()).unwrap();
    writer.flush().unwrap();
}

fn main() {
    let pid_file = PathBuf::from(option_value("--pid-file"));
    let mode = option_value("--fixture-mode");
    fs::write(pid_file, std::process::id().to_string()).unwrap();
    eprintln!("FIXTURE_PRIVATE_DIAGNOSTIC");
    if mode == "shutdown" {
        let request = read_frame(&mut std::io::stdin().lock());
        write_hello(&mut std::io::stdout().lock(), &request_id(&request));
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
