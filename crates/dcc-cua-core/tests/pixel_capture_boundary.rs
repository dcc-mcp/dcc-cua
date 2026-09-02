//! Exact production capture and pre-publication paths, with no native OS calls.
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
    panic!("unterminated source item: {marker}");
}

#[rstest]
fn exact_source_first_capture_instance_boundary() {
    let pixel = include_str!("../src/runtime/pixel_observation.rs");
    let runtime = include_str!("../src/runtime.rs");
    let observation = include_str!("../src/runtime/session/observation.rs");
    let native = include_str!("../../dcc-cua-platform-windows/src/visible_capture.rs");
    let gates = include_str!("../src/runtime/session/gates.rs");
    let mut selected = String::new();
    for marker in [
        "pub(super) enum PixelObservationRoute",
        "pub(super) struct ExactWindowPixelGeometry",
        "pub(super) enum ExactWindowPixelCaptureMode",
        "pub(super) struct ExactWindowPixelInstanceIdentity",
        "pub(super) struct ExactWindowPixelPublicationFence",
    ] {
        selected.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n");
        selected.push_str(item(pixel, marker));
        selected.push('\n');
    }
    for marker in [
        "impl PixelObservationRoute",
        "impl From<dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence>",
        "impl ExactWindowPixelCaptureMode",
        "pub(super) fn validate_exact_window_pixel_target_state",
        "fn validate_final_exact_window_pixel_instance",
        "pub(super) fn validate_final_exact_window_pixel_publication",
        "pub(super) fn validate_native_exact_window_pixel_evidence",
    ] {
        selected.push_str(item(pixel, marker));
        selected.push('\n');
    }
    selected.push_str(item(runtime, "struct ExactWindowCapture"));
    selected.push_str(item(runtime, "fn bgra_has_visible_rgb"));
    selected.push_str(item(runtime, "async fn capture_exact_window("));
    selected.push_str(item(
        gates,
        "pub(crate) async fn gated_exact_window_observation",
    ));
    let mut native_types = String::new();
    for marker in [
        "pub struct ExactWindowPixelInstanceEvidence",
        "pub struct ExactWindowPixelEvidence",
    ] {
        native_types.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n");
        native_types.push_str(item(native, marker));
    }
    let mut methods = item(
        observation,
        "pub(crate) fn finish_observation_sensitive_attempt",
    )
    .to_owned();
    // Retain each real method through every native capture/final fence. Only
    // the successful serialization tail becomes a counted publication sink.
    for (marker, tail) in [
        (
            "async fn capture_window_pixels(",
            "        let (width, height) = png_dimensions",
        ),
        (
            "async fn capture_window_visually(",
            "        let data = final_capture.data;",
        ),
    ] {
        let method = item(observation, marker);
        methods.push_str(method.split_once(tail).expect("publication boundary").0);
        methods.push_str("self.publications += 1; Ok(final_capture) }\n");
    }
    let model = include_str!("../src/runtime/session/tests/pixel_capture_native.rs")
        .replace("use rstest::rstest;", "")
        .replace("#[rstest]", "#[test]");
    let source = format!(
        "#![allow(dead_code, unused_variables)]\nmod actual {{ {model}\n{selected}\nimpl ComputerUseSession {{ {methods} }}\nmod dcc_cua_platform_windows {{ use super::*; {native_types} NATIVE_BOUNDARY }} }}"
    ).replace("NATIVE_BOUNDARY", include_str!("../src/runtime/session/tests/pixel_capture_os.rs"))
        .replace("use rstest::rstest;", "")
        .replace("#[cfg(windows)]", "");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-pixel-boundary-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let input = directory.join("boundary.rs");
    let binary = directory.join(if cfg!(windows) {
        "boundary.exe"
    } else {
        "boundary"
    });
    std::fs::write(&input, source).unwrap();
    println!("preserved exact-source harness: {}", input.display());
    let compile = Command::new("rustc")
        .args(["--edition=2024", "--test"])
        .arg(input)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(binary)
        .args(["--test-threads=1", "--nocapture"])
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&run.stdout));
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
