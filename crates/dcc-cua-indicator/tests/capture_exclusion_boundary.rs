//! Compile the exact production coordinator/identity code with in-memory
//! Win32 and synchronization boundaries. No native windows or Host are started.
//! The production deadline is retained; only OS calls and lease acquisition
//! are modeled. This also runs on non-Windows CI.
use rstest::rstest;
use std::process::Command;

fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    text.split_once(start)
        .expect("source start")
        .1
        .split_once(end)
        .expect("source end")
        .0
}

#[rstest]
fn exact_source_capture_exclusion_native_boundary() {
    let source = include_str!("../src/platform/capture_exclusion.rs");
    let identity = include_str!("../src/platform/cursor_registration.rs");
    let model = include_str!("../src/tests/capture_exclusion_native.rs")
        .replace("#![allow(non_snake_case)]", "")
        .replace("use rstest::rstest;", "")
        .replace("#[rstest]", "#[test]");
    let (native, tests) = model
        .split_once("fn setup(")
        .expect("native model boundary");
    let classification = between(
        source,
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]",
        "/// A crash-safe",
    );
    let coordinator = source
        .split_once("struct VisibleOverlayEnumeration")
        .expect("coordinator")
        .1;
    let constants = source
        .lines()
        .filter(|line| {
            [
                "const CAPTURE_EXCLUSION_TIMEOUT:",
                "const BANNER_CLASS_NAME:",
                "const FRAME_CLASS_NAME:",
                "const CURSOR_CLASS_NAME:",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        constants.len(),
        4,
        "use the production deadline and classifier constants"
    );
    let constants = constants.join("\n");
    let identity = identity
        .split_once("static RENDERER_ID:")
        .expect("identity")
        .1;
    let generated = format!(
        r#"
#![allow(dead_code, non_snake_case, unused_unsafe, clippy::upper_case_acronyms)]
mod native {{
{native}
}}
fn register_cursor_renderer_id(id: String) -> String {{ platform::cursor_registration::register(id) }}
mod platform {{
pub(crate) mod cursor_registration {{
use std::sync::OnceLock;
use crate::native::*;
static RENDERER_ID:{identity}
}}
mod capture_exclusion {{
use std::sync::{{OnceLock, atomic::{{AtomicBool, AtomicU64, AtomicUsize, Ordering}}}};
use std::time::{{Duration, Instant}};
use std::thread;
use crate::native::*;
use super::cursor_registration::{{OwnedCursorWindow, registered_window}};
{constants}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]{classification}
struct VisibleOverlayEnumeration{coordinator}
mod tests {{
use super::*;
use crate::platform::capture_exclusion;
use crate::native::*;
fn setup({tests}
}}
}}
}}
"#
    );
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "dcc-cua-overlay-boundary-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let source_path = directory.join("native-boundary.rs");
    let binary = directory.join(if cfg!(windows) {
        "native-boundary.exe"
    } else {
        "native-boundary"
    });
    std::fs::write(&source_path, generated).unwrap();
    let compile = Command::new("rustc")
        .args(["--edition=2024", "--test"])
        .arg(&source_path)
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
