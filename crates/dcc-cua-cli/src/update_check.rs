use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const FAILURE_RETRY: Duration = Duration::from_secs(60 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const FINISH_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RELEASE_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Default, Deserialize, Serialize)]
struct UpdateCheckCache {
    #[serde(default)]
    next_check_unix_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latest_version: Option<String>,
}

pub(crate) fn start(arguments: &[String]) -> Option<tokio::task::JoinHandle<Option<String>>> {
    automatic_check_enabled(arguments).then(|| tokio::spawn(check_for_update()))
}

pub(crate) async fn finish(mut handle: tokio::task::JoinHandle<Option<String>>) {
    if let Ok(Ok(Some(message))) = tokio::time::timeout(FINISH_TIMEOUT, &mut handle).await {
        eprintln!("{message}");
    }
    if !handle.is_finished() {
        handle.abort();
    }
}

fn automatic_check_enabled(arguments: &[String]) -> bool {
    command_allows_check(arguments)
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && !environment_flag("CI")
        && !environment_flag("DCC_CUA_NO_UPDATE_CHECK")
}

pub(crate) fn command_allows_check<T: AsRef<str>>(arguments: &[T]) -> bool {
    let Some(command) = arguments.first().map(AsRef::as_ref) else {
        return false;
    };
    !matches!(
        command,
        "-h" | "--help"
            | "help"
            | "-V"
            | "--version"
            | "version"
            | "update"
            | "browser-extension"
            | "__private-worker"
            | "host"
            | "host-call"
            | "host-batch"
            | "host-jsonl"
            | "host-ensure"
    )
}

fn environment_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|value| {
        !value.is_empty()
            && !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
    })
}

async fn check_for_update() -> Option<String> {
    let path = cache_path()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let mut cache = load_cache(&path);
    if now >= cache.next_check_unix_secs {
        match tokio::time::timeout(HTTP_TIMEOUT, fetch_latest_complete_version()).await {
            Ok(Ok(latest)) => {
                cache.latest_version = Some(latest);
                cache.next_check_unix_secs = now.saturating_add(CACHE_TTL.as_secs());
            }
            Ok(Err(_)) | Err(_) => {
                cache.next_check_unix_secs = now.saturating_add(FAILURE_RETRY.as_secs());
            }
        }
        let _ = save_cache(&path, &cache);
    }
    cache
        .latest_version
        .as_deref()
        .and_then(|latest| notification(env!("CARGO_PKG_VERSION"), latest))
}

async fn fetch_latest_complete_version() -> Result<String, Box<dyn std::error::Error + Send + Sync>>
{
    let client = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
    let url = "https://api.github.com/repos/dcc-mcp/dcc-cua/releases?per_page=20";
    let mut request = client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            concat!("dcc-cua/", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28");
    if let Some(token) = super::update::github_token() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RELEASE_RESPONSE_BYTES as u64)
    {
        return Err("GitHub releases response exceeds the update-check limit".into());
    }
    let body = response.bytes().await?;
    if body.len() > MAX_RELEASE_RESPONSE_BYTES {
        return Err("GitHub releases response exceeds the update-check limit".into());
    }
    let releases = super::update::parse_github_releases(std::str::from_utf8(&body)?)?;
    let target = self_update::get_target();
    super::update::latest_release_assets(&releases, target)
        .map(|(release, _, _)| release.version.clone())
        .ok_or_else(|| format!("no complete {target} release bundle found").into())
}

fn cache_path() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"))
    } else {
        env::var_os("HOME")
    }?;
    Some(
        PathBuf::from(home)
            .join(".dcc-cua")
            .join("cache")
            .join("update-check.json"),
    )
}

fn load_cache(path: &Path) -> UpdateCheckCache {
    let Ok(bytes) = fs::read(path) else {
        return UpdateCheckCache::default();
    };
    if bytes.len() > 16 * 1024 {
        return UpdateCheckCache::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_cache(path: &Path, cache: &UpdateCheckCache) -> Result<(), std::io::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("update cache requires a parent directory"))?;
    fs::create_dir_all(parent)?;
    crate::write_json_atomically(path, &serde_json::to_vec_pretty(cache)?)
}

pub(crate) fn notification(current: &str, latest: &str) -> Option<String> {
    self_update::version::bump_is_greater(current, latest)
        .ok()?
        .then(|| {
            format!(
                "A new version of dcc-cua is available: {current} -> {latest}\n\
                 Run 'dcc-cua update' to install it."
            )
        })
}
