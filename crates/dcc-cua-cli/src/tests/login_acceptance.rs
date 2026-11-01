//! Fixture oracle checks run normally; the native provider row is explicitly opt-in.
use rstest::rstest;

mod fixture;
use fixture::{LoginFixture, SECRET};

#[rstest]
fn login_fixture_requires_credentials_and_reuses_an_http_only_session() {
    let fixture = LoginFixture::new();
    let signed_out = fixture.request("GET", "/fixture", "", None);
    assert!(
        signed_out.contains("fixture-signed-out"),
        "unexpected controlled GET result: {signed_out}"
    );
    assert!(!signed_out.contains(SECRET));
    let rejected = fixture.request(
        "POST",
        "/login",
        "user=fixture-user&password=incorrect",
        None,
    );
    assert!(rejected.starts_with("HTTP/1.1 403"));
    let accepted = fixture.request(
        "POST",
        "/login",
        &format!("user=fixture-user&password={SECRET}"),
        None,
    );
    assert!(accepted.starts_with("HTTP/1.1 303"));
    assert!(accepted.contains("HttpOnly; SameSite=Strict"));
    assert!(!accepted.contains(SECRET));
    let cookie = accepted
        .lines()
        .find_map(|line| line.strip_prefix("Set-Cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let session = fixture.request("GET", "/fixture", "", Some(cookie));
    assert!(session.contains("fixture-authenticated"));
    assert!(!session.contains("type=password"));
    assert_eq!(fixture.counts().accepted_logins, 1);
    assert_eq!(fixture.counts().submissions, 2);
    assert_eq!(fixture.counts().authenticated_requests, 1);
}

#[rstest]
#[case("two_factor")]
#[case("captcha")]
#[case("new_legal_terms")]
fn login_fixture_boundaries_never_complete_credential_submission(#[case] reason: &'static str) {
    let fixture = LoginFixture::new();
    fixture.boundary(reason);
    let boundary = fixture.request("GET", "/fixture", "", None);
    assert!(boundary.contains(&format!("needs_user_action:{reason}")));
    assert!(!boundary.contains("<form"));
    let refused = fixture.request(
        "POST",
        "/login",
        &format!("user=fixture-user&password={SECRET}"),
        None,
    );
    assert!(refused.starts_with("HTTP/1.1 409"));
    assert!(!refused.contains(SECRET));
    assert_eq!(fixture.counts().accepted_logins, 0);
    assert_eq!(fixture.counts().boundary_submissions, 1);
}

#[cfg(windows)]
mod native;

#[rstest]
fn login_fixture_serves_requests_while_browser_preconnect_is_idle() {
    let fixture = LoginFixture::new();
    let idle = fixture.idle_connection();
    let started = std::time::Instant::now();
    let response = fixture.request("GET", "/fixture", "", None);
    assert!(response.contains("fixture-signed-out"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "idle preconnect serialized a real browser request"
    );
    drop(idle);
}

#[rstest]
fn login_fixture_does_not_close_a_connection_before_delayed_headers_arrive() {
    use std::io::{Read, Write};
    let fixture = LoginFixture::new();
    let mut connection = fixture.idle_connection();
    connection
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(60));
    connection
        .write_all(b"GET /fixture HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    assert!(
        response.contains("fixture-signed-out"),
        "accepted connection was closed before request headers"
    );
}
