use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use self_update::update::{Release, ReleaseAsset};

const OWNER: &str = "dcc-mcp";
const REPOSITORY: &str = "dcc-cua";
const BINARY: &str = "dcc-cua";

type UpdateResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn run(flags: &[String]) -> UpdateResult<()> {
    let current = env!("CARGO_PKG_VERSION");
    let target = self_update::get_target();
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(OWNER)
        .repo_name(REPOSITORY)
        .build()?
        .fetch()?;
    let (release, asset) = latest_release_asset(&releases, target)
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
    install_release(&std::env::current_exe()?, asset)?;
    println!("dcc-cua updated to v{}", release.version);
    Ok(())
}

pub(crate) fn release_archive_name(version: &str, target: &str) -> String {
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("{BINARY}-{version}-{target}.{extension}")
}

pub(crate) fn latest_release_asset<'a>(
    releases: &'a [Release],
    target: &str,
) -> Option<(&'a Release, &'a ReleaseAsset)> {
    releases.iter().find_map(|release| {
        let expected = release_archive_name(&release.version, target);
        release
            .assets
            .iter()
            .find(|asset| asset.name == expected)
            .map(|asset| (release, asset))
    })
}

fn install_release(executable: &Path, asset: &ReleaseAsset) -> UpdateResult<()> {
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

    let extracted = transaction.path().join("extracted");
    fs::create_dir(&extracted)?;
    self_update::Extract::from_source(&archive_path).extract_into(&extracted)?;

    let new_executable = extracted.join(format!("{BINARY}{}", std::env::consts::EXE_SUFFIX));
    require_file(&new_executable)?;
    self_update::self_replace::self_replace(new_executable)?;
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
