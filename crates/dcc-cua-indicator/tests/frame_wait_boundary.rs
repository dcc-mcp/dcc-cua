//! Execute the two exact production wait sites with a deterministic OS/clock
//! boundary. This proves scheduling logic, not real Win32 delivery latency.
use rstest::rstest;
use std::process::Command;

#[rstest]
fn exact_source_presenter_frame_wait_boundary() {
    let platform = include_str!("../src/platform.rs");
    let banner = platform
        .split_once("fn run_banner(")
        .unwrap()
        .1
        .split_once("fn validate_target(")
        .unwrap()
        .0;
    let suppressed = banner
        .split_once("&mut cursor_exclusion,")
        .unwrap()
        .1
        .split_once(") {")
        .unwrap()
        .1
        .split_once("continue;")
        .unwrap()
        .0;
    let normal = banner
        .rsplit_once("frame_visible.store(false, Ordering::Release);")
        .unwrap()
        .1
        .split_once('}')
        .unwrap()
        .1
        .split_once('}')
        .unwrap()
        .0;
    let interval = platform
        .lines()
        .find(|line| line.starts_with("const FRAME_INTERVAL:"))
        .unwrap();
    // No copied scheduler: compile the complete production helper body when
    // present. The baseline uses the extracted thread::sleep calls directly.
    let helper_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform/frame_wait.rs");
    let helper = if helper_path.exists() {
        let source = std::fs::read_to_string(helper_path).unwrap();
        format!(
            "mod frame_wait {{ use super::*; pub(super) fn wait_for_frame{} }}",
            source.split_once("pub(super) fn wait_for_frame").unwrap().1
        )
    } else {
        String::new()
    };
    let model = include_str!("../src/tests/frame_wait_native.rs")
        .replace("use rstest::rstest;", "")
        .replace("#[rstest]", "#[test]");
    let generated = format!(
        r#"
#![allow(dead_code, non_snake_case, unused_unsafe, unused_variables, clippy::upper_case_acronyms)]
{model}
{interval}
{helper}
fn normal(stop: &AtomicBool, interrupted: &AtomicBool, runtime: &BannerRuntime) -> Result<(), IndicatorError> {{ {normal} Ok(()) }}
fn suppressed(stop: &AtomicBool, interrupted: &AtomicBool, runtime: &BannerRuntime) -> Result<(), IndicatorError> {{ {suppressed} Ok(()) }}
"#
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("dcc-cua-frame-wait-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let result = run_model(&directory, "boundary", &generated);
    println!("{}", String::from_utf8_lossy(&result.stdout));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    // Mutate only temporary compiled harnesses, never the author's source.
    // These executable controls show that wiring, fixed deadlines and stop
    // checks matter; successful compilation alone is not the regression gate.
    for (name, mutant) in [
        (
            "sleep-regression",
            generated.replace(
                "frame_wait::wait_for_frame(stop, interrupted, runtime)?;",
                "thread::sleep(FRAME_INTERVAL);",
            ),
        ),
        (
            "deadline-reset",
            generated
                .replace("let deadline = Instant::now() + FRAME_INTERVAL;", "")
                .replacen(
                    "loop {",
                    "loop { let deadline = Instant::now() + FRAME_INTERVAL;",
                    1,
                ),
        ),
        (
            "lost-stop-check",
            generated.replace("stop_requested(stop, interrupted, runtime)?", "false"),
        ),
    ] {
        assert_ne!(mutant, generated, "mutation must reach the production body");
        let result = run_model(&directory, name, &mutant);
        assert!(!result.status.success(), "undetected mutation: {name}");
        println!("rejected executable mutation: {name}");
    }
}

fn run_model(directory: &std::path::Path, name: &str, generated: &str) -> std::process::Output {
    let source = directory.join(format!("{name}.rs"));
    let binary = directory.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    });
    std::fs::write(&source, generated).unwrap();
    let compiled = Command::new("rustc")
        .args(["--edition=2024", "--test"])
        .arg(source)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    Command::new(binary)
        .args(["--test-threads=1", "--nocapture"])
        .output()
        .unwrap()
}
