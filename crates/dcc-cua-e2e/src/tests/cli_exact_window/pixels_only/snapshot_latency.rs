//! Manual native measurements against disposable project-owned windows.
use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
#[ignore = "interactive desktop required; publish binding before allowing captures"]
async fn owned_window_snapshot_latency() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("DCC_CUA_LATENCY_DIRECTORY").expect("private measurement directory"),
    );
    assert!(
        directory.read_dir().unwrap().next().is_none(),
        "use a fresh directory"
    );
    let fixture = Fixture::new();
    let pid = std::process::id();
    let binding = json!({
        "provider":"dcc-cua", "runtime":env!("CARGO_PKG_VERSION"),
        "pid":pid, "hwnd":fixture.target,
    });
    std::fs::write(directory.join("binding.json"), binding.to_string()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    while !directory.join("begin").exists() {
        assert!(Instant::now() < deadline, "binding was not acknowledged");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!("{binding}");
    let before = fixture.counters();
    let driver = ComputerUseDriver::create().unwrap();
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(pid),
                window_handle: Some(fixture.target),
                window_title: None,
            },
            "owned latency fixture",
            "owned-latency-fixture",
        )
        .unwrap();
    let start = Instant::now();
    bounded_session(session.start_pixels_only()).await.unwrap();
    let startup_us = start.elapsed().as_micros();
    let mut measurements = Vec::new();
    for (width, height, diagnostics) in [
        (280, 220, false),
        (280, 220, true),
        (1280, 720, false),
        (1280, 720, true),
    ] {
        fixture.resize_input_roots(windows_sys::Win32::Foundation::RECT {
            left: 80,
            top: 100,
            right: 80 + width,
            bottom: 100 + height,
        });
        let mut samples = Vec::new();
        for index in 0..9 {
            let started = Instant::now();
            let capture =
                bounded_session(session.screenshot_pixels_only_with_diagnostics(diagnostics))
                    .await
                    .unwrap();
            let elapsed_us = started.elapsed().as_micros();
            assert_eq!(capture.observation.process_id, pid);
            assert_eq!(capture.observation.window_handle, fixture.target);
            assert_eq!(capture.observation.width, width as u32);
            assert_eq!(capture.observation.height, height as u32);
            assert!(!capture.data.is_empty());
            let mut png = png::Decoder::new(capture.data.as_slice())
                .read_info()
                .unwrap();
            let mut pixels = vec![0; png.output_buffer_size()];
            let info = png.next_frame(&mut pixels).unwrap();
            assert_eq!((info.width, info.height), (width as u32, height as u32));
            assert!(pixels[..info.buffer_size()].iter().all(|byte| *byte == 255));
            // The first native capture warms the graphics path and is reported separately.
            samples.push(json!({"index":index, "elapsed_us":elapsed_us,
                "diagnostics":capture.observation.capture_provenance["capture_diagnostics"]}));
        }
        measurements.push(json!({"width":width,"height":height,
            "capture_diagnostics":diagnostics,"samples":samples}));
    }
    let stopped = bounded_session(session.stop()).await.unwrap();
    assert!(stopped.success && !stopped.active && !stopped.cleanup_pending);
    assert_eq!(
        fixture.counters(),
        before,
        "capture entered accessibility or input"
    );
    let report = json!({"binding":binding,"route":"native SDK pixels_only",
        "startup_us":startup_us,"measurements":measurements,"cleanup":true});
    std::fs::write(directory.join("results.json"), report.to_string()).unwrap();
    eprintln!("native snapshot measurements complete; exact ownership and cleanup verified");
}
