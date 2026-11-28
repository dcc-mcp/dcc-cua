use super::NativeFrameSource;
use rstest::rstest;

#[rstest]
fn print_window_source_has_a_stable_debug_backend() {
    assert_eq!(
        NativeFrameSource::PrintWindow.backend(),
        "dcc-cua-print-window-exact-window"
    );
    assert_eq!(
        serde_json::to_string(&NativeFrameSource::PrintWindow).expect("serialize source"),
        "\"print_window\""
    );
}
