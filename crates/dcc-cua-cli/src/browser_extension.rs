use std::io;
use std::path::Path;
use std::time::Duration;

use dcc_cua_client::HostClient;
use dcc_cua_protocol::MAX_JSON_FRAME_BYTES;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::cli_args::flag_value;

const EXTENSION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const NATIVE_HOST_NAME: &str = "com.dcc_mcp.dcc_cua";

pub(super) fn native_hello_ack() -> Value {
    json!({
        "schema": "dcc-cua.browser-extension.v1",
        "type": "hello_ack",
        "protocol": 1,
        "capabilities": ["native_host_bridge_v1"],
    })
}

#[derive(Clone, Copy)]
enum BrowserFamily {
    Chrome,
    Edge,
    Firefox,
}

impl BrowserFamily {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "chrome" => Ok(Self::Chrome),
            "edge" => Ok(Self::Edge),
            "firefox" => Ok(Self::Firefox),
            _ => Err("--browser must be chrome, edge, or firefox".into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
        }
    }
}

pub(crate) fn execute_management(flags: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = flags
        .first()
        .map(String::as_str)
        .ok_or("browser-extension requires plan, status, or install-native-host")?;
    let browser = flag_value(flags, "--browser")
        .ok_or("browser-extension requires --browser chrome|edge|firefox")?;
    let browser = BrowserFamily::parse(&browser)?;
    let extension_id = flag_value(flags, "--extension-id")
        .ok_or("browser-extension requires --extension-id PUBLISHED_ID")?;
    validate_extension_id(&extension_id)?;
    match subcommand {
        "install-native-host" => {
            let manifest_path = install_native_host(browser, &extension_id)?;
            stdoutln!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "type": "browser_extension_native_host_installed",
                    "browser": browser.name(),
                    "extension_id": extension_id,
                    "manifest_path": manifest_path,
                    "next_action": "install the signed store extension if needed, then click its action in the exact tab to pair",
                    "silent_extension_sideload": false,
                }))?
            );
        }
        "status" => {
            stdoutln!(
                "{}",
                serde_json::to_string_pretty(&native_host_status(browser, &extension_id)?)?
            );
        }
        "plan" => {
            let cdp_state = flag_value(flags, "--cdp-state").unwrap_or_else(|| "available".into());
            if !matches!(cdp_state.as_str(), "available" | "unavailable") {
                return Err("--cdp-state must be available or unavailable".into());
            }
            let status = native_host_status(browser, &extension_id)?;
            let registered = status["native_host_registered"].as_bool() == Some(true);
            let plan = if cdp_state == "available" {
                json!({
                    "provider": "cdp",
                    "reason": "CDP is available and remains the default browser provider",
                    "next_action": "reuse the current logical-task Host session",
                })
            } else if registered {
                json!({
                    "provider": "extension",
                    "reason": "CDP is unavailable and the exact native host identity is registered",
                    "next_action": "query browser_extension_status on the current logical-task session; if no provider is connected, ask the user to click the extension action in the exact tab",
                })
            } else {
                json!({
                    "provider": "extension",
                    "reason": "CDP is unavailable and the optional extension route is not registered",
                    "next_action": format!("run dcc-cua browser-extension install-native-host --browser {} --extension-id {} after the signed extension identity is known", browser.name(), extension_id),
                    "human_action_required": "ordinary users must install the signed browser-store extension; silent sideload is not permitted",
                })
            };
            stdoutln!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "type": "browser_provider_plan",
                    "browser": browser.name(),
                    "extension_id": extension_id,
                    "cdp_state": cdp_state,
                    "native_host": status,
                    "plan": plan,
                }))?
            );
        }
        _ => return Err("browser-extension requires plan, status, or install-native-host".into()),
    }
    Ok(())
}

pub(super) fn validate_extension_id(extension_id: &str) -> Result<(), String> {
    if extension_id.is_empty()
        || extension_id.len() > 128
        || !extension_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "@._-".contains(character))
    {
        return Err("extension id must contain 1..128 ASCII letters, digits, @, ., _, or -".into());
    }
    Ok(())
}

