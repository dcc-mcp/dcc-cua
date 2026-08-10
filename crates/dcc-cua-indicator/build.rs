use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn field<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(value, |current, name| {
        current
            .get(name)
            .unwrap_or_else(|| panic!("theme is missing {}", path.join(".")))
    })
}

fn string<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    field(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("theme field {} must be a string", path.join(".")))
}

fn integer(value: &Value, path: &[&str]) -> u64 {
    field(value, path)
        .as_u64()
        .unwrap_or_else(|| panic!("theme field {} must be an unsigned integer", path.join(".")))
}

fn color(value: &Value, path: &[&str]) -> (u8, u8, u8) {
    let encoded = field(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("theme field {} must be a color", path.join(".")));
    assert!(
        encoded.len() == 7 && encoded.starts_with('#'),
        "theme field {} must use #RRGGBB",
        path.join(".")
    );
    let component = |range| {
        u8::from_str_radix(&encoded[range], 16)
            .unwrap_or_else(|_| panic!("theme field {} must use #RRGGBB", path.join(".")))
    };
    (component(1..3), component(3..5), component(5..7))
}

fn write_tokens(theme: &Value, output: &Path) {
    let surface = color(theme, &["indicator", "surface"]);
    let line = color(theme, &["indicator", "line"]);
    let text = color(theme, &["indicator", "text"]);
    let muted = color(theme, &["indicator", "muted"]);
    let accent = color(theme, &["indicator", "accent"]);
    let recording = color(theme, &["indicator", "recording"]);
    let status_information = color(theme, &["indicator", "status", "information"]);
    let status_ready = color(theme, &["indicator", "status", "ready"]);
    let status_action = color(theme, &["indicator", "status", "action"]);
    let status_recording = color(theme, &["indicator", "status", "recording"]);
    let status_stopping = color(theme, &["indicator", "status", "stopping"]);
    let cursor_accent = color(theme, &["cursor", "accent"]);
    let cursor_theme_id = string(theme, &["cursor", "theme_id"]);
    assert!(
        !cursor_theme_id.trim().is_empty(),
        "cursor.theme_id must not be empty"
    );
    let reduced_motion = string(theme, &["cursor", "reduced_motion"]);
    assert!(
        matches!(reduced_motion, "auto" | "reduce" | "animate"),
        "cursor.reduced_motion must be auto, reduce, or animate"
    );
    assert_eq!(
        accent, cursor_accent,
        "cursor and indicator accent tokens must be identical"
    );

    let generated = format!(
        "\
pub(crate) const SURFACE: (u8, u8, u8) = {surface:?};\n\
pub(crate) const LINE: (u8, u8, u8) = {line:?};\n\
pub(crate) const TEXT: (u8, u8, u8) = {text:?};\n\
pub(crate) const MUTED: (u8, u8, u8) = {muted:?};\n\
pub(crate) const ACCENT: (u8, u8, u8) = {accent:?};\n\
pub(crate) const CURSOR_ACCENT: (u8, u8, u8) = {cursor_accent:?};\n\
pub(crate) const CURSOR_THEME_ID: &str = {cursor_theme_id:?};\n\
pub(crate) const REDUCED_MOTION: &str = {reduced_motion:?};\n\
pub(crate) const RECORDING: (u8, u8, u8) = {recording:?};\n\
pub(crate) const STATUS_INFORMATION: (u8, u8, u8) = {status_information:?};\n\
pub(crate) const STATUS_READY: (u8, u8, u8) = {status_ready:?};\n\
pub(crate) const STATUS_ACTION: (u8, u8, u8) = {status_action:?};\n\
pub(crate) const STATUS_RECORDING: (u8, u8, u8) = {status_recording:?};\n\
pub(crate) const STATUS_STOPPING: (u8, u8, u8) = {status_stopping:?};\n\
pub(crate) const FRAME_THICKNESS_DIP: i32 = {};\n\
pub(crate) const FRAME_GRADIENT_STEPS: usize = {};\n\
pub(crate) const FRAME_PULSE_PERIOD_MS: u64 = {};\n\
pub(crate) const FRAME_ALPHA_MIN: u8 = {};\n\
pub(crate) const FRAME_ALPHA_MAX: u8 = {};\n",
        integer(theme, &["indicator", "frame", "thickness_dip"]),
        integer(theme, &["indicator", "frame", "gradient_steps"]),
        integer(theme, &["indicator", "frame", "pulse_period_ms"]),
        integer(theme, &["indicator", "frame", "alpha_min"]),
        integer(theme, &["indicator", "frame", "alpha_max"]),
    );
    fs::write(output, generated).expect("write generated theme tokens");
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let theme_path = manifest.join("theme/dcc-cua-theme.json");
    println!("cargo:rerun-if-changed={}", theme_path.display());
    let theme: Value = serde_json::from_slice(&fs::read(&theme_path).expect("read shared theme"))
        .expect("parse shared theme");
    assert_eq!(field(&theme, &["schema_version"]).as_u64(), Some(1));
    write_tokens(
        &theme,
        &PathBuf::from(env::var_os("OUT_DIR").expect("build output")).join("theme_tokens.rs"),
    );
}
