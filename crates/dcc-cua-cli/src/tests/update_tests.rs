use rstest::rstest;
use self_update::update::{Release, ReleaseAsset};

use crate::update::{
    install_pair_with, latest_release_assets, lock_installation, release_archive_name,
    should_install_release, update_entry_names,
};

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn binary_name(background: bool) -> String {
    format!(
        "dcc-cua{}{}",
        if background { "-background" } else { "" },
        std::env::consts::EXE_SUFFIX
    )
}

fn update_fixture(
    root: &Path,
    background: bool,
    companion_exists: bool,
) -> (PathBuf, PathBuf, PathBuf) {
    let executable = root.join(binary_name(background));
    let companion = root.join(binary_name(!background));
    let extracted = root.join("extracted");
    fs::create_dir(&extracted).unwrap();
    fs::write(&executable, b"old current").unwrap();
    if companion_exists {
        fs::write(&companion, b"old companion").unwrap();
    }
    fs::write(extracted.join(binary_name(false)), b"new terminal").unwrap();
    fs::write(extracted.join(binary_name(true)), b"new background").unwrap();
    (executable, companion, extracted)
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
fn update_installs_both_entries_without_swapping_their_identity(
    #[case] background: bool,
    #[case] companion_exists: bool,
) {
    let root = tempfile::tempdir().unwrap();
    let (executable, _, extracted) = update_fixture(root.path(), background, companion_exists);
    install_pair_with(&executable, &extracted, |replacement| {
        fs::rename(replacement, &executable)
    })
    .unwrap();
    assert_eq!(
        fs::read(root.path().join(binary_name(false))).unwrap(),
        b"new terminal"
    );
    assert_eq!(
        fs::read(root.path().join(binary_name(true))).unwrap(),
        b"new background"
    );
}

#[rstest]
#[case(false)]
#[case(true)]
fn missing_archive_companion_does_not_change_either_installed_entry(#[case] background: bool) {
    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) = update_fixture(root.path(), background, true);
    fs::remove_file(extracted.join(binary_name(!background))).unwrap();
    assert!(
        install_pair_with(&executable, &extracted, |_| {
            panic!("must validate the complete pair before self-replacement")
        })
        .is_err()
    );
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert_eq!(fs::read(companion).unwrap(), b"old companion");
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
fn failed_self_replacement_restores_previous_companion_state(
    #[case] background: bool,
    #[case] companion_exists: bool,
) {
    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) =
        update_fixture(root.path(), background, companion_exists);
    let error = install_pair_with(&executable, &extracted, |_| {
        Err(io::Error::other("injected self-replacement refusal"))
    })
    .unwrap_err();
    assert!(error.to_string().contains("injected"));
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    if companion_exists {
        assert_eq!(fs::read(companion).unwrap(), b"old companion");
    } else {
        assert!(!companion.exists());
    }
}

#[rstest]
fn failed_self_replacement_that_moved_current_away_restores_both_paths() {
    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) = update_fixture(root.path(), false, true);
    assert!(
        install_pair_with(&executable, &extracted, |_| {
            fs::remove_file(&executable)?;
            Err(io::Error::other("injected post-rename failure"))
        })
        .is_err()
    );
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert_eq!(fs::read(companion).unwrap(), b"old companion");
}

#[rstest]
fn renamed_entry_is_refused_instead_of_guessing_which_binary_to_replace() {
    assert!(update_entry_names(Path::new("custom-cua")).is_err());
}

#[rstest]
#[case("1.0.0", "1.1.0", true)]
#[case("1.1.0", "1.1.0", true)]
#[case("1.2.0", "1.1.0", false)]
fn same_version_repairs_the_pair_but_a_newer_install_is_not_downgraded(
    #[case] current: &str,
    #[case] latest: &str,
    #[case] install: bool,
) {
    assert_eq!(should_install_release(current, latest).unwrap(), install);
}

#[rstest]
fn concurrent_updates_refuse_before_changing_either_entry() {
    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) = update_fixture(root.path(), false, true);
    let first_update = lock_installation(root.path()).unwrap();
    assert!(install_pair_with(&executable, &extracted, |_| panic!("must refuse")).is_err());
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert_eq!(fs::read(companion).unwrap(), b"old companion");
    drop(first_update);
    assert!(lock_installation(root.path()).is_ok());
}

#[rstest]
fn incomplete_rollback_preserves_recovery_bytes() {
    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) = update_fixture(root.path(), false, true);
    let error = install_pair_with(&executable, &extracted, |_| {
        fs::remove_file(&companion)?;
        fs::create_dir(&companion)?;
        Err(io::Error::other("injected rollback obstruction"))
    })
    .unwrap_err();
    assert!(error.to_string().contains("rollback was incomplete"));
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert!(fs::read_dir(root.path()).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        fs::read(path.join("companion")).ok().as_deref() == Some(b"old companion")
    }));
}

#[cfg(unix)]
#[rstest]
fn symlinked_companion_is_refused_without_touching_its_target() {
    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) = update_fixture(root.path(), false, false);
    let unrelated = root.path().join("unrelated");
    fs::write(&unrelated, b"unrelated bytes").unwrap();
    std::os::unix::fs::symlink(&unrelated, &companion).unwrap();
    assert!(install_pair_with(&executable, &extracted, |_| panic!("must refuse")).is_err());
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert_eq!(fs::read(unrelated).unwrap(), b"unrelated bytes");
}

#[cfg(windows)]
#[rstest]
fn locked_windows_companion_refuses_before_self_replacement() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = tempfile::tempdir().unwrap();
    let (executable, companion, extracted) = update_fixture(root.path(), false, true);
    // Allow backup reads but deny delete/replace, as a resident image can do.
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&companion)
        .unwrap();
    assert!(install_pair_with(&executable, &extracted, |_| panic!("must refuse")).is_err());
    assert_eq!(fs::read(executable).unwrap(), b"old current");
    assert_eq!(fs::read(&companion).unwrap(), b"old companion");
    drop(locked);
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