fn install_native_host(
    browser: BrowserFamily,
    extension_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let manifest_path = native_manifest_path(browser)?;
    let parent = manifest_path
        .parent()
        .ok_or("native messaging manifest path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let executable = std::fs::canonicalize(std::env::current_exe()?)?;
    let identity_field = match browser {
        BrowserFamily::Firefox => json!({"allowed_extensions": [extension_id]}),
        BrowserFamily::Chrome | BrowserFamily::Edge => json!({
            "allowed_origins": [format!("chrome-extension://{extension_id}/")]
        }),
    };
    let mut manifest = json!({
        "name": NATIVE_HOST_NAME,
        "description": "DCC-CUA browser extension native messaging bridge",
        "path": executable,
        "type": "stdio",
    });
    manifest
        .as_object_mut()
        .expect("manifest is an object")
        .extend(
            identity_field
                .as_object()
                .expect("identity is an object")
                .clone(),
        );
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    register_native_manifest(browser, &manifest_path)?;
    Ok(manifest_path.to_string_lossy().into_owned())
}

fn native_host_status(
    browser: BrowserFamily,
    extension_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let manifest_path = native_manifest_path(browser)?;
    let manifest = std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let exact_identity = manifest.as_ref().is_some_and(|manifest| match browser {
        BrowserFamily::Firefox => manifest["allowed_extensions"]
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == extension_id)),
        BrowserFamily::Chrome | BrowserFamily::Edge => manifest["allowed_origins"]
            .as_array()
            .is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value == &format!("chrome-extension://{extension_id}/"))
            }),
    });
    Ok(json!({
        "type": "browser_extension_native_host_status",
        "browser": browser.name(),
        "extension_id": extension_id,
        "manifest_path": manifest_path,
        "native_host_registered": exact_identity && native_manifest_registered(browser, &manifest_path),
        "extension_connection": "query Host browser_extension_status after opening the exact logical-task window session",
    }))
}

fn native_manifest_path(browser: BrowserFamily) -> Result<std::path::PathBuf, io::Error> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
            })?;
        return Ok(root
            .join("dcc-cua")
            .join("native-messaging")
            .join(format!("{}.json", browser.name())));
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is unavailable"))?;
        let directory = match browser {
            BrowserFamily::Chrome => {
                "Library/Application Support/Google/Chrome/NativeMessagingHosts"
            }
            BrowserFamily::Edge => {
                "Library/Application Support/Microsoft Edge/NativeMessagingHosts"
            }
            BrowserFamily::Firefox => "Library/Application Support/Mozilla/NativeMessagingHosts",
        };
        return Ok(home
            .join(directory)
            .join(format!("{NATIVE_HOST_NAME}.json")));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is unavailable"))?;
        let directory = match browser {
            BrowserFamily::Chrome => ".config/google-chrome/NativeMessagingHosts",
            BrowserFamily::Edge => ".config/microsoft-edge/NativeMessagingHosts",
            BrowserFamily::Firefox => ".mozilla/native-messaging-hosts",
        };
        return Ok(home
            .join(directory)
            .join(format!("{NATIVE_HOST_NAME}.json")));
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native messaging is unsupported on this platform",
    ))
}

