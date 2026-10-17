use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use rstest::rstest;

fn captured(mut command: Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start local doctor");
    let drain = |mut stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut timed_out = false;
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = Output {
        status: child.wait().unwrap(),
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    };
    assert!(
        !timed_out,
        "diagnostics-only exceeded startup deadline: {output:?}"
    );
    output
}

fn isolated_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dcc-cua"));
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", home.join("local"))
        .env("APPDATA", home.join("roaming"))
        .env("CUA_DRIVER_CURSOR_THEME_DIR", home.join("themes"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("CUA_WAYLAND_NEST")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    command
}

fn assert_context_only_report(output: &Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["mode"], "diagnostics_only");
    assert_eq!(report["read_only"], true);
    assert_eq!(report["diagnostic_report_complete"], true);
    assert_eq!(report["ready"], false);
    assert_eq!(report["input_ready"], false);
    assert_eq!(report["native_acceptance"], "not_run");
    assert_eq!(report["backend"], "not_initialized");
    for check in [
        "sdk_constructor",
        "sdk_metadata",
        "sdk_inventory",
        "permissions",
        "health",
        "window_inventory",
    ] {
        assert_eq!(report["checks"][check]["status"], "not_run", "{check}");
    }
    if !cfg!(windows) {
        assert_eq!(report["checks"]["interactive_desktop"]["status"], "not_run");
    }
}

#[rstest]
fn local_diagnostics_only_completes_without_runtime_state_or_home_writes() {
    let home = tempfile::tempdir().unwrap();
    let mut command = isolated_command(home.path());
    command
        .args(["doctor", "--diagnostics-only"])
        // Ordinary SDK startup rejects this relative theme-store override.
        .env(
            "CUA_DRIVER_CURSOR_THEME_DIR",
            "forbidden-relative-theme-store",
        );

    assert_context_only_report(&captured(command));
    assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
}

#[rstest]
#[case(&["doctor", "--diagnostics-only", "--diagnostics-only"])]
#[case(&["doctor", "--diagnostics-only=true"])]
#[case(&["doctor", "--diagnostics-only="])]
#[case(&["doctor", "--diagnostics-only", "true"])]
#[case(&["doctor", "--diagnostics-only", "--endpoint=forbidden"])]
#[case(&["doctor", "--diagnostics-only", "--spawn=forbidden"])]
#[case(&["doctor", "--diagnostics-only", "--route=visual"])]
#[case(&["doctor", "--diagnostics-only", "--help"])]
#[case(&["doctor", "--diagnostics-only", "extra"])]
#[case(&["host", "--diagnostics-only"])]
#[case(&["version", "--diagnostics-only"])]
#[case(&["__capture-preparation-supervisor-v1", "--diagnostics-only"])]
fn invalid_diagnostics_only_arguments_exit_before_startup(#[case] arguments: &[&str]) {
    let home = tempfile::tempdir().unwrap();
    let mut command = isolated_command(home.path());
    command.args(arguments);

    let output = captured(command);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
}

#[cfg(target_os = "linux")]
#[rstest]
#[case::absent(None)]
#[case::empty(Some(""))]
#[case::enabled(Some("1"))]
fn linux_nesting_never_starts_a_compositor_or_writes_runtime_state(#[case] nesting: Option<&str>) {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let marker = fixture.path().join("compositor-started");
    let compositor = fixture.path().join("compositor-sentinel");
    std::fs::write(
        &compositor,
        "#!/bin/sh\nprintf invoked > \"$CUA_DIAGNOSTICS_COMPOSITOR_MARKER\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&compositor, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = isolated_command(home.path());
    command
        .args(["doctor", "--diagnostics-only"])
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env("CUA_WAYLAND_NEST_COMPOSITOR", &compositor)
        .env("CUA_DIAGNOSTICS_COMPOSITOR_MARKER", &marker);
    if let Some(value) = nesting {
        command.env("CUA_WAYLAND_NEST", value);
    }

    assert_context_only_report(&captured(command));
    assert!(
        !marker.exists(),
        "SDK inventory started the compositor sentinel"
    );
    assert!(std::fs::read_dir(runtime.path()).unwrap().next().is_none());
    assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
}

#[rstest]
fn manifest_advertises_local_context_without_native_acceptance() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcc-cua"))
        .arg("manifest")
        .output()
        .unwrap();
    assert!(output.status.success());
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mode = &manifest["runtime"]["local_diagnostics_only"];
    assert_eq!(
        mode["cli_command"],
        serde_json::json!(["doctor", "--diagnostics-only"])
    );
    assert_eq!(mode["additional_arguments_accepted"], false);
    assert_eq!(mode["sdk_initialized"], false);
    assert_eq!(mode["sdk_inventory"], "not_run");
    assert_eq!(mode["native_acceptance"], "not_run");
    assert_eq!(mode["ready"], false);
    assert_eq!(mode["input_ready"], false);
}
