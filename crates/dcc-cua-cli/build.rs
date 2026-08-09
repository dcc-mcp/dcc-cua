fn main() {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => configure_windows_linker(),
        Ok("macos") => configure_macos_loader(),
        _ => {}
    }
}

fn configure_windows_linker() {
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