fn register_native_manifest(
    browser: BrowserFamily,
    manifest_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        let vendor = match browser {
            BrowserFamily::Chrome => "Google\\Chrome",
            BrowserFamily::Edge => "Microsoft\\Edge",
            BrowserFamily::Firefox => "Mozilla",
        };
        let key = format!("HKCU\\Software\\{vendor}\\NativeMessagingHosts\\{NATIVE_HOST_NAME}");
        let output = crate::owned_process::command(
            crate::owned_process::OwnedConsoleChildRole::NativeMessagingRegistry,
            "reg.exe",
        )
        .args(["add", &key, "/ve", "/t", "REG_SZ", "/d"])
        .arg(manifest_path)
        .arg("/f")
        .output()?;
        if !output.status.success() {
            return Err(format!(
                "failed to register native messaging manifest: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
    }
    #[cfg(not(windows))]
    {
        let _ = browser;
        let _ = manifest_path;
    }
    Ok(())
}

fn native_manifest_registered(browser: BrowserFamily, manifest_path: &Path) -> bool {
    #[cfg(windows)]
    {
        let vendor = match browser {
            BrowserFamily::Chrome => "Google\\Chrome",
            BrowserFamily::Edge => "Microsoft\\Edge",
            BrowserFamily::Firefox => "Mozilla",
        };
        let key = format!("HKCU\\Software\\{vendor}\\NativeMessagingHosts\\{NATIVE_HOST_NAME}");
        let Ok(output) = crate::owned_process::command(
            crate::owned_process::OwnedConsoleChildRole::NativeMessagingRegistry,
            "reg.exe",
        )
        .args(["query", &key, "/ve"])
        .output() else {
            return false;
        };
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .contains(manifest_path.to_string_lossy().as_ref())
    }
    #[cfg(not(windows))]
    {
        let _ = browser;
        manifest_path.is_file()
    }
}

pub(crate) fn invocation_origin(arguments: &[String]) -> Option<String> {
    if let Some(origin) = arguments.first().filter(|argument| {
        argument.starts_with("chrome-extension://") || argument.starts_with("moz-extension://")
    }) {
        return Some((*origin).clone());
    }
    if arguments.len() >= 2
        && Path::new(&arguments[0])
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        && !arguments[1].starts_with('-')
    {
        return Some(arguments[1].clone());
    }
    None
}

pub(crate) async fn run_native_host(
    invocation_origin: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    let hello = read_native_message(&mut input).await?;
    let mut client = HostClient::connect_default("dcc-cua-browser-extension-native-host").await?;
    let registration = client
        .request(
            "register_browser_extension",
            json!({
                "hello": hello,
                "invocation_origin": invocation_origin,
                "browser_process_id": browser_process_id()?,
            }),
        )
        .await?;
    let provider_id = required_string(&registration.value, "provider_id")?.to_owned();
    let provider_secret = required_string(&registration.value, "provider_secret")?.to_owned();
    let poll_timeout_ms = registration.value["poll_timeout_ms"]
        .as_u64()
        .unwrap_or(5_000);
    write_native_message(&mut output, &native_hello_ack()).await?;

    let result = async {
        loop {
            let next = client
                .request(
                    "browser_extension_next",
                    json!({
                        "provider_id": provider_id,
                        "provider_secret": provider_secret,
                        "timeout_ms": poll_timeout_ms,
                    }),
                )
                .await?;
            let command = &next.value["command"];
            if command.is_null() {
                continue;
            }
            write_native_message(&mut output, command).await?;
            let response =
                tokio::time::timeout(EXTENSION_RESPONSE_TIMEOUT, read_native_message(&mut input))
                    .await
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "browser extension response timed out",
                        )
                    })??;
            client
                .request(
                    "complete_browser_extension",
                    json!({
                        "provider_id": provider_id,
                        "provider_secret": provider_secret,
                        "response": response,
                    }),
                )
                .await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    let _ = client
        .request(
            "unregister_browser_extension",
            json!({
                "provider_id": provider_id,
                "provider_secret": provider_secret,
            }),
        )
        .await;
    result
}

pub(super) async fn read_native_message<R>(
    reader: &mut R,
) -> Result<Value, Box<dyn std::error::Error>>
where
    R: AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).await?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_JSON_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("native message length must be between 1 and {MAX_JSON_FRAME_BYTES}"),
        )
        .into());
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

pub(super) async fn write_native_message<W>(
    writer: &mut W,
    value: &Value,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(value)?;
    if body.is_empty() || body.len() > MAX_JSON_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native response exceeds the bounded message size",
        )
        .into());
    }
    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, io::Error> {
    value[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Host response omitted {field}"),
            )
        })
}

fn browser_process_id() -> Result<u32, io::Error> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };

        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let current = std::process::id();
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        if unsafe { Process32FirstW(snapshot, &mut entry) } != 0 {
            loop {
                if entry.th32ProcessID == current {
                    found = Some(entry.th32ParentProcessID);
                    break;
                }
                if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                    break;
                }
            }
        }
        unsafe { CloseHandle(snapshot) };
        return found.filter(|process_id| *process_id > 0).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "browser parent process was not found",
            )
        });
    }
    #[cfg(unix)]
    {
        let process_id = unsafe { libc::getppid() };
        return u32::try_from(process_id)
            .ok()
            .filter(|process_id| *process_id > 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "browser parent process was not found",
                )
            });
    }
    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "browser process discovery is unsupported on this platform",
    ))
}
