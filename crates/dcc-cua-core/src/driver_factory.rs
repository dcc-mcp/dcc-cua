use std::sync::Arc;

use cua_driver_sdk::{
    CuaDriver, DriverAuthorizationHost, DriverError, DriverHostOptions, PrivateWorkerOptions,
};
use cursor_overlay::CursorConfig;

use crate::contracts::ConfiguredDriverOptions;
use crate::platform_process::prepare_platform_process;

pub(crate) const UPSTREAM_CURSOR_RENDERER_ENABLED: bool = cfg!(target_os = "linux");

pub(crate) fn create_embedded() -> Result<(Arc<CuaDriver>, bool), DriverError> {
    prepare_platform_process();
    CuaDriver::try_create_for_host(driver_host_options())
        .map(|driver| (driver, UPSTREAM_CURSOR_RENDERER_ENABLED))
}

pub(crate) fn create_private_worker(
    options: PrivateWorkerOptions,
) -> Result<(Arc<CuaDriver>, bool), DriverError> {
    prepare_platform_process();
    CuaDriver::create_private_worker(options).map(|driver| (driver, true))
}

pub(crate) fn create_authorized(
    options: ConfiguredDriverOptions,
    host: Arc<dyn DriverAuthorizationHost>,
) -> Result<(Arc<CuaDriver>, bool), DriverError> {
    prepare_platform_process();
    let mut host_options = driver_host_options();
    host_options.authorization_host = Some(host);
    CuaDriver::try_create_configured_for_host(options, host_options)
        .map(|driver| (driver, UPSTREAM_CURSOR_RENDERER_ENABLED))
}

pub(crate) fn driver_host_options() -> DriverHostOptions {
    DriverHostOptions {
        cursor: CursorConfig {
            // Windows keeps the custom Host-owned pointer. The embedded CUA
            // runtime owns it on Linux; packaged macOS uses a private worker.
            enabled: UPSTREAM_CURSOR_RENDERER_ENABLED,
            ..CursorConfig::default()
        },
        host_owns_permission_ux: true,
        host_bundle_id: None,
        claude_code_compatibility: false,
        prepare_desktop_environment: true,
        // ADR 0002 (revised): no host tools are registered. Upstream risk
        // classification fails closed for unknown tool names, so DCC typed
        // execution lives in the DCC-MCP gateway instead of this registry.
        register_host_tools: None,
        authorization_host: None,
        activity_observer: None,
    }
}
