fn main() {
    configure_build_identity();
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => configure_windows_linker(),
        Ok("macos") => configure_macos_loader(),
        _ => {}
    }
}

fn configure_build_identity() {
    use std::process::Command;
    // Capture the compiled source identity. Reading the executable currently at
    // its old installation path cannot identify an already-running bridge.
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|value| value.trim().to_owned())
    };
    if let Some(revision) = git(&["rev-parse", "HEAD"])
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        println!("cargo:rustc-env=CUA_BUILD_SOURCE_REVISION={revision}");
        if let Some(status) = git(&["status", "--porcelain", "--untracked-files=normal"]) {
            println!(
                "cargo:rustc-env=CUA_BUILD_SOURCE_DIRTY={}",
                !status.is_empty()
            );
        }
    }
    for key in ["PROFILE", "TARGET"] {
        if let Ok(value) = std::env::var(key) {
            println!("cargo:rustc-env=CUA_BUILD_{key}={value}");
        }
    }
    for name in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &branch])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=../../crates");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=build.rs");
}

fn configure_windows_linker() {
    println!("cargo:rerun-if-changed=windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"),
        )
        .join("windows.manifest");
        // Loader policy applies before main; runtime FreeConsole cannot prevent a flash.
        // Older Windows ignores this policy and needs CREATE_NO_WINDOW at the launcher.
        println!("cargo:rustc-link-arg-bin=dcc-cua=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=dcc-cua=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
    let argument = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => "/STACK:8388608",
        Ok("gnu") => "-Wl,--stack,8388608",
        _ => return,
    };
    println!("cargo:rustc-link-arg-bin=dcc-cua={argument}");
}

fn configure_macos_loader() {
    // ScreenCaptureKit's Swift bridge resolves libswift_Concurrency through
    // @rpath. Dependency build scripts cannot add LC_RPATH entries to this
    // final executable, and a private worker intentionally starts from a
    // scrubbed environment. Give both the Host and its worker deterministic
    // system and colocated-runtime lookup paths instead of relying on DYLD_*.
    for runtime_path in ["/usr/lib/swift", "@executable_path"] {
        println!("cargo:rustc-link-arg-bin=dcc-cua=-Wl,-rpath,{runtime_path}");
    }
}
