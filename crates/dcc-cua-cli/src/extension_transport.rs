//! Discovery records are hints, never task authority. Verify the OS peer and full birth.
use dcc_cua_client::HostClient;
use dcc_cua_core::ComputerUseDriver;
use dcc_cua_host::HostSecurityServices;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::connection_diagnostics::{ConnectionRecord, ConnectionState, list_connections};

pub(crate) fn endpoint(connection_id: &str) -> Option<String> {
    let id = Uuid::parse_str(connection_id.strip_prefix("mcp-connection-")?).ok()?;
    if id.get_version_num() != 4 {
        return None;
    }
    let base = HostClient::default_endpoint();
    #[cfg(windows)]
    {
        Some(format!("{base}-extension-{}", id.simple()))
    }
    #[cfg(unix)]
    {
        unix_endpoint(std::path::Path::new(&base), id)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = base;
        None
    }
}

#[cfg(unix)]
pub(crate) fn unix_endpoint(base: &std::path::Path, id: Uuid) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;

    // A complete lowercase UUID avoids case-folding aliases and unnecessary
    // suffixes under macOS's shorter sockaddr_un path capacity.
    let path = base.with_file_name(id.simple().to_string());
    // SAFETY: sockaddr_un contains only integer/character fields; zero is valid.
    // The value is never passed to the OS, only its native path capacity is read.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return None;
    }
    path.to_str().map(str::to_owned)
}

pub(crate) struct ProviderListener {
    handle: JoinHandle<Result<(), dcc_cua_host::HostError>>,
    owner: String,
}

pub(crate) async fn ensure(
    listener: &mut Option<ProviderListener>,
    driver: &ComputerUseDriver,
    connection_id: Option<&str>,
    methods: &[String],
    owner: &str,
    target: &dcc_cua_host::TrustedTaskAuthorizationTarget,
) -> Result<(), String> {
    if methods
        .iter()
        .any(|method| method.starts_with("browser_extension_"))
        && let Some(id) = connection_id
    {
        if listener.as_ref().is_some_and(ProviderListener::is_finished) {
            return Err("provider transport stopped; start a fresh MCP connection".into());
        }
        if listener
            .as_ref()
            .is_some_and(|listener| !listener.is_owned_by(owner))
        {
            return Err(
                "another extension task owns this connection; revoke it before pairing".into(),
            );
        }
        if listener.is_none() {
            *listener = Some(ProviderListener::start(driver.clone(), id, owner, target).await?);
        }
    }
    Ok(())
}

impl ProviderListener {
    pub(crate) async fn start(
        driver: ComputerUseDriver,
        connection_id: &str,
        owner: &str,
        target: &dcc_cua_host::TrustedTaskAuthorizationTarget,
    ) -> Result<Self, String> {
        let endpoint = endpoint(connection_id).ok_or("invalid provider transport identity")?;
        let dcc_cua_host::TrustedTaskAuthorizationTarget::ExactWindow {
            process_id,
            window_handle,
        } = *target
        else {
            return Err("extension pairing requires an exact existing browser window".into());
        };
        let services = HostSecurityServices::browser_extension_transport_for_window(
            process_id,
            window_handle,
            owner,
        );
        let (ready, received) = oneshot::channel();
        let handle = tokio::spawn(async move {
            dcc_cua_host::run_endpoint_with_security_services_ready(
                driver, endpoint, services, ready,
            )
            .await
        });
        // Own the task before awaiting so cancellation also aborts this server.
        Self {
            handle,
            owner: owner.to_owned(),
        }
        .wait_ready(received, std::time::Duration::from_secs(5))
        .await
    }

    async fn wait_ready(
        mut self,
        ready: oneshot::Receiver<()>,
        timeout: std::time::Duration,
    ) -> Result<Self, String> {
        match tokio::time::timeout(timeout, ready).await {
            Ok(Ok(())) if !self.handle.is_finished() => Ok(self),
            Ok(_) => {
                let result = (&mut self.handle).await;
                Err(match result {
                    Ok(Err(error)) => format!("provider transport startup failed: {error}"),
                    _ => "provider transport ended during startup".into(),
                })
            }
            Err(_) => {
                self.handle.abort();
                let _ = (&mut self.handle).await;
                Err("provider transport startup timed out".into())
            }
        }
    }

    pub(crate) fn is_owned_by(&self, owner: &str) -> bool {
        self.owner == owner
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

#[cfg(test)]
mod tests;

impl Drop for ProviderListener {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(crate) fn candidates() -> Result<Vec<ConnectionRecord>, String> {
    candidate_records(list_connections(None))
}

pub(crate) fn candidate_records(
    report: crate::connection_diagnostics::ConnectionsReport,
) -> Result<Vec<ConnectionRecord>, String> {
    if !report.registry_available || report.scan_truncated {
        return Err("complete provider discovery is unavailable; pair again after recovery".into());
    }
    let records: Vec<_> = report
        .connections
        .into_iter()
        .filter(|record| {
            record.state == ConnectionState::Initialized
                && record.tasks.active > 0
                && record
                    .associated_hosts
                    .iter()
                    .any(|host| host.kind == "browser_extension_transport")
                && record.bridge.is_live()
                && endpoint(&record.connection_id).is_some()
        })
        .collect();
    if records.len() > 16 {
        return Err("too many extension tasks; reduce tasks before pairing".into());
    }
    Ok(records)
}

pub(crate) async fn connect(record: &ConnectionRecord) -> Result<HostClient, String> {
    if !record.bridge.is_live() {
        return Err("provider process birth changed".into());
    }
    let client = HostClient::connect_to_process(
        endpoint(&record.connection_id).ok_or("invalid provider endpoint")?,
        "dcc-cua-extension-native-transport",
        record.bridge.pid,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !record.bridge.is_live() {
        return Err("provider process birth changed".into());
    }
    Ok(client)
}

pub(crate) fn revoke(listener: &mut Option<ProviderListener>, owner: &str) {
    if listener
        .as_ref()
        .is_some_and(|listener| listener.is_owned_by(owner))
    {
        *listener = None;
    }
}

pub(crate) fn connection_id(
    diagnostics: Option<&crate::connection_diagnostics::ConnectionDiagnostics>,
) -> Option<String> {
    diagnostics.map(|diagnostics| diagnostics.snapshot().connection_id)
}

pub(crate) fn append_diagnostics(
    listener: &Option<ProviderListener>,
    hosts: &mut Vec<crate::connection_diagnostics::AssociatedHost>,
) {
    if listener.is_some() {
        let mut transport = crate::connection_diagnostics::AssociatedHost::embedded(None);
        transport.kind = "browser_extension_transport".into();
        hosts.push(transport);
    }
}
