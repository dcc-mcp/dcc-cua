use crate::update::{
    install_single_with, latest_release_assets, lock_installation, release_archive_name,
    should_install_release, update_entry_name,
};
use rstest::rstest;
use self_update::update::{Release, ReleaseAsset};
use std::path::{Path, PathBuf};
use std::{fs, io};

fn update_fixture(root: &Path) -> (PathBuf, PathBuf) {
    let name = format!("dcc-cua{}", std::env::consts::EXE_SUFFIX);
    let executable = root.join(&name);
    let extracted = root.join("extracted");
    fs::create_dir(&extracted).unwrap();
    fs::write(&executable, b"old current").unwrap();
    fs::write(extracted.join(name), b"new current").unwrap();
    (executable, extracted)
}

#[rstest]
fn update_installs_single_entry_without_touching_historical_sibling() {
    let root = tempfile::tempdir().unwrap();
    let (executable, extracted) = update_fixture(root.path());
    let historical = root.path().join(format!(
        "dcc-cua-background{}",
        std::env::consts::EXE_SUFFIX
    ));
    fs::write(&historical, b"unowned historical bytes").unwrap();
    install_single_with(&executable, &extracted, |replacement| {
        fs::rename(replacement, &executable)
    })
    .unwrap();
    assert_eq!(fs::read(executable).unwrap(), b"new current");
    assert_eq!(fs::read(historical).unwrap(), b"unowned historical bytes");
}

#[rstest]
#[case(false)]
#[case(true)]
fn invalid_archive_entry_does_not_change_installation(#[case] empty: bool) {
    let root = tempfile::tempdir().unwrap();
    let (executable, extracted) = update_fixture(root.path());
    let replacement = extracted.join(executable.file_name().unwrap());
    if empty {
        fs::write(replacement, b"").unwrap();
    } else {
        fs::remove_file(replacement).unwrap();
    }
    assert!(
        install_single_with(&executable, &extracted, |_| panic!("must validate first")).is_err()
    );
    assert_eq!(fs::read(executable).unwrap(), b"old current");
}

#[rstest]
#[case(false)]
#[case(true)]
fn failed_self_replacement_restores_missing_current(#[case] moved: bool) {
    let root = tempfile::tempdir().unwrap();
    let (executable, extracted) = update_fixture(root.path());
    let error = install_single_with(&executable, &extracted, |_| {
        if moved {
            fs::remove_file(&executable)?;
        }
        Err(io::Error::other("injected self-replacement refusal"))
    })
    .unwrap_err();
    assert!(error.to_string().contains("injected"));
    assert_eq!(fs::read(executable).unwrap(), b"old current");
}

#[rstest]
fn renamed_entry_is_refused_instead_of_guessing_which_binary_to_replace() {
    assert!(update_entry_name(Path::new("custom-cua")).is_err());
    assert!(update_entry_name(Path::new("dcc-cua-background.exe")).is_err());
}

#[rstest]
#[case("1.0.0", "1.1.0", true)]
#[case("1.1.0", "1.1.0", true)]
#[case("1.2.0", "1.1.0", false)]
fn same_version_repairs_executable_without_downgrade(
    #[case] current: &str,
    #[case] latest: &str,
    #[case] install: bool,
) {
    assert_eq!(should_install_release(current, latest).unwrap(), install);
}

#[rstest]
fn concurrent_updates_refuse_before_self_replacement() {
    let root = tempfile::tempdir().unwrap();
    let (executable, extracted) = update_fixture(root.path());
    let first = lock_installation(root.path()).unwrap();
    assert!(install_single_with(&executable, &extracted, |_| panic!("must refuse")).is_err());
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    drop(first);
    assert!(lock_installation(root.path()).is_ok());
}

#[rstest]
fn incomplete_rollback_retains_recovery_bytes() {
    let root = tempfile::tempdir().unwrap();
    let (executable, extracted) = update_fixture(root.path());
    let error = install_single_with(&executable, &extracted, |_| {
        fs::remove_file(&executable)?;
        fs::create_dir(&executable)?;
        Err(io::Error::other("injected rollback obstruction"))
    })
    .unwrap_err();
    assert!(error.to_string().contains("rollback was incomplete"));
    assert!(fs::read_dir(root.path()).unwrap().any(|entry| {
        fs::read(entry.unwrap().path().join("current"))
            .ok()
            .as_deref()
            == Some(b"old current")
    }));
}

#[cfg(unix)]
#[rstest]
fn symlinked_replacement_is_refused_without_touching_its_target() {
    let root = tempfile::tempdir().unwrap();
    let (executable, extracted) = update_fixture(root.path());
    let replacement = extracted.join(executable.file_name().unwrap());
    fs::remove_file(&replacement).unwrap();
    let unrelated = root.path().join("unrelated");
    fs::write(&unrelated, b"unrelated bytes").unwrap();
    std::os::unix::fs::symlink(&unrelated, replacement).unwrap();
    assert!(install_single_with(&executable, &extracted, |_| panic!("must refuse")).is_err());
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert_eq!(fs::read(unrelated).unwrap(), b"unrelated bytes");
}

#[rstest]
fn github_api_asset_urls_are_normalized_to_official_downloads() {
    let target = "x86_64-pc-windows-msvc";
    let archive = release_archive_name("1.3.2", target);
    let releases = [Release {
        version: "1.3.2".into(),
        assets: vec![
            ReleaseAsset {
                name: archive.clone(),
                download_url:
                    "https://api.github.com/repos/dcc-mcp/dcc-cua/releases/assets/522082632".into(),
            },
            ReleaseAsset {
                name: format!("{archive}.sha256"),
                download_url:
                    "https://api.github.com/repos/dcc-mcp/dcc-cua/releases/assets/522082629".into(),
            },
        ],
        ..Default::default()
    }];

    let (_, selected, checksum) = latest_release_assets(&releases, target).unwrap();
    assert_eq!(
        selected.download_url,
        format!("https://github.com/dcc-mcp/dcc-cua/releases/download/v1.3.2/{archive}")
    );
    assert_eq!(
        checksum.download_url,
        format!("https://github.com/dcc-mcp/dcc-cua/releases/download/v1.3.2/{archive}.sha256")
    );
}
