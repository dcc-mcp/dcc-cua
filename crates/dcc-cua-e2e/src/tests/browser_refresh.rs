use super::*;
use rstest::rstest;
use std::{future::Future, pin::Pin};

pub(super) fn safe_pre_dispatch_refresh(message: &str) -> bool {
    let expected = [
        ("action_attempted", "false"),
        ("input_sent", "false"),
        ("action_completion_unknown", "false"),
        ("local_session_invalidated", "false"),
        ("session_remains_active", "true"),
        ("automatic_input", "false"),
        ("blind_retry", "false"),
        ("fresh_observation_required", "true"),
        ("exact_target_revalidation_required", "true"),
    ];
    let Some(fields) = message
        .strip_prefix("SessionRefreshRequired: session_refresh_required: ")
        .or_else(|| message.strip_prefix("session_refresh_required: "))
    else {
        return false;
    };
    let mut observed = [false; 9];
    for segment in fields.split(';') {
        let field = segment.trim();
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        if key.contains(':') || value.contains(':') || value.contains('=') {
            return false;
        }
        let Some(index) = expected
            .iter()
            .position(|(expected_key, _)| key.trim() == *expected_key)
        else {
            return false;
        };
        if observed[index] || value.trim() != expected[index].1 {
            return false;
        }
        observed[index] = true;
    }
    observed.into_iter().all(|present| present)
}

fn valid_snapshot_bound_request_fields(
    method: &str,
    request_fields: &serde_json::Map<String, Value>,
) -> bool {
    let expected: &[&str] = match method {
        "browser_click" => &[],
        "browser_type" => &["text", "replace"],
        "browser_pointer" => &["action", "input_route"],
        "browser_set_input_files" => &["files"],
        "browser_download" => &["destination_root"],
        _ => return false,
    };
    request_fields.len() == expected.len()
        && expected.iter().all(|key| request_fields.contains_key(*key))
}

type BrowserRequestFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HostResponse, HostClientError>> + 'a>>;

trait BrowserRequestTransport {
    fn request<'a>(&'a mut self, method: &str, params: Value) -> BrowserRequestFuture<'a>;
}

impl BrowserRequestTransport for HostProcess {
    fn request<'a>(&'a mut self, method: &str, params: Value) -> BrowserRequestFuture<'a> {
        let method = method.to_owned();
        Box::pin(async move { self.client_mut().request(method, params).await })
    }
}

