use rstest::rstest;
use self_update::update::{Release, ReleaseAsset};

use crate::update::{latest_release_assets, parse_github_releases, release_archive_name};

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

#[rstest]
fn published_release_is_ignored_until_its_platform_bundle_is_complete() {
    let target = "x86_64-pc-windows-msvc";
    let ready_archive = release_archive_name("1.3.3", target);
    let body = serde_json::json!([
        {
            "tag_name": "v1.4.0",
            "draft": false,
            "prerelease": false,
            "assets": []
        },
        {
            "tag_name": "v1.3.3",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": ready_archive,
                    "browser_download_url": format!(
                        "https://github.com/dcc-mcp/dcc-cua/releases/download/v1.3.3/{ready_archive}"
                    )
                },
                {
                    "name": format!("{ready_archive}.sha256"),
                    "browser_download_url": format!(
                        "https://github.com/dcc-mcp/dcc-cua/releases/download/v1.3.3/{ready_archive}.sha256"
                    )
                }
            ]
        }
    ]);

    let releases = parse_github_releases(&body.to_string()).unwrap();
    let (release, _, _) = latest_release_assets(&releases, target).unwrap();
    assert_eq!(release.version, "1.3.3");
}
