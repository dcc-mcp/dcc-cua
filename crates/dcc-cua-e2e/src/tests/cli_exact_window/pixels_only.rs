//! Public-route coverage: native execution is opt-in controlled Windows CI only.
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use dcc_cua_core::{ComputerUseDriver, ComputerUseErrorCode, ComputerUseTargetScope};
use rstest::rstest;
use serde_json::{Value, json};

mod inventory;
mod native_fixture;
use native_fixture::{Fixture, cursor_position, foreground, visible_windows_for};

fn pixel_arguments(pid: u32, hwnd: u64, output: &Path) -> Vec<String> {
    vec![
        "--pid".into(),
        pid.to_string(),
        "--window-id".into(),
        hwnd.to_string(),
        "--pixels-only".into(),
        "--session".into(),
        "pixels-only-cli-fixture".into(),
        "--output".into(),
        output.to_string_lossy().into_owned(),
    ]
}

struct ReapedCli(std::process::Child);

impl Drop for ReapedCli {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bounded_cli(binary: &Path, arguments: &[String]) -> Output {
    let mut owner = ReapedCli(
        Command::new(binary)
            .arg("snapshot")
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("external snapshot"),
    );
    let child = &mut owner.0;
    let pid = child.id();
    let read = |pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(1_048_577)
                .read_to_end(&mut bytes)
                .expect("bounded CLI output");
            bytes
        })
    };
    let stdout = read(Box::new(child.stdout.take().unwrap()));
    let stderr = read(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + Duration::from_secs(30);
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().expect("poll external snapshot") {
            break (status, false);
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate timed-out CLI fixture");
            break (child.wait().expect("reap timed-out CLI fixture"), true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = Output {
        status,
        stdout: stdout.join().expect("stdout reader"),
        stderr: stderr.join().expect("stderr reader"),
    };
    assert_eq!(
        visible_windows_for(pid),
        0,
        "CLI left a session/overlay window"
    );
    assert!(
        !timed_out,
        "CLI exceeded its fixture budget; child reaped: {output:?}"
    );
    assert!(output.stdout.len() <= 1_048_576 && output.stderr.len() <= 1_048_576);
    output
}

fn receipt(stdout: &[u8], stderr: &[u8]) -> Result<Value, String> {
    if !stderr.is_empty() {
        return Err("stderr must remain empty".into());
    }
    // from_slice rejects a second receipt or arbitrary trailing stdout.
    serde_json::from_slice(stdout).map_err(|error| error.to_string())
}

fn validate_pixels(value: &Value, pid: u32, hwnd: u64) -> Result<(), &'static str> {
    let observation = &value["observation"];
    let provenance = &observation["capture_provenance"];
    if value["success"] != true
        || value["observation_mode"] != "pixels_only"
        || !value["activation"].is_null()
        || value["node_count"] != 0
        || observation["process_id"] != pid
        || observation["window_handle"] != hwnd
        || provenance["process_id"] != pid
        || provenance["window_handle"] != hwnd
        || provenance["observation_mode"] != "pixels_only"
        || provenance["accessibility_available"] != false
        || provenance["degraded"] != false
        || provenance["pixels_captured"] != true
        || provenance["scope"] != "window"
        || provenance["whole_desktop_capture"] != false
        || value["accessibility"]["observation_mode"] != "pixels_only"
        || value["accessibility"]["accessibility_available"] != false
    {
        return Err("not an exact provider-free pixel receipt");
    }
    Ok(())
}

fn validate_png(path: &Path, value: &Value) {
    let data = std::fs::read(path).expect("published PNG");
    let mut reader = png::Decoder::new(data.as_slice())
        .read_info()
        .expect("valid PNG header");
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).expect("complete PNG pixels");
    assert_eq!(value["observation"]["width"], info.width);
    assert_eq!(value["observation"]["height"], info.height);
    assert!(info.width > 100 && info.height > 100);
    let channels = info.color_type.samples();
    let center =
        (info.height as usize / 2 * info.width as usize + info.width as usize / 2) * channels;
    assert!(
        pixels[center..center + 3].iter().all(|value| *value >= 240),
        "custom target's white center must survive capture without an overlay"
    );
}

async fn bounded_session<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("public session operation exceeded its fixture budget")
}

