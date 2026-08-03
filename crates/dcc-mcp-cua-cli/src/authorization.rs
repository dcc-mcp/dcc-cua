use std::sync::Arc;

use async_trait::async_trait;
use dcc_mcp_cua_core::{
    ComputerUseDriver, ConfiguredDriverOptions, DriverAuthorizationAction,
    DriverAuthorizationDecision, DriverAuthorizationHost, DriverAuthorizationHostError,
    DriverAuthorizationRequest, RuntimeAuthorizationOptions, SessionPermissionMode,
};

const EXISTING_PROFILE_GRANT: &str = "existing-profile";

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
                unrestricted_acknowledged: false,
                max_session_ttl_seconds: 8 * 60 * 60,
                max_idle_ttl_seconds: 30 * 60,
            },
        },
        Arc::new(ExistingProfileAuthorizationHost),
    )?)
}

pub(crate) fn existing_profile_grant_requested(flags: &[String]) -> Result<bool, String> {
    let mut requested = false;
    let mut index = 0;
    while index < flags.len() {
        let value = if flags[index] == "--grant" {
            index += 1;
            Some(
                flags
                    .get(index)
                    .ok_or_else(|| "--grant requires a value".to_owned())?
                    .as_str(),
            )
        } else {
            flags[index].strip_prefix("--grant=")
        };
        if let Some(value) = value {
            if !matches!(value, "existing-profile" | "existing_profile") {
                return Err(format!(
                    "unsupported Host grant {value:?}; supported: {EXISTING_PROFILE_GRANT}"
                ));
            }
            requested = true;
        }
        index += 1;
    }
    Ok(requested)
}

struct ExistingProfileAuthorizationHost;

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
