use dcc_mcp_cua_host::{
    HOST_CAPABILITIES, HOST_PROTOCOL_VERSION, HostTransport, MAX_BINARY_FRAME_BYTES,
    MAX_JSON_FRAME_BYTES,
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
            "default_endpoint": HostTransport::default_endpoint(),
            "protocol_version": HOST_PROTOCOL_VERSION,
            "snapshot_transports": ["binary_frame", "shared_memory"],
            "max_json_frame_bytes": MAX_JSON_FRAME_BYTES,
            "max_binary_frame_bytes": MAX_BINARY_FRAME_BYTES,
            "capabilities": HOST_CAPABILITIES,
        },
        "core_bridge": {
            "command": ["host-jsonl", "--snapshot-transport", "shared_memory"],
            "rust_crate": "dcc-mcp-cua-client",
        },
        "upstream_driver": {
            "command": ["cua-driver", "<COMMAND>"],
            "binary_environment": "CUA_DRIVER_BIN",
            "top_level_aliases": {
                "daemon": "serve",
                "mcp": "mcp",
                "recording": "recording",
            },
        },
    })
}
