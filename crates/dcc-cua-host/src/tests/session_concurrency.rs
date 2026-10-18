use rstest::rstest;

use super::super::*;
use crate::request_handler::ensure_connection_session_capacity;

#[rstest]
fn connection_session_limit_accepts_the_last_slot() {
    assert!(ensure_connection_session_capacity(MAX_SESSIONS_PER_CONNECTION - 1).is_ok());
}

#[rstest]
fn connection_session_limit_rejects_additional_agent_work() {
    let error = ensure_connection_session_capacity(MAX_SESSIONS_PER_CONNECTION).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("connection session limit reached")
    );
    assert!(
        error
            .to_string()
            .contains(&MAX_SESSIONS_PER_CONNECTION.to_string())
    );
    assert_eq!(error_code(&error), "session_limit_reached");
}

#[rstest]
fn absent_session_guidance_stays_scoped_to_this_connection() {
    let mut sessions = std::collections::HashMap::new();
    let error = session_with_capability(
        &mut sessions,
        "PRIVATE_SESSION",
        "PRIVATE_GRANT",
        "PRIVATE_CAPABILITY",
    )
    .err()
    .unwrap();
    let response = host_error_response(&error);
    assert_eq!(response["code"], "session_not_found");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("this Host connection")
    );
    assert!(!response.to_string().contains("PRIVATE_"));
    assert!(sessions.is_empty());
}
