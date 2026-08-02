use std::error::Error;

const OWNER: &str = "dcc-mcp";
const REPOSITORY: &str = "dcc-mcp-computer-use";
const BINARY: &str = "dcc-mcp-cua";

pub fn run(flags: &[String]) -> Result<(), Box<dyn Error + Send + Sync>> {
    let target = self_update::get_target();
    if has_flag(flags, "--check") {
        let releases = self_update::backends::github::ReleaseList::configure()
            .repo_owner(OWNER)
            .repo_name(REPOSITORY)
            .build()?
            .fetch()?;
        let asset = releases
            .iter()
            .find_map(|release| {
                release
                    .asset_for(target, None)
                    .map(|asset| (release, asset))
            })
            .ok_or_else(|| format!("no {target} release asset found"))?;
        println!(
            "{{\"current\":\"{}\",\"latest\":\"{}\",\"asset\":\"{}\"}}",
            env!("CARGO_PKG_VERSION"),
            asset.0.version,
            asset.1.name
        );
        return Ok(());
    }

    let status = self_update::backends::github::Update::configure()
        .repo_owner(OWNER)
        .repo_name(REPOSITORY)
        .bin_name(BINARY)
        .show_download_progress(true)
        .current_version(env!("CARGO_PKG_VERSION"))
        .build()?
        .update()?;
    println!("dcc-mcp-cua updated to v{}", status.version());
    Ok(())
}

fn has_flag(flags: &[String], name: &str) -> bool {
    flags.iter().any(|flag| flag == name)
}
