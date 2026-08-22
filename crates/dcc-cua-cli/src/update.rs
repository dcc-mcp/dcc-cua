use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use self_update::update::{Release, ReleaseAsset};
use sha2::{Digest, Sha256};

use crate::cli_args::has_flag;

const OWNER: &str = "dcc-mcp";
const REPOSITORY: &str = "dcc-cua";
const BINARY: &str = "dcc-cua";
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const RELEASE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

type UpdateResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn run(flags: &[String]) -> UpdateResult<()> {
    let current = env!("CARGO_PKG_VERSION");
    let target = self_update::get_target();
    let releases = fetch_releases()?;
    let (release, asset, checksum) = latest_release_assets(&releases, target)
        .ok_or_else(|| format!("no complete {target} release bundle found"))?;

    if has_flag(flags, "--check") {
        println!(
            "{}",
            serde_json::json!({
                "current": current,
                "latest": release.version,
                "asset": asset.name,
            })
        );
        return Ok(());
    }
    if !self_update::version::bump_is_greater(current, &release.version)? {
        println!("dcc-cua is already up to date at v{current}");
        return Ok(());
    }

    confirm_update(current, &release.version)?;
    install_release(&std::env::current_exe()?, asset, checksum)?;
    println!("dcc-cua updated to v{}", release.version);
    Ok(())
}

/// Query GitHub for the available releases.
///
/// The primary source is the GitHub REST API via `self_update`, which returns
/// the full release list and lets the updater pick the newest release that
/// ships a complete bundle for this platform. The API allows only 60
/// unauthenticated requests per hour per IP address, and shared or proxied
/// networks routinely exhaust that quota (GitHub answers HTTP 403), so release
/// discovery falls back to the rate-limit-free public web endpoint:
/// `GET https://github.com/{owner}/{repo}/releases/latest` redirects to the
/// latest release tag page. Asset names follow a deterministic layout, so the
/// exact official download URLs can be rebuilt from the tag alone.
fn fetch_releases() -> UpdateResult<Vec<Release>> {
    let mut builder = self_update::backends::github::ReleaseList::configure();
    builder.repo_owner(OWNER).repo_name(REPOSITORY);
    if let Some(token) = github_token() {
        builder.auth_token(&token);
    }
    let api_result = builder.build()?.fetch();
    match api_result {
        Ok(releases) => Ok(releases),
        Err(api_error) => match latest_release_fallback() {
            Ok(release) => Ok(vec![release]),
            Err(fallback_error) => Err(format!(
                "GitHub release query failed: {api_error}; public release-page \
                 fallback failed: {fallback_error}. The GitHub REST API allows \
                 60 unauthenticated requests per hour per IP address; set \
                 GITHUB_TOKEN (or GH_TOKEN) to raise that limit, or retry later."
            )
            .into()),
        },
    }
}

fn github_token() -> Option<String> {
    env::var("GITHUB_TOKEN")
        .or_else(|_| env::var("GH_TOKEN"))
        .ok()
        .filter(|token| !token.is_empty())
}

/// Discover the newest release through the public web redirect instead of the
/// rate-limited REST API.
fn latest_release_fallback() -> UpdateResult<Release> {
    let location = latest_release_location()?;
    let version = version_from_release_location(&location)
        .ok_or_else(|| format!("unexpected latest-release redirect target: {location}"))?;
    Ok(fallback_release(&version, self_update::get_target()))
}

/// Request `releases/latest` without following redirects and return its
/// `Location` header, e.g.
/// `https://github.com/dcc-mcp/dcc-cua/releases/tag/v1.3.0`.
fn latest_release_location() -> UpdateResult<String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "release discovery requires a running tokio runtime")?;
    handle.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(RELEASE_DISCOVERY_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let latest_url = format!("https://github.com/{OWNER}/{REPOSITORY}/releases/latest");
        let response = client.get(latest_url).send().await?;
        if !matches!(
            response.status(),
            reqwest::StatusCode::MOVED_PERMANENTLY
                | reqwest::StatusCode::FOUND
                | reqwest::StatusCode::SEE_OTHER
                | reqwest::StatusCode::TEMPORARY_REDIRECT
                | reqwest::StatusCode::PERMANENT_REDIRECT
        ) {
            return Err(format!(
                "latest-release query returned HTTP {}",
                response.status().as_u16()
            )
            .into());
        }
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| "latest-release query returned no Location header".into())
    })
}

/// Synthesize the release entry the updater expects from a validated version.
/// Asset names follow the deterministic release layout and the URLs are the
/// exact official `releases/download` locations the updater requires.
pub(crate) fn fallback_release(version: &str, target: &str) -> Release {
    let archive = release_archive_name(version, target);
    let checksum = format!("{archive}.sha256");
    Release {
        name: format!("v{version}"),
        version: version.to_owned(),
        date: String::new(),
        body: None,
        assets: vec![
            ReleaseAsset {
                name: archive.clone(),
                download_url: official_asset_url(version, &archive),
            },
            ReleaseAsset {
                name: checksum.clone(),
                download_url: official_asset_url(version, &checksum),
            },
        ],
    }
}

