use dcc_mcp_cua_host::{
    HOST_HELLO_TIMEOUT_MS, HOST_PROTOCOL_VERSION, HostTransport, MAX_APPLICATION_LABEL_CHARS,
    MAX_BINARY_FRAME_BYTES, MAX_HOST_CONNECTIONS, MAX_JSON_FRAME_BYTES,
    MAX_PARALLEL_DISCOVERY_REQUESTS, MAX_TASK_GRANT_ID_CHARS, host_capabilities,
};
use serde_json::{Value, json};

pub(crate) fn document() -> Value {
    json!({
        "schema_version": 1,
        "name": "dcc-mcp-cua",
        "version": env!("CARGO_PKG_VERSION"),
        "rust_version": env!("CARGO_PKG_RUST_VERSION"),
        "target": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "host": {
            "stdio_command": ["host", "--stdio"],
            "endpoint_command": ["host"],
            "ensure_command": ["host-ensure"],
            "default_endpoint": HostTransport::default_endpoint(),
            "protocol_version": HOST_PROTOCOL_VERSION,
            "snapshot_transports": ["binary_frame", "shared_memory"],
            "max_json_frame_bytes": MAX_JSON_FRAME_BYTES,
            "max_binary_frame_bytes": MAX_BINARY_FRAME_BYTES,
            "max_connections": MAX_HOST_CONNECTIONS,
            "hello_timeout_ms": HOST_HELLO_TIMEOUT_MS,
            "max_parallel_discovery_requests": MAX_PARALLEL_DISCOVERY_REQUESTS,
            "grant_limits": {
                "task_grant_id_max_chars": MAX_TASK_GRANT_ID_CHARS,
                "application_label_max_chars": MAX_APPLICATION_LABEL_CHARS,
            },
            "capabilities": host_capabilities(cfg!(any(windows, target_os = "linux", target_os = "macos"))),
        },
        "core_bridge": {
            "command": ["host-jsonl"],
            "preferred_snapshot_transport": "shared_memory",
            "rust_crate": "dcc-mcp-cua-client",
        },
        "upstream_driver": {
            "command": ["cua-driver", "<COMMAND>"],
            "binary_environment": "CUA_DRIVER_BIN",
            "bundled_binary": crate::update::bundled_driver_relative_path(env!("CARGO_PKG_VERSION")),
            "development_sibling_fallback": format!("cua-driver{}", std::env::consts::EXE_SUFFIX),
            "top_level_aliases": {
                "daemon": "serve",
                "mcp": "mcp",
                "recording": "recording",
            },
        },
    })
}
