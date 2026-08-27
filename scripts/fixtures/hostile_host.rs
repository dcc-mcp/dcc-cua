use std::io::{self, Write};

fn main() {
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
