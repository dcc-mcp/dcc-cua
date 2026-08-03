fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let argument = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => "/STACK:8388608",
        Ok("gnu") => "-Wl,--stack,8388608",
        _ => return,
    };
    println!("cargo:rustc-link-arg-bin=dcc-mcp-cua={argument}");
}
