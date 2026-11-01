use super::*;
use rstest::rstest;
use std::time::{SystemTime, UNIX_EPOCH};

fn future() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000
}

#[rstest]
fn exact_session_target_binding_cannot_be_replayed_or_widened() {
    let authorization = NativeTaskAuthorization::new(42, 7, future());
    assert!(authorization.validate("session-1", 42, 7).is_err());
    assert!(authorization.bind_session("session-1", 43, 7).is_err());
    assert!(authorization.bind_session("session-1", 42, 8).is_err());
    authorization.bind_session("session-1", 42, 7).unwrap();
    authorization.validate("session-1", 42, 7).unwrap();
    assert!(authorization.bind_session("session-2", 42, 7).is_err());
    assert!(authorization.validate("session-2", 42, 7).is_err());
    assert!(authorization.validate("session-1", 43, 7).is_err());
    assert!(authorization.validate("session-1", 42, 8).is_err());
}

#[rstest]
fn expiry_and_revocation_fail_closed() {
    let expired = NativeTaskAuthorization::new(42, 7, 0);
    assert!(expired.bind_session("session-1", 42, 7).is_err());
    let authorization = NativeTaskAuthorization::new(42, 7, future());
    authorization.bind_session("session-1", 42, 7).unwrap();
    authorization.revoke();
    assert!(authorization.validate("session-1", 42, 7).is_err());
    assert!(authorization.bind_session("session-1", 42, 7).is_err());
}

#[rstest]
fn only_explicit_normal_login_identity_authorization_relaxes_default_policy() {
    let mut target = crate::window_target::WindowTarget {
        pid: 42,
        window_id: 7,
        app_name: "example-login.exe".into(),
        title: "Login".into(),
        bounds: [0, 0, 640, 480],
        is_on_screen: true,
        is_minimized: false,
        z_index: Some(0),
        is_foreground: true,
    };
    let policy = crate::window_target::validate_target_policy_with_authorization;
    assert!(policy(&target, "session-1", None).is_err());
    let authorization = NativeTaskAuthorization::new(42, 7, future());
    authorization.bind_session("session-1", 42, 7).unwrap();
    policy(&target, "session-1", Some(&authorization)).unwrap();
    assert!(policy(&target, "session-2", Some(&authorization)).is_err());
    for application in [
        "consent.exe",
        "CredentialUIBroker.exe",
        "LogonUI.exe",
        "lsass.exe",
        "winlogon.exe",
        "powershell.exe",
        "pwsh.exe",
        "bash",
        "python3.12",
        "node.exe",
        "example-security.exe",
        "example-consent.exe",
        "com.apple.loginwindow",
        "login",
        "gdm-password",
        "polkit-gnome-authentication-agent",
        "",
        "native-window",
    ] {
        target.app_name = application.into();
        assert!(
            policy(&target, "session-1", Some(&authorization)).is_err(),
            "{application}"
        );
    }
    target.app_name = "example-login.exe".into();
    authorization.revoke();
    assert!(policy(&target, "session-1", Some(&authorization)).is_err());
}

#[rstest]
fn native_identity_and_capture_share_one_constructor_owned_lifetime() {
    let lifetime = std::sync::Arc::new(RecordingAuthorization::new(future()));
    let authorization = NativeTaskAuthorization::with_shared_lifetime(42, 7, lifetime.clone());
    authorization.bind_session("session-1", 42, 7).unwrap();
    lifetime.with_active(|| ()).unwrap();
    lifetime.revoke();
    assert!(authorization.validate("session-1", 42, 7).is_err());
    assert!(lifetime.with_active(|| ()).is_err());
}
