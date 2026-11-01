//! Real Host -> Core -> browser provider. No packaged MCP or native keyring is used.
use super::LoginFixture;
use super::fixture::{HANDLE, SECRET};
use async_trait::async_trait;
use dcc_cua_client::{HostClient, HostClientError, LogicalTaskSession, SnapshotTransport};
use dcc_cua_core::{ComputerUseDriver, ComputerUseOwnedBrowserLaunchSpec};
use dcc_cua_host::{
    HostSecretVault, HostSecretVaultError, HostSecurityServices, SecretValue,
    TrustedTaskActionScope, TrustedTaskAuthorizationRegistration, TrustedTaskAuthorizationTarget,
    process_connection_with_security_services, trusted_task_authorization_broker,
};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct FixtureVault {
    resolutions: AtomicUsize,
}

#[async_trait]
impl HostSecretVault for FixtureVault {
    async fn resolve(&self, handle: &str) -> Result<SecretValue, HostSecretVaultError> {
        if handle != HANDLE {
            return Err(HostSecretVaultError::NotFound);
        }
        self.resolutions.fetch_add(1, Ordering::SeqCst);
        SecretValue::new(SECRET)
    }
    async fn store(&self, _: &str, _: SecretValue) -> Result<(), HostSecretVaultError> {
        Err(HostSecretVaultError::Unavailable)
    }
}

#[rstest]
#[case("two_factor")]
#[case("captcha")]
#[case("new_legal_terms")]
#[tokio::test]
#[ignore = "requires an isolated interactive Windows desktop and task-owned Chromium; run explicitly"]
async fn owned_browser_login_uses_vault_reuses_cookie_and_stops_at_human_boundary(
    #[case] reason: &'static str,
) {
    let fixture = LoginFixture::new();
    let origin = fixture.origin();
    let vault = Arc::new(FixtureVault::default());
    let launch: ComputerUseOwnedBrowserLaunchSpec =
        serde_json::from_value(json!({"browser":"chromium","profile":"isolated_new"})).unwrap();
    let (issuer, authority) = trusted_task_authorization_broker();
    let receipt = issuer
        .register(TrustedTaskAuthorizationRegistration {
            connection_id: None,
            task_id: Some("synthetic-login".into()),
            task_grant_id: "synthetic-login-grant".into(),
            application_label: "Controlled browser fixture".into(),
            target: TrustedTaskAuthorizationTarget::OwnedBrowser(launch),
            allowed_host_methods: [
                "browser_snapshot",
                "browser_navigate",
                "browser_type",
                "browser_click",
            ]
            .iter()
            .map(|value| (*value).into())
            .collect(),
            allowed_actions: vec![TrustedTaskActionScope {
                action: "browser_type".into(),
                input_kind: "browser".into(),
                secret_input: true,
                authorization_category: "credential".into(),
                browser_origin: Some(origin.clone()),
            }],
            allowed_browser_origins: vec![origin.clone()],
            browser_scope: None,
            recording_output_dir: None,
            capture_preparation: None,
            expires_at_unix_ms: now_ms() + 180_000,
        })
        .unwrap_or_else(|_| panic!("synthetic login registration refused"));
    let driver = ComputerUseDriver::create()
        .unwrap_or_else(|_| panic!("native provider unavailable for opt-in login test"));
    let services = HostSecurityServices::default()
        .with_task_authorization_host(authority)
        .with_secret_vault(vault.clone());
    let (client_io, host_io) = tokio::io::duplex(256 * 1024);
    let host = tokio::spawn(async move {
        process_connection_with_security_services(driver, host_io, services).await
    });
    let mut client =
        HostClient::from_stream_with_transport(client_io, SnapshotTransport::BinaryFrame);
    client
        .hello("synthetic-login-acceptance")
        .await
        .unwrap_or_else(|_| panic!("synthetic Host handshake refused"));
    let opened = client.open_logical_task_session("synthetic-login", json!({
        "task_grant_id":"synthetic-login-grant","application_label":"Controlled browser fixture",
        "owned_browser_launch":launch,"allowed_browser_origins":[origin],
        "allow_browser_input":true,"allow_trusted_confirmation":true,
        "task_authorization_id":receipt.authorization_id,
        "task_authorization_window_capability":receipt.window_capability
    }), 60_000).await;
    let mut session = match opened {
        Ok(session) => session,
        Err(_) => {
            let _ = tokio::time::timeout(Duration::from_secs(15), host).await;
            panic!(
                "owned isolated browser startup refused; no fallback or existing profile is permitted"
            );
        }
    };
    let target = session.target();
    let pid = target["process_id"].as_u64().expect("exact browser PID");
    let hwnd = target["window_handle"]
        .as_u64()
        .expect("exact browser HWND");
    eprintln!(
        "provider=dcc-cua runtime={} pid={pid} hwnd={hwnd}",
        env!("CARGO_PKG_VERSION")
    );
    let result = login_flow(
        &mut session,
        &fixture,
        &vault,
        reason,
        &issuer,
        &receipt.authorization_id,
    )
    .await;
    let _ = issuer.revoke(&receipt.authorization_id);
    let closed = session.close().await;
    let cleanly_closed = closed.is_ok();
    drop(closed);
    let cleanup = tokio::time::timeout(Duration::from_secs(15), host).await;
    assert!(
        matches!(cleanup, Ok(Ok(Ok(())))),
        "native login test Host cleanup was not confirmed"
    );
    assert!(cleanly_closed, "native login task stop was not confirmed");
    assert!(
        result.is_ok(),
        "native login acceptance stage failed: {}",
        result.err().unwrap_or_default()
    );
}

