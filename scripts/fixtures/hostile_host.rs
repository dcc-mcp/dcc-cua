use std::io::{self, Read, Write};

fn main() {
    let mut request_length = [0_u8; 4];
    let mut stdin = io::stdin().lock();
    stdin
        .read_exact(&mut request_length)
        .expect("read request frame prefix");
    let request_length = u32::from_be_bytes(request_length) as usize;
    let mut request = vec![0_u8; request_length];
    stdin
        .read_exact(&mut request)
        .expect("read complete request frame");

    eprintln!("CHILD_PRIVATE_DIAGNOSTIC_7e87d1 C:\\private\\credential.txt");
    io::stderr()
        .write_all(&vec![b'x'; 64 * 1024])
        .expect("write oversized private diagnostics");
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&(4_u32 * 1024 * 1024 + 1).to_be_bytes())
        .expect("write oversized frame prefix");
    stdout.flush().expect("flush oversized frame prefix");
}