/// Extract the release version from the exact official `releases/latest`
/// redirect target, rejecting anything that is not
/// `https://github.com/{OWNER}/{REPOSITORY}/releases/tag/v{VERSION}`.
pub(crate) fn version_from_release_location(location: &str) -> Option<String> {
    let (scheme, rest) = location.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let (host, path) = rest.split_once('/')?;
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let expected = format!("{OWNER}/{REPOSITORY}/releases/tag/v");
    let version = path.strip_prefix(&expected)?;
    is_plausible_version(version).then_some(version.to_owned())
}

fn is_plausible_version(version: &str) -> bool {
    !version.is_empty()
        && !version.starts_with('.')
        && !version.ends_with('.')
        && version.chars().any(|character| character.is_ascii_digit())
        && version.chars().all(|character| {
            character.is_ascii_digit()
                || character.is_ascii_alphabetic()
                || matches!(character, '.' | '-' | '+')
        })
}

pub(crate) fn release_archive_name(version: &str, target: &str) -> String {
    let extension = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("{BINARY}-{version}-{target}.{extension}")
}

pub(crate) fn latest_release_assets<'a>(
    releases: &'a [Release],
    target: &str,
) -> Option<(&'a Release, &'a ReleaseAsset, &'a ReleaseAsset)> {
    releases.iter().find_map(|release| {
        let expected = release_archive_name(&release.version, target);
        let archive = release.assets.iter().find(|asset| {
            asset.name == expected
                && asset.download_url == official_asset_url(&release.version, &expected)
        })?;
        let checksum_name = format!("{expected}.sha256");
        let checksum = release.assets.iter().find(|asset| {
            asset.name == checksum_name
                && asset.download_url == official_asset_url(&release.version, &checksum_name)
        })?;
        Some((release, archive, checksum))
    })
}

fn official_asset_url(version: &str, name: &str) -> String {
    format!("https://github.com/{OWNER}/{REPOSITORY}/releases/download/v{version}/{name}")
}

fn install_release(
    executable: &Path,
    asset: &ReleaseAsset,
    checksum_asset: &ReleaseAsset,
) -> UpdateResult<()> {
    let install_root = executable
        .parent()
        .ok_or("current executable has no installation directory")?;
    let transaction = self_update::TempDir::new_in(install_root)?;
    let checksum_path = transaction.path().join(&checksum_asset.name);
    let mut checksum_file = File::create(&checksum_path)?;
    download_asset(checksum_asset, &mut checksum_file).map_err(|error| {
        format!(
            "{} release checksum download failed: {error}",
            checksum_asset.name
        )
    })?;
    checksum_file.sync_all()?;
    let archive_path = transaction.path().join(&asset.name);
    let mut archive = File::create(&archive_path)?;
    let mut download = self_update::Download::from_url(&asset.download_url);
    download
        .show_progress(true)
        .set_header(
            "accept".parse().expect("valid static header name"),
            "application/octet-stream"
                .parse()
                .expect("valid static header value"),
        )
        .download_to(&mut archive)
        .map_err(|error| format!("{} release archive download failed: {error}", asset.name))?;
    archive.sync_all()?;
    if archive.metadata()?.len() > MAX_ARCHIVE_BYTES {
        return Err("release archive exceeds the 256 MiB update limit".into());
    }
    verify_sha256(
        &archive_path,
        &fs::read_to_string(&checksum_path)?,
        &asset.name,
    )?;

    let extracted = transaction.path().join("extracted");
    fs::create_dir(&extracted)?;
    let binary_name = format!("{BINARY}{}", std::env::consts::EXE_SUFFIX);
    self_update::Extract::from_source(&archive_path).extract_file(&extracted, &binary_name)?;

    let new_executable = extracted.join(binary_name);
    require_file(&new_executable)?;
    let executable_metadata = fs::symlink_metadata(&new_executable)?;
    if executable_metadata.file_type().is_symlink()
        || !executable_metadata.is_file()
        || executable_metadata.len() > MAX_BINARY_BYTES
    {
        return Err("release executable is not a bounded regular file".into());
    }
    self_update::self_replace::self_replace(new_executable)?;
    Ok(())
}

fn download_asset(asset: &ReleaseAsset, destination: &mut File) -> UpdateResult<()> {
    let mut download = self_update::Download::from_url(&asset.download_url);
    download
        .show_progress(true)
        .set_header(
            "accept".parse().expect("valid static header name"),
            "application/octet-stream"
                .parse()
                .expect("valid static header value"),
        )
        .download_to(destination)?;
    Ok(())
}

pub(crate) fn verify_sha256(path: &Path, sidecar: &str, expected_name: &str) -> UpdateResult<()> {
    let mut fields = sidecar.split_whitespace();
    let expected = fields.next().ok_or("checksum sidecar is empty")?;
    let name = fields
        .next()
        .ok_or("checksum sidecar must include the exact archive name")?
        .trim_start_matches('*');
    if fields.next().is_some() || name != expected_name || expected.len() != 64 {
        return Err("checksum sidecar does not name the exact archive".into());
    }
    let bytes = fs::read(path)?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err("release archive SHA-256 mismatch".into());
    }
    Ok(())
}

fn require_file(path: &Path) -> UpdateResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("release bundle is missing {}", path.display()).into())
    }
}

fn confirm_update(current: &str, latest: &str) -> UpdateResult<()> {
    print!("Update dcc-cua v{current} to v{latest}? [Y/n] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ) {
        Ok(())
    } else {
        Err("update aborted".into())
    }
}
