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
