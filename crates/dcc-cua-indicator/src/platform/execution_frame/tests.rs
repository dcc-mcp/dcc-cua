use super::*;
use rstest::rstest;

#[rstest]
#[ignore = "requires an interactive Windows desktop with DWM"]
fn compositor_accepts_the_owned_execution_frame() {
    let awareness = ThreadDpiAwareness::enter().unwrap();
    let frame = ExecutionFrame::new().unwrap();
    let cover = ExecutionFrame::new().unwrap();
    println!(
        "provider=dcc-cua runtime={} pid={} hwnd={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        frame.window.0.0 as usize
    );
    let monitor = read_monitor_geometry(frame.window.0).unwrap();
    let rect = [monitor.work_left + 32, monitor.work_top + 32, 32, 3];
    let show = |window| unsafe {
        SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
        .unwrap();
    };
    show(frame.window.0);
    let exposure = Exposure::new(&frame.window, Placement::Edge(rect), monitor, 3);
    assert!(exposure.exposed(), "uncovered native band");
    show(cover.window.0);
    drop(awareness);
    assert!(unsafe { IsWindowVisible(frame.window.0) }.as_bool());
    assert!(!exposure.exposed(), "higher native window covers the band");
    drop(cover);
    assert!(exposure.exposed(), "the original band is exposed again");
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
