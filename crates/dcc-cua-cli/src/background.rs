#![cfg_attr(windows, windows_subsystem = "windows")]

// This is a separate PE entry point: hiding a console after main starts is too late.
// Inherited MCP pipes and the runtime's error/exit behavior remain unchanged.
fn main() {
    dcc_cua_cli::run();
}
