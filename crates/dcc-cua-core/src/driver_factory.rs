use std::sync::Arc;

use cua_driver_sdk::{
    CuaDriver, DriverAuthorizationHost, DriverError, DriverHostOptions, PrivateWorkerOptions,
};
use cursor_overlay::CursorConfig;

use crate::contracts::{
    ComputerUseError, ComputerUseErrorCode, ComputerUseResult, ConfiguredDriverOptions,
    MOUSE_CURSOR_THEME,
};
use crate::platform_process::prepare_platform_process;

pub(crate) const UPSTREAM_CURSOR_RENDERER_ENABLED: bool = cfg!(any(windows, target_os = "linux"));
pub(crate) const BUNDLED_CURSOR_THEME: &[u8] =
    include_bytes!("../../../assets/cursor-theme/dcc-cua.cua-theme");

pub(crate) fn ensure_bundled_cursor_theme() -> ComputerUseResult<()> {
    let theme = cursor_overlay::decode_theme(BUNDLED_CURSOR_THEME).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("decode bundled cursor theme: {error}"),
        )
    })?;
    if theme.id != MOUSE_CURSOR_THEME {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            "bundled cursor theme does not match the runtime contract",
        ));
    }

    let root = cursor_overlay::theme_store_root().map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("resolve CUA cursor theme store: {error}"),
        )
    })?;
    std::fs::create_dir_all(&root).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("create CUA cursor theme store {}: {error}", root.display()),
        )
    })?;
    let path = root.join(format!("{MOUSE_CURSOR_THEME}.cua-theme"));
    if std::fs::read(&path).is_ok_and(|installed| installed == BUNDLED_CURSOR_THEME) {
        return Ok(());
    }
    std::fs::write(&path, BUNDLED_CURSOR_THEME).map_err(|error| {
        ComputerUseError::new(
            ComputerUseErrorCode::BackendUnavailable,
            format!("install bundled cursor theme {}: {error}", path.display()),
        )
    })
}

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
    // Upstream's native renderer is a process-lifetime singleton. Its opaque
    // title token lets our presenter adopt only this configured local instance.
    #[cfg(windows)]
    static CURSOR_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    #[cfg(windows)]
    let cursor_id = CURSOR_ID.get_or_init(|| {
        dcc_cua_indicator::register_cursor_renderer_id(format!("dcc-cua-{}", uuid::Uuid::new_v4()))
    });
    DriverHostOptions {
        cursor: CursorConfig {
            // Embedded Windows and Linux use CUA's native theme renderer;
            // packaged macOS uses the same renderer through a private worker.
            enabled: UPSTREAM_CURSOR_RENDERER_ENABLED,
            theme_id: MOUSE_CURSOR_THEME.into(),
            #[cfg(windows)]
            cursor_id: cursor_id.clone(),
            ..CursorConfig::default()
        },
        host_owns_permission_ux: true,
        host_bundle_id: None,
        claude_code_compatibility: false,
        prepare_desktop_environment: true,
        register_host_tools: None,
        authorization_host: None,
        activity_observer: None,
    }
}
