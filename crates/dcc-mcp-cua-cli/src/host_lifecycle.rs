use std::process::{Child, Command, Stdio};
use std::time::Duration;

use dcc_mcp_cua_client::HostClient;
use serde_json::{Value, json};
use tokio::time::Instant;

const HOST_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const HOST_START_TIMEOUT: Duration = Duration::from_secs(15);
const HOST_START_RETRY_MS: u64 = 100;

pub(crate) async fn ensure(
    endpoint: String,
    host_args: &[String],
) -> Result<Value, Box<dyn std::error::Error>> {
    if let Ok(ping) = ping(&endpoint).await {
        return Ok(ready_response("existing", &endpoint, None, ping));
    }

    prepare_detached_spawn()?;
    let binary = std::env::current_exe()?;
    let mut child = host_command(&binary, host_args).spawn()?;
    let child_pid = child.id();
    let deadline = Instant::now() + HOST_START_TIMEOUT;
    loop {
        if let Ok(ping) = ping(&endpoint).await {
            let running = child.try_wait()?.is_none();
            return Ok(ready_response(
                if running { "started" } else { "existing" },
                &endpoint,
                running.then_some(child_pid),
                ping,
            ));
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("Host exited before the endpoint was ready: {status}").into());
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(HOST_START_RETRY_MS)).await;
    }

    stop_failed_child(&mut child);
    Err(format!("Host endpoint did not become ready: {endpoint}").into())
}

fn host_command(binary: &std::path::Path, host_args: &[String]) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("host")
        .args(host_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

#[cfg(windows)]
fn prepare_detached_spawn() -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(kind) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn prepare_detached_spawn() -> std::io::Result<()> {
    Ok(())
}

async fn ping(endpoint: &str) -> Result<Value, String> {
    tokio::time::timeout(HOST_PROBE_TIMEOUT, async {
        let mut client =
            HostClient::connect(endpoint.to_owned(), "dcc-mcp-cua-host-ensure").await?;
        Ok::<_, dcc_mcp_cua_client::HostClientError>(client.ping().await?.value)
    })
    .await
    .map_err(|_| format!("Host endpoint probe timed out: {endpoint}"))?
    .map_err(|error| error.to_string())
}

fn ready_response(status: &str, endpoint: &str, pid: Option<u32>, ping: Value) -> Value {
    json!({
        "type": "host_ready",
        "status": status,
        "endpoint": endpoint,
        "pid": pid,
        "protocol_version": ping["protocol_version"],
        "host_version": ping["host_version"],
    })
}

fn stop_failed_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
