use std::io::{Read, Write};

fn read_frame(reader: &mut impl Read) -> Vec<u8> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).unwrap();
    let mut body = vec![0_u8; u32::from_be_bytes(prefix) as usize];
    reader.read_exact(&mut body).unwrap();
    body
}

fn request_id(body: &[u8]) -> &str {
    let text = std::str::from_utf8(body).unwrap();
    let marker = "\"request_id\":\"";
    let start = text.find(marker).unwrap() + marker.len();
    let end = text[start..].find('"').unwrap() + start;
    &text[start..end]
}

fn write_response(writer: &mut impl Write, request_id: &str, response_type: &str) {
    let body = format!(
        "{{\"type\":\"{response_type}\",\"request_id\":\"{request_id}\",\"capabilities\":[]}}"
    );
    writer.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
    writer.write_all(body.as_bytes()).unwrap();
    writer.flush().unwrap();
}

fn main() {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let hello = read_frame(&mut input);
    write_response(&mut output, request_id(&hello), "hello");
    let ping = read_frame(&mut input);
    write_response(&mut output, request_id(&ping), "pong");
}