async fn run_snapshot_bound_browser_mutation<T: BrowserRequestTransport>(
    transport: &mut T,
    method: &str,
    target_id: &str,
    tab_id: &str,
    window_capability: &str,
    element_text: &str,
    request_fields: Value,
) -> Result<HostResponse, HostClientError> {
    let request_fields = request_fields
        .as_object()
        .expect("snapshot-bound browser request fields must be an object");
    assert!(
        valid_snapshot_bound_request_fields(method, request_fields),
        "snapshot-bound browser request fields must match the method whitelist"
    );

    for attempt in 0..2 {
        let browser_snapshot = tokio::time::timeout(
            Duration::from_secs(90),
            transport.request(
                "browser_snapshot",
                json!({
                    "session_id": SESSION_ID,
                    "task_grant_id": GRANT_ID,
                    "window_capability": window_capability,
                    "request": {
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "snapshot_format": "semantic_v2"
                    }
                }),
            ),
        )
        .await
        .map_err(|_| HostClientError::Protocol("browser_snapshot exceeded 90 seconds".into()))??;
        let mut request = request_fields.clone();
        request.insert("target_id".to_owned(), json!(target_id));
        request.insert("tab_id".to_owned(), json!(tab_id));
        request.insert(
            "snapshot_id".to_owned(),
            json!(browser_snapshot_id(&browser_snapshot.value)),
        );
        request.insert(
            "ref".to_owned(),
            json!(browser_ref_by_text(&browser_snapshot.value, element_text)),
        );
        let params = json!({
            "session_id": SESSION_ID,
            "task_grant_id": GRANT_ID,
            "window_capability": window_capability,
            "request": request
        });
        let started = Instant::now();
        let response =
            tokio::time::timeout(Duration::from_secs(90), transport.request(method, params))
                .await
                .map_err(|_| HostClientError::Protocol(format!("{method} exceeded 90 seconds")))?;
        match response {
            Ok(response) => {
                eprintln!(
                    "Host request {method:?} completed in {:?}",
                    started.elapsed()
                );
                return Ok(response);
            }
            Err(HostClientError::Remote {
                code,
                message,
                response,
            }) if attempt == 0
                && code == "session_refresh_required"
                && safe_pre_dispatch_refresh(&message) =>
            {
                eprintln!("Host requested a fresh browser snapshot before {method}: {response}");
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("snapshot-bound browser mutation returns or panics within two attempts")
}

pub(super) async fn browser_mutation_with_pre_dispatch_refresh_recovery(
    host: &mut HostProcess,
    method: &str,
    target_id: &str,
    tab_id: &str,
    window_capability: &str,
    element_text: &str,
    request_fields: Value,
) -> HostResponse {
    run_snapshot_bound_browser_mutation(
        host,
        method,
        target_id,
        tab_id,
        window_capability,
        element_text,
        request_fields,
    )
    .await
    .unwrap_or_else(|error| panic!("Host request {method:?} failed: {error:?}"))
}

#[rstest]
fn refresh_recovery_requires_proof_that_no_action_was_attempted() {
    let safe = "SessionRefreshRequired: session_refresh_required: action_attempted=false; input_sent=false; action_completion_unknown=false; local_session_invalidated=false; session_remains_active=true; automatic_input=false; blind_retry=false; fresh_observation_required=true; exact_target_revalidation_required=true";
    assert!(safe_pre_dispatch_refresh(safe));
    for (expected, unsafe_value) in [
        ("action_attempted=false", "action_attempted=true"),
        ("input_sent=false", "input_sent=unknown"),
        (
            "action_completion_unknown=false",
            "action_completion_unknown=true",
        ),
        (
            "local_session_invalidated=false",
            "local_session_invalidated=true",
        ),
        (
            "session_remains_active=true",
            "session_remains_active=false",
        ),
        ("automatic_input=false", "automatic_input=true"),
        ("blind_retry=false", "blind_retry=true"),
        (
            "fresh_observation_required=true",
            "fresh_observation_required=false",
        ),
        (
            "exact_target_revalidation_required=true",
            "exact_target_revalidation_required=false",
        ),
    ] {
        assert!(!safe_pre_dispatch_refresh(
            &safe.replace(expected, unsafe_value)
        ));
    }
    assert!(!safe_pre_dispatch_refresh(&format!(
        "{safe}; action_attempted=false"
    )));
    assert!(!safe_pre_dispatch_refresh(&format!(
        "{safe}; prior_action_attempted=false"
    )));
    assert!(!safe_pre_dispatch_refresh(&safe.replace(
        "action_attempted=false",
        "action_attempted=falsehood"
    )));
    assert!(!safe_pre_dispatch_refresh(&safe.replacen(
        "action_attempted=false",
        "action_attempted=true: action_attempted=false",
        1
    )));
    assert!(!safe_pre_dispatch_refresh(
        safe.strip_prefix("SessionRefreshRequired: session_refresh_required: ")
            .expect("test prefix")
    ));
    assert!(!safe_pre_dispatch_refresh(&format!(
        "{safe}; refresh contract changed"
    )));
}

#[rstest]
fn snapshot_bound_request_fields_reject_old_coordinates_and_refs() {
    assert!(valid_snapshot_bound_request_fields(
        "browser_pointer",
        json!({"action": "double_click", "input_route": "dom_event"})
            .as_object()
            .expect("request object")
    ));
    for request in [
        json!({"action": "drag", "input_route": "dom_event", "destination_ref": "old-ref"}),
        json!({"action": "click", "input_route": "raw_input", "x": 1, "y": 2}),
    ] {
        assert!(!valid_snapshot_bound_request_fields(
            "browser_pointer",
            request.as_object().expect("request object")
        ));
    }
}

#[cfg(test)]
struct ScriptedBrowserTransport {
    responses: std::collections::VecDeque<Result<HostResponse, HostClientError>>,
    calls: Vec<(String, Value)>,
}

#[cfg(test)]
impl BrowserRequestTransport for ScriptedBrowserTransport {
    fn request<'a>(&'a mut self, method: &str, params: Value) -> BrowserRequestFuture<'a> {
        self.calls.push((method.to_owned(), params));
        let response = self
            .responses
            .pop_front()
            .expect("scripted browser response");
        Box::pin(std::future::ready(response))
    }
}

#[cfg(test)]
fn scripted_snapshot(snapshot_id: &str, element_ref: &str) -> HostResponse {
    HostResponse {
        value: json!({
            "result": {"structuredContent": {
                "snapshot_id": snapshot_id,
                "refs": [{"label": "Increment", "ref": element_ref}]
            }}
        }),
        binary_attachment: None,
    }
}

#[cfg(test)]
fn scripted_refresh_error(message: &str) -> HostClientError {
    HostClientError::Remote {
        code: "session_refresh_required".to_owned(),
        message: message.to_owned(),
        response: json!({"type": "error", "code": "session_refresh_required"}),
    }
}

#[rstest]
#[tokio::test]
async fn refresh_recovery_rebinds_snapshot_evidence_once() {
    let safe = "SessionRefreshRequired: session_refresh_required: action_attempted=false; input_sent=false; action_completion_unknown=false; local_session_invalidated=false; session_remains_active=true; automatic_input=false; blind_retry=false; fresh_observation_required=true; exact_target_revalidation_required=true";
    let mut transport = ScriptedBrowserTransport {
        responses: [
            Ok(scripted_snapshot("snapshot-1", "ref-1")),
            Err(scripted_refresh_error(safe)),
            Ok(scripted_snapshot("snapshot-2", "ref-2")),
            Ok(HostResponse {
                value: json!({"success": true}),
                binary_attachment: None,
            }),
        ]
        .into(),
        calls: Vec::new(),
    };

    run_snapshot_bound_browser_mutation(
        &mut transport,
        "browser_click",
        "target",
        "tab",
        "capability",
        "Increment",
        json!({}),
    )
    .await
    .expect("safe pre-dispatch refresh should recover");

    assert_eq!(
        transport
            .calls
            .iter()
            .map(|(method, _)| method.as_str())
            .collect::<Vec<_>>(),
        [
            "browser_snapshot",
            "browser_click",
            "browser_snapshot",
            "browser_click"
        ]
    );
    assert_eq!(transport.calls[1].1["request"]["snapshot_id"], "snapshot-1");
    assert_eq!(transport.calls[1].1["request"]["ref"], "ref-1");
    assert_eq!(transport.calls[3].1["request"]["snapshot_id"], "snapshot-2");
    assert_eq!(transport.calls[3].1["request"]["ref"], "ref-2");
}

#[rstest]
#[tokio::test]
async fn refresh_recovery_refuses_unsafe_or_second_replay() {
    let safe = "SessionRefreshRequired: session_refresh_required: action_attempted=false; input_sent=false; action_completion_unknown=false; local_session_invalidated=false; session_remains_active=true; automatic_input=false; blind_retry=false; fresh_observation_required=true; exact_target_revalidation_required=true";
    for responses in [
        vec![
            Ok(scripted_snapshot("snapshot-1", "ref-1")),
            Err(scripted_refresh_error(
                &safe.replace("action_attempted=false", "action_attempted=true"),
            )),
        ],
        vec![
            Ok(scripted_snapshot("snapshot-1", "ref-1")),
            Err(scripted_refresh_error(safe)),
            Ok(scripted_snapshot("snapshot-2", "ref-2")),
            Err(scripted_refresh_error(safe)),
        ],
    ] {
        let expected_calls = responses.len();
        let mut transport = ScriptedBrowserTransport {
            responses: responses.into(),
            calls: Vec::new(),
        };
        let error = run_snapshot_bound_browser_mutation(
            &mut transport,
            "browser_click",
            "target",
            "tab",
            "capability",
            "Increment",
            json!({}),
        )
        .await
        .expect_err("unsafe or repeated refresh must stop");
        assert!(matches!(
            error,
            HostClientError::Remote { ref code, .. } if code == "session_refresh_required"
        ));
        assert_eq!(transport.calls.len(), expected_calls);
    }
}
