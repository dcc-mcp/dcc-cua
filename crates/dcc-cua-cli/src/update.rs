use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use self_update::update::{Release, ReleaseAsset};
use sha2::{Digest, Sha256};

const OWNER: &str = "dcc-mcp";
const REPOSITORY: &str = "dcc-cua";
const BINARY: &str = "dcc-cua";
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;

type UpdateResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn run(flags: &[String]) -> UpdateResult<()> {
    let current = env!("CARGO_PKG_VERSION");
    let target = self_update::get_target();
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(REPOSITORY)
        .build()?
        .fetch()?;
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
        .download_to(&mut archive)?;
    archive.sync_all()?;
    if archive.metadata()?.len() > MAX_ARCHIVE_BYTES {
        return Err("release archive exceeds the 256 MiB update limit".into());
    }
    let checksum_path = transaction.path().join(&checksum_asset.name);
    let mut checksum_file = File::create(&checksum_path)?;
    download_asset(checksum_asset, &mut checksum_file)?;
    checksum_file.sync_all()?;
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

fn has_flag(flags: &[String], name: &str) -> bool {
    flags.iter().any(|flag| flag == name)
}
