use std::sync::Arc;

use async_trait::async_trait;
use dcc_cua_host::{HostSecretVault, HostSecretVaultError, SecretValue};
use dcc_cua_protocol::validate_secret_handle;

const KEYRING_SERVICE: &str = "dcc-cua";

pub(crate) fn native_secret_vault() -> Arc<dyn HostSecretVault> {
    Arc::new(KeyringSecretVault)
}

struct KeyringSecretVault;

fn map_keyring_error(error: keyring::Error) -> HostSecretVaultError {
    if matches!(error, keyring::Error::NoEntry) {
        HostSecretVaultError::NotFound
    } else {
        HostSecretVaultError::Unavailable
    }
}

#[async_trait]
impl HostSecretVault for KeyringSecretVault {
    async fn resolve(&self, handle: &str) -> Result<SecretValue, HostSecretVaultError> {
        validate_secret_handle(handle).map_err(|_| HostSecretVaultError::InvalidHandle)?;
        let handle = handle.to_owned();
        let value = tokio::task::spawn_blocking(move || {
            keyring::Entry::new(KEYRING_SERVICE, &handle)
                .and_then(|entry| entry.get_password())
                .map_err(map_keyring_error)
        })
        .await
        .map_err(|_| HostSecretVaultError::Unavailable)??;
        SecretValue::new(value)
    }

    async fn store(&self, handle: &str, value: SecretValue) -> Result<(), HostSecretVaultError> {
        validate_secret_handle(handle).map_err(|_| HostSecretVaultError::InvalidHandle)?;
        let handle = handle.to_owned();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(KEYRING_SERVICE, &handle).map_err(map_keyring_error)?;
            entry
                .set_password(value.expose())
                .map_err(map_keyring_error)
        })
        .await
        .map_err(|_| HostSecretVaultError::Unavailable)??;
        Ok(())
    }
}
