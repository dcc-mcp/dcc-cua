//! No native call: compile the exact production arbiter with an in-memory OS gate.
use rstest::rstest;
use std::process::Command;

#[rstest]
fn exact_source_cooperative_capture_survives_long_input_with_two_consumers() {
    let leases = include_str!("../src/input_capture.rs")
        .replace("//!", "//")
        .replace("#[cfg(test)]\nmod tests;", "")
        .replace("#[cfg(test)]\r\nmod tests;", "");
    let coordinator = include_str!("../src/platform/cooperative_capture.rs").replace("//!", "//");
    let model = include_str!("../src/tests/cooperative_capture_native.rs")
        .replace("//!", "//")
        .replace("use rstest::rstest;", "")
        .replace("#[rstest]", "#[test]")
        .replace(
            "// PRODUCTION_COORDINATOR",
            &format!("pub mod cooperative_capture {{ {coordinator} }}"),
        );
    let generated = format!("#![allow(dead_code)]\nmod input_capture {{ {leases} }}\n{model}");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-cooperative-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let source = directory.join("boundary.rs");
    let binary = directory.join(if cfg!(windows) {
        "boundary.exe"
    } else {
        "boundary"
    });
    std::fs::write(&source, generated).unwrap();
    let compile = Command::new("rustc")
        .args(["--edition=2024", "--test"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&binary)
        .arg("--test-threads=1")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let pdb = binary.with_extension("pdb");
    if pdb.is_file() {
        std::fs::remove_file(pdb).unwrap();
    }
    std::fs::remove_file(binary).unwrap();
    std::fs::remove_file(source).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