fn assert_failure(output: &Output, path: &Path, expected_code: &str) {
    assert!(
        !output.status.success(),
        "capture/identity failure must fail closed"
    );
    assert_eq!(
        receipt(&output.stdout, &output.stderr).expect("single redacted failure"),
        json!({"success":false, "error":{
            "code":expected_code, "message":"dcc-cua could not complete the command"
        }})
    );
    assert_eq!(
        std::str::from_utf8(&output.stdout).unwrap().lines().count(),
        1
    );
    assert!(!path.exists(), "failed capture published PNG bytes");
}

#[rstest]
fn pixels_only_failure_receipts_require_the_exact_case_code() {
    use std::os::windows::process::ExitStatusExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unpublished.png");
    // Replay the observed wrong-owner envelope and the source-traced occlusion
    // envelope through the same assertion used by the external CLI fixture.
    for actual_code in ["target_unavailable", "invalid_target"] {
        let output = Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: format!(
                "{{\"success\":false,\"error\":{{\"code\":\"{actual_code}\",\"message\":\"dcc-cua could not complete the command\"}}}}\n"
            )
            .into_bytes(),
            stderr: Vec::new(),
        };
        for expected_code in ["target_unavailable", "invalid_target", "command_failed"] {
            let accepted = std::panic::catch_unwind(|| {
                assert_failure(&output, &path, expected_code);
            })
            .is_ok();
            assert_eq!(accepted, actual_code == expected_code);
        }
    }
}

#[rstest]
fn pixels_only_receipt_controls_detect_output_and_wrong_route_mutations() {
    let good = json!({
        "success":true, "observation_mode":"pixels_only", "activation":null, "node_count":0,
        "observation":{"process_id":42,"window_handle":77,"capture_provenance":{
            "process_id":42,"window_handle":77,"observation_mode":"pixels_only",
            "accessibility_available":false,"degraded":false,"pixels_captured":true,
            "scope":"window","whole_desktop_capture":false
        }},
        "accessibility":{"observation_mode":"pixels_only","accessibility_available":false}
    });
    assert!(validate_pixels(&good, 42, 77).is_ok());
    for (pointer, replacement) in [
        ("/observation_mode", json!("accessibility_preferred")),
        (
            "/observation/capture_provenance/observation_mode",
            json!("accessibility_unavailable_degraded"),
        ),
        ("/observation/capture_provenance/process_id", json!(43)),
        ("/observation/window_handle", json!(78)),
        (
            "/observation/capture_provenance/whole_desktop_capture",
            json!(true),
        ),
        ("/activation", json!({"success":true})),
    ] {
        let mut mutated = good.clone();
        *mutated.pointer_mut(pointer).unwrap() = replacement;
        assert!(validate_pixels(&mutated, 42, 77).is_err(), "{pointer}");
    }
    let bytes = serde_json::to_vec(&good).unwrap();
    assert!(receipt(&bytes, &[]).is_ok());
    assert!(receipt(&[bytes.as_slice(), b"\n{}"].concat(), &[]).is_err());
    assert!(receipt(&bytes, b"private diagnostic").is_err());
}