async fn login_flow(
    session: &mut LogicalTaskSession,
    fixture: &LoginFixture,
    vault: &FixtureVault,
    reason: &'static str,
    issuer: &dcc_cua_host::TrustedTaskAuthorizationIssuer,
    authorization_id: &str,
) -> Result<(), &'static str> {
    let binding = request(
        session,
        "browser_snapshot",
        json!({"request":{"snapshot_format":"semantic_v2"}}),
    )
    .await?;
    let state = &binding["result"]["structuredContent"];
    if state["binding_quality"] != "exact" {
        return Err("browser binding was not exact");
    }
    let target = state["target_id"]
        .as_str()
        .ok_or("browser target missing")?
        .to_owned();
    let tab = state["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["active"] == true))
        .and_then(|tab| tab["tab_id"].as_str())
        .ok_or("exact active tab missing")?
        .to_owned();
    navigate(session, &target, &tab, &fixture.page_url()).await?;
    let snapshot = observe(session, &target, &tab).await?;
    let user = input(
        &snapshot,
        &target,
        &tab,
        "fixture-user",
        json!({"text":"fixture-user"}),
    )?;
    request(session, "browser_type", json!({"request":user})).await?;
    let snapshot = observe(session, &target, &tab).await?;
    let secret = input(
        &snapshot,
        &target,
        &tab,
        "fixture-password",
        json!({"secret_handle":HANDLE}),
    )?;
    request(session, "browser_type", json!({"request":secret.clone()})).await?;
    if vault.resolutions.load(Ordering::SeqCst) != 1 {
        return Err("secret vault resolution count");
    }
    let snapshot = observe(session, &target, &tab).await?;
    let submit = ref_for(&snapshot, "fixture-submit")?;
    request(
        session,
        "browser_click",
        json!({"request":{
            "target_id":target,"tab_id":tab,"snapshot_id":snapshot_id(&snapshot)?,"ref":submit
        }}),
    )
    .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fixture.counts().accepted_logins != 1 || fixture.counts().authenticated_requests == 0 {
        if tokio::time::Instant::now() >= deadline {
            return Err("server-side authenticated session not established");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The second visit must reuse the HttpOnly cookie without resolving another credential.
    navigate(session, &target, &tab, &fixture.page_url()).await?;
    let reused = observe(session, &target, &tab).await?;
    if !reused.to_string().contains("fixture-authenticated")
        || fixture.counts().submissions != 1
        || vault.resolutions.load(Ordering::SeqCst) != 1
    {
        return Err("existing session was not reused");
    }
    fixture.boundary(reason);
    navigate(session, &target, &tab, &fixture.page_url()).await?;
    let boundary = observe(session, &target, &tab).await?;
    if !boundary
        .to_string()
        .contains(&format!("needs_user_action:{reason}"))
    {
        return Err("fixture boundary evidence missing");
    }
    // MCP's typed pause is covered separately. Here the same broker revocation must
    // prevent Host credential resolution and provider dispatch after a human boundary.
    issuer
        .revoke(authorization_id)
        .map_err(|_| "boundary lease revocation failed")?;
    let refused = session
        .request("browser_type", json!({"request":secret}))
        .await;
    if !matches!(refused,Err(HostClientError::Remote{code,..}) if code == "task_authorization_revoked")
    {
        return Err("revoked login task was not refused before dispatch");
    }
    if vault.resolutions.load(Ordering::SeqCst) != 1 || fixture.counts().boundary_submissions != 0 {
        return Err("human boundary caused an automatic credential retry");
    }
    Ok(())
}

async fn request(
    session: &mut LogicalTaskSession,
    method: &str,
    params: Value,
) -> Result<Value, &'static str> {
    if params.to_string().contains(SECRET) {
        return Err("plaintext secret crossed request IPC");
    }
    let response = session
        .request(method, params)
        .await
        .map_err(|_| "native browser operation refused")?
        .value;
    if response.to_string().contains(SECRET) {
        return Err("secret leaked in Host response");
    }
    if response["result"]["isError"] == true
        || response["result"]["structuredContent"]["status"] == "error"
    {
        return Err("native browser returned a typed refusal");
    }
    Ok(response)
}

async fn navigate(
    session: &mut LogicalTaskSession,
    target: &str,
    tab: &str,
    url: &str,
) -> Result<(), &'static str> {
    request(
        session,
        "browser_navigate",
        json!({"request":{"target_id":target,"tab_id":tab,"url":url,"delivery_mode":"foreground"}}),
    )
    .await?;
    Ok(())
}
async fn observe(
    session: &mut LogicalTaskSession,
    target: &str,
    tab: &str,
) -> Result<Value, &'static str> {
    request(
        session,
        "browser_snapshot",
        json!({"request":{"target_id":target,"tab_id":tab,"snapshot_format":"semantic_v2"}}),
    )
    .await
}
fn snapshot_id(snapshot: &Value) -> Result<&str, &'static str> {
    let state = &snapshot["result"]["structuredContent"];
    state["snapshot_id"]
        .as_str()
        .or_else(|| state["snapshot"]["id"].as_str())
        .ok_or("fresh browser snapshot id missing")
}
fn ref_for(snapshot: &Value, label: &str) -> Result<String, &'static str> {
    let refs = snapshot["result"]["structuredContent"]["refs"]
        .as_array()
        .ok_or("browser semantic refs missing")?;
    let matches = refs
        .iter()
        .filter(|entry| {
            entry["label"]
                .as_str()
                .or_else(|| entry["name"].as_str())
                .is_some_and(|name| name.contains(label))
        })
        .filter_map(|entry| entry["ref"].as_str())
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("controlled login control was absent or ambiguous");
    }
    Ok(matches[0].to_owned())
}
fn input(
    snapshot: &Value,
    target: &str,
    tab: &str,
    label: &str,
    source: Value,
) -> Result<Value, &'static str> {
    let mut request = json!({"target_id":target,"tab_id":tab,"snapshot_id":snapshot_id(snapshot)?,"ref":ref_for(snapshot,label)?,"replace":true});
    request
        .as_object_mut()
        .unwrap()
        .extend(source.as_object().ok_or("invalid input source")?.clone());
    Ok(request)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
