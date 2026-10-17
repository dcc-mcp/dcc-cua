//! Execute exact production ownership/hide/drop bodies with modeled Win32.
//! This is a portable safety regression, not a native desktop acceptance test.
use rstest::rstest;
use std::process::Command;

fn item<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source.find(marker).expect(marker);
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source[brace..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return &source[start..=brace + offset];
        }
    }
    panic!("unterminated production item");
}

#[rstest]
fn exact_source_owned_surface_instance_boundary() {
    let source = include_str!("../src/platform.rs");
    let model = include_str!("../src/tests/owned_surface_native.rs")
        .replace("//!", "//")
        .replace("use rstest::rstest;", "")
        .replace("#[rstest]", "#[test]");
    let declaration = source
        .lines()
        .find(|line| line.starts_with("struct OverlayWindow("))
        .unwrap();
    let counter = source
        .lines()
        .find(|line| line.starts_with("static NEXT_OVERLAY_INSTANCE:"))
        .unwrap();
    let hiding = source
        .split_once("let owned_input_hidden = input_suppression.hide_if_requested(|| {")
        .unwrap()
        .1
        .split_once("})?;")
        .unwrap()
        .0;
    let generated = format!(
        "#![allow(dead_code, non_snake_case, unused_unsafe, clippy::upper_case_acronyms)]\n\
        use std::sync::atomic::{{AtomicBool, AtomicUsize, Ordering}};\n\
        {counter}\n{declaration}\n{}\n{}\n{model}\n\
        fn hide_surfaces(overlay: OverlayWindow, frames: Vec<OverlayWindow>) -> bool {{\n\
        let visible = AtomicBool::new(true); let frame_visible = AtomicBool::new(true);\n\
        {hiding}\n}}",
        item(source, "impl OverlayWindow {"),
        item(source, "impl Drop for OverlayWindow {")
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-owned-surface-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("boundary.rs");
    let binary = directory.join(if cfg!(windows) {
        "boundary.exe"
    } else {
        "boundary"
    });
    std::fs::write(&path, generated).unwrap();
    let compile = Command::new("rustc")
        .args(["--edition=2024", "--test"])
        .arg(path)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let result = Command::new(binary)
        .args(["--test-threads=1", "--nocapture"])
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&result.stdout));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