#[rstest]
#[tokio::test]
async fn external_pixels_only_cli_covers_provider_free_capture_failure_and_cleanup() {
    let binary = std::path::PathBuf::from(
        std::env::var_os("DCC_CUA_E2E_BINARY").expect("CI-built external CLI"),
    );
    let fixture = Fixture::new();
    let pid = std::process::id();
    eprintln!(
        "provider=dcc-cua runtime={} target_pid={pid} target_hwnd={} decoy_hwnd={}",
        env!("CARGO_PKG_VERSION"),
        fixture.target,
        fixture.decoy
    );
    assert_eq!(
        dcc_cua_platform_windows::exact_window_capture_route(pid, fixture.target).unwrap(),
        dcc_cua_platform_windows::ExactWindowCaptureRoute::VerifiedVisible,
        "two custom roots force independently fenced visible pixels"
    );
    let before = (fixture.counters(), foreground(), cursor_position());
    let initial = inventory::read(pid, "fixture-ready");
    let fixtures = [
        (fixture.target, "pixels-only custom target", true),
        (fixture.decoy, "pixels-only custom decoy", true),
        (fixture.blocker, "pixels-only capture blocker", false),
    ]
    .map(|(hwnd, title, visible)| {
        let window = initial
            .iter()
            .find(|window| window.identity.hwnd == hwnd)
            .expect("exact fixture root")
            .clone();
        assert_eq!(window.identity.class, "DccCuaPixelsOnlyFixture");
        assert_eq!(window.identity.title, title);
        assert_eq!(window.visible, visible);
        window
    });
    let directory = tempfile::tempdir().expect("scoped PNG outputs");
    let image_path = directory.path().join("pixels-only.png");

    // Real executable dispatch -> start_pixels_only -> screenshot_pixels_only -> stop -> output.
    let arguments = pixel_arguments(pid, fixture.target, &image_path);
    let output = bounded_cli(&binary, &arguments);
    assert!(
        output.status.success(),
        "public pixels-only route: {output:?}"
    );
    let value = receipt(&output.stdout, &output.stderr).expect("single success JSON");
    validate_pixels(&value, pid, fixture.target).expect("truthful explicit route");
    assert_eq!(
        value["observation"]["capture_backend"],
        "dcc-cua-visible-exact-window"
    );
    assert_eq!(
        value["observation"]["capture_provenance"]["fallback"],
        "same_executable_multi_window_exact_visible_proof"
    );
    assert_eq!(
        value["observation"]["source_rect"],
        value["observation"]["capture_provenance"]["desktop_crop_bounds"]
    );
    assert_eq!(value["output"], image_path.to_string_lossy().as_ref());
    validate_png(&image_path, &value);
    assert_eq!(
        (fixture.counters(), foreground(), cursor_position()),
        before,
        "explicit capture entered a provider or changed activation/input"
    );

    let wrong_path = directory.path().join("wrong-owner.png");
    let wrong = bounded_cli(
        &binary,
        &pixel_arguments(pid.wrapping_add(1), fixture.target, &wrong_path),
    );
    assert_failure(&wrong, &wrong_path, "target_unavailable");

    fixture.block_capture(true);
    let blocked_path = directory.path().join("blocked.png");
    let blocked = bounded_cli(
        &binary,
        &pixel_arguments(pid, fixture.target, &blocked_path),
    );
    assert_failure(&blocked, &blocked_path, "invalid_target");
    eprintln!("pixels-only external success, wrong-owner and occluded-capture receipts passed");
    fixture.block_capture(false);
    assert_eq!(
        (fixture.counters(), foreground(), cursor_position()),
        before
    );

    // Exercise the same public session methods with the owner still alive, so
    // cleanup cannot be satisfied merely by OS teardown of the CLI process.
    let renderer_id = dcc_cua_indicator::register_cursor_renderer_id(format!(
        "pixels-only-{}",
        directory.path().file_name().unwrap().to_string_lossy()
    ));
    // The existing API's ACTUAL returned token is what driver_factory adopts.
    let mut ownership = inventory::Contract::new(fixtures, &renderer_id);
    ownership
        .check_native(pid, "before-driver", inventory::Phase::Fixture, false)
        .expect("fixture roots survived external calls and blocker was hidden");
    let driver = ComputerUseDriver::create().expect("controlled embedded driver");
    assert!(driver.upstream_cursor_renderer_enabled());
    ownership
        .check_native(pid, "before-session-start", inventory::Phase::Driver, false)
        .expect("only the configured driver cursor may join the fixture roots");
    let mut session = driver
        .session(
            ComputerUseTargetScope {
                process_id: Some(pid),
                window_handle: Some(fixture.target),
                window_title: None,
            },
            "custom pixel fixture",
            "pixels-only-session-fixture",
        )
        .unwrap();
    let started = bounded_session(session.start_pixels_only())
        .await
        .expect("explicit session start");
    assert_eq!(started["success"], true);
    assert_eq!(started["upstream_session"]["state"], "visual_only");
    let active_status = session.status();
    for guarded in [false, true] {
        let error = if guarded {
            bounded_session(session.start_with_request(&Default::default())).await
        } else {
            bounded_session(session.start()).await
        }
        .expect_err("repeated startup must reject without changing the active pixel route");
        assert_eq!(error.code, ComputerUseErrorCode::InvalidAction);
        let status = session.status();
        assert_eq!(status["active"], true);
        for field in [
            "target",
            "upstream_session",
            "session_id",
            "latest_observation_id",
        ] {
            assert_eq!(
                status[field], active_status[field],
                "rejected startup changed {field}"
            );
        }
        assert_eq!(
            (fixture.counters(), foreground(), cursor_position()),
            before
        );
    }
    let label = started["banner"]["label"]
        .as_str()
        .expect("exact presenter label");
    ownership
        .check_native(
            pid,
            "success-session-active",
            inventory::Phase::Active(label),
            false,
        )
        .expect("exact session resources and configured cursor");
    let screenshot = bounded_session(session.screenshot_pixels_only())
        .await
        .expect("explicit session pixels");
    assert_eq!(
        screenshot.observation.capture_provenance["observation_mode"],
        "pixels_only"
    );
    assert_eq!(screenshot.observation.process_id, pid);
    assert_eq!(screenshot.observation.window_handle, fixture.target);
    assert_eq!(screenshot.observation.capture_provenance["degraded"], false);
    assert_eq!(
        screenshot.observation.capture_provenance["accessibility_available"],
        false
    );
    eprintln!(
        "pixels-only repeated start and guarded start rejected with InvalidAction; subsequent exact screenshot retained nondegraded pixels_only provenance"
    );
    ownership
        .check_native(
            pid,
            "success-before-stop",
            inventory::Phase::Active(label),
            false,
        )
        .expect("bind resources before stop, never by post-stop allowance");
    let stopped = bounded_session(session.stop())
        .await
        .expect("stop after success");
    assert!(stopped.success && !stopped.active && !stopped.marker.visible);
    ownership.check_native(pid, "success-after-stop", inventory::Phase::Stopped, false)
        .expect("success stop must remove every session resource and preserve exact driver/fixture roots");
    let restarted = bounded_session(session.start_pixels_only())
        .await
        .expect("restart same exact session");
    fixture.block_capture(true);
    let label = restarted["banner"]["label"]
        .as_str()
        .expect("exact restarted presenter label");
    ownership
        .check_native(
            pid,
            "blocked-session-active",
            inventory::Phase::Active(label),
            true,
        )
        .expect("exact blocker and restarted session resources");
    let error = bounded_session(session.screenshot_pixels_only())
        .await
        .expect_err("occluded capture must fail");
    assert_eq!(error.code, ComputerUseErrorCode::InvalidTarget);
    let stopped = bounded_session(session.stop())
        .await
        .expect("stop after capture failure");
    assert!(stopped.success && !stopped.active && !stopped.cleanup_pending);
    assert!(stopped.cleanup_issues.is_empty() && !stopped.marker.visible);
    ownership
        .check_native(pid, "error-after-stop", inventory::Phase::Stopped, true)
        .expect(
            "error stop must remove every session resource while owner and blocker remain alive",
        );
    assert!(
        bounded_session(session.screenshot_pixels_only())
            .await
            .is_err(),
        "stopped session cannot capture"
    );
    fixture.block_capture(false);
    ownership
        .check_native(pid, "error-after-unblock", inventory::Phase::Stopped, false)
        .expect("exact fixture restoration, no retained/new session resources");
    assert_eq!(
        (fixture.counters(), foreground(), cursor_position()),
        before
    );

    // Positive detector control / wrong-dispatch sensitivity: the ordinary
    // external route actually enters accessibility, unlike --pixels-only.
    let ordinary_path = directory.path().join("ordinary-control.png");
    let mut ordinary = pixel_arguments(pid, fixture.target, &ordinary_path);
    ordinary.retain(|arg| arg != "--pixels-only");
    let ordinary_output = bounded_cli(&binary, &ordinary);
    assert!(
        fixture.counters().0 > before.0.0,
        "provider request detector was never exercised"
    );
    eprintln!(
        "pixels-only public session success/error cleanup passed; ordinary-route provider detector requests={}",
        fixture.counters().0 - before.0.0
    );
    if ordinary_output.status.success() {
        let value = receipt(&ordinary_output.stdout, &ordinary_output.stderr).unwrap();
        assert!(
            validate_pixels(&value, pid, fixture.target).is_err(),
            "ordinary dispatch must not pass explicit-pixels postconditions"
        );
    }
}
