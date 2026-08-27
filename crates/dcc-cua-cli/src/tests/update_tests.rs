use rstest::rstest;
use self_update::update::{Release, ReleaseAsset};

use crate::update::{latest_release_assets, release_archive_name};

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
