#[cfg(any(target_os = "macos", test))]
use std::path::Path;
#[cfg(not(target_os = "macos"))]
use std::sync::Arc;

use crate::cli_args::{flag_values, has_flag};
#[cfg(not(target_os = "macos"))]
use async_trait::async_trait;
#[cfg(any(target_os = "macos", test))]
use dcc_cua_core::PrivateWorkerOptions;
use dcc_cua_core::{
    ComputerUseDriver, ConfiguredDriverOptions, RuntimeAuthorizationOptions, SessionPermissionMode,
};
#[cfg(not(target_os = "macos"))]
use dcc_cua_core::{
    DriverAuthorizationAction, DriverAuthorizationDecision, DriverAuthorizationHost,
    DriverAuthorizationHostError, DriverAuthorizationRequest,
};

const EXISTING_PROFILE_GRANT: &str = "existing-profile";
#[cfg(any(target_os = "macos", test))]
const PRIVATE_WORKER_HOST_ID: &str = "com.dcc-cua.host";

#[cfg(not(target_os = "macos"))]
pub(crate) fn driver_for_host(
    flags: &[String],
) -> Result<ComputerUseDriver, Box<dyn std::error::Error>> {
    if !existing_profile_grant_requested(flags)? {
        return Ok(ComputerUseDriver::create()?);
    }
    Ok(ComputerUseDriver::create_with_authorization_host(
        ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![SessionPermissionMode::Standard],
                compatibility_mode: SessionPermissionMode::Standard,
                compatibility_bounded_manifest_path: None,
                compatibility_capability_manifest_path: None,
                unrestricted_acknowledged: false,
                max_session_ttl_seconds: 8 * 60 * 60,
                max_idle_ttl_seconds: 30 * 60,
            },
        },
        Arc::new(ExistingProfileAuthorizationHost),
    )?)
}

#[cfg(target_os = "macos")]
pub(crate) fn driver_for_private_worker(
    flags: &[String],
    binary_path: &Path,
) -> Result<ComputerUseDriver, Box<dyn std::error::Error>> {
    let existing_profile_granted = existing_profile_grant_requested(flags)?;
    Ok(ComputerUseDriver::create_private_worker(
        host_private_worker_options(binary_path, existing_profile_granted),
    )?)
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn host_private_worker_options(
    binary_path: &Path,
    existing_profile_granted: bool,
) -> PrivateWorkerOptions {
    let permission_mode = if existing_profile_granted {
        SessionPermissionMode::Unrestricted
    } else {
        SessionPermissionMode::Standard
    };
    PrivateWorkerOptions {
        binary_path: binary_path.to_string_lossy().into_owned(),
        host_bundle_id: PRIVATE_WORKER_HOST_ID.into(),
        startup_timeout_ms: None,
        shutdown_timeout_ms: None,
        // The private-worker protocol cannot carry a host callback. Only the
        // explicit existing-profile startup grant raises this internal ceiling;
        // the worker has no public endpoint and parent Host grants still gate it.
        configured_driver: ConfiguredDriverOptions {
            claude_code_compatibility: false,
            authorization: RuntimeAuthorizationOptions {
                allowed_modes: vec![permission_mode],
                compatibility_mode: permission_mode,
                compatibility_bounded_manifest_path: None,
                compatibility_capability_manifest_path: None,
                unrestricted_acknowledged: existing_profile_granted,
                max_session_ttl_seconds: 8 * 60 * 60,
                max_idle_ttl_seconds: 30 * 60,
            },
        },
        environment: Vec::new(),
        // The Host is a protocol process. Raw worker diagnostics can contain
        // runtime, filesystem, or user-controlled details, so they must never
        // escape through the Host's public stderr stream.
        inherit_stderr: false,
    }
}

pub(crate) fn existing_profile_grant_requested(flags: &[String]) -> Result<bool, String> {
    let values = flag_values(flags, "--grant");
    if has_flag(flags, "--grant") && values.is_empty() {
        return Err("--grant requires a value".to_owned());
    }
    for value in &values {
        if !matches!(value.as_str(), "existing-profile" | "existing_profile") {
            return Err(format!(
                "unsupported Host grant {value:?}; supported: {EXISTING_PROFILE_GRANT}"
            ));
        }
    }
    Ok(!values.is_empty())
}

#[cfg(not(target_os = "macos"))]
struct ExistingProfileAuthorizationHost;

#[cfg(not(target_os = "macos"))]
#[async_trait]
impl DriverAuthorizationHost for ExistingProfileAuthorizationHost {
    async fn authorize(
        &self,
        request: DriverAuthorizationRequest,
    ) -> Result<DriverAuthorizationDecision, DriverAuthorizationHostError> {
        let allowed = request.schema == "cua-driver-authorization-request-v1"
            && request.permission_mode == "standard"
            && request.adapter_id == "browser_prepare.existing_profile"
            && request.risk_class == "r2"
            && !request.public_session.is_empty();
        Ok(DriverAuthorizationDecision {
            action: if allowed {
                DriverAuthorizationAction::Allow
            } else {
                DriverAuthorizationAction::Deny
            },
            request_digest: request.request_digest,
        })
    }
}
