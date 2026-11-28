use super::*;
use rstest::rstest;

#[rstest]
#[ignore = "requires an interactive Windows desktop with DWM"]
fn compositor_accepts_the_owned_execution_frame() {
    let _frame = ExecutionFrame::new().unwrap();
}

#[rstest]
fn exterior_edge_must_fit_on_the_targets_monitor() {
    let monitor = MonitorGeometry {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
        work_left: -1920,
        work_top: 0,
        work_right: 0,
        work_bottom: 1040,
    };
    let full = TargetGeometry {
        x: -1920,
        y: 0,
        width: 1920,
        height: 1040,
        dpi: 96,
    };
    assert!(!has_outer_edge(full, monitor, 3));
    assert_eq!(monitor_edge(full, monitor, 3), Some([-1920, 1040, 1920, 3]));
    assert_eq!(
        monitor_edge(
            TargetGeometry {
                height: 1080,
                ..full
            },
            monitor,
            3
        ),
        None
    );
    assert!(has_outer_edge(
        TargetGeometry {
            height: 1037,
            ..full
        },
        monitor,
        3
    ));
    assert!(!has_outer_edge(
        TargetGeometry {
            x: 100,
            y: 2000,
            ..full
        },
        monitor,
        3
    ));
}
