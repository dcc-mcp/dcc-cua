//! Connection-local completion ownership, not a process or session pool.

use std::future::Future;
use std::time::Duration;

use dcc_cua_client::LogicalTaskSession;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::TaskProposal;

pub(super) struct OwnedHostTask {
    handle: Option<JoinHandle<Result<(), String>>>,
    completion: Option<Value>,
}

impl OwnedHostTask {
    pub(super) fn spawn(future: impl Future<Output = Result<(), String>> + Send + 'static) -> Self {
        Self {
            handle: Some(tokio::spawn(future)),
            completion: None,
        }
    }

    pub(super) async fn finish(&mut self, timeout: Duration) -> Value {
        if let Some(completion) = &self.completion {
            return completion.clone();
        }
        let handle = self.handle.as_mut().expect("unfinished Host remains owned");
        // Poll by reference: timeout or cancellation keeps the original handle
        // available for a later cleanup attempt. Never abort a native operation.
        let completion = match tokio::time::timeout(timeout, handle).await {
            Err(_) => {
                return json!({"completed":false,"success":false,
                    "error":"Host connection cleanup is still pending"});
            }
            Ok(Ok(Ok(()))) => json!({"completed":true,"success":true}),
            Ok(Ok(Err(error))) => json!({"completed":true,"success":false,"error":error}),
            Ok(Err(error)) => json!({"completed":true,"success":false,"error":error.to_string()}),
        };
        self.handle.take();
        self.completion = Some(completion.clone());
        completion
    }
}

// Deliberately no abort-on-drop: the Host owns ordered native rollback after
// its client transport closes. A dropped owner does not attest completion.

pub(super) async fn stop_owned_task_session(session: LogicalTaskSession) -> Value {
    let session_id = session.session_id().to_owned();
    let mut client = session.into_client();
    match client
        .request_with_timeout(
            "stop_session",
            json!({"session_id":session_id}),
            std::time::Duration::from_secs(60),
        )
        .await
    {
        Ok(response)
            if response.value["type"] == "session_stopped"
                && response.value["session_id"] == session_id
                && response.value["success"].is_boolean()
                && response.value["active"] == false
                && response.value["cleanup_pending"] == false =>
        {
            response.value
        }
        Ok(response) => json!({"success":false,"active":null,"cleanup_pending":true,
            "error":"Host did not acknowledge authoritative session cleanup",
            "host_response":response.value}),
        Err(error) => json!({"success":false,"active":null,"cleanup_pending":true,
            "error":error.to_string()}),
    }
}

pub(super) async fn finish_owned_host_task(proposal: &mut TaskProposal) {
    let Some(host_task) = proposal.host_task.as_mut() else {
        return;
    };
    let cleanup = proposal.cleanup.get_or_insert_with(|| {
        json!({"success":false,"active":null,"cleanup_pending":true,
            "error":"task session cleanup acknowledgement was interrupted or unavailable"})
    });
    // Transport completion is separate evidence: it cannot replace a missing
    // authoritative session/recording cleanup acknowledgement.
    // Publish pending evidence before awaiting so cancelling this wait cannot
    // leave an earlier session acknowledgement looking like completed teardown.
    cleanup["host_connection_completion"] = json!({"completed":false,"success":false});
    cleanup["host_connection_completion"] = host_task.finish(std::time::Duration::from_secs(5)).await;
}

pub(super) fn task_cleanup_payload(cleanup: Option<&Value>) -> Value {
    let Some(cleanup) = cleanup else {
        return Value::Null;
    };
    let mut payload = cleanup.clone();
    if let Some(completion) = cleanup.get("host_connection_completion") {
        // Preserve the authoritative session receipt unchanged while exposing
        // aggregate flags for callers that do not inspect the task status.
        let mut session_cleanup = cleanup.clone();
        if let Some(object) = session_cleanup.as_object_mut() {
            object.remove("host_connection_completion");
        }
        payload["session_cleanup"] = session_cleanup;
        if completion["completed"] != true {
            payload["success"] = json!(false);
            payload["cleanup_pending"] = json!(true);
        } else if completion["success"] != true {
            payload["success"] = json!(false);
        }
    }
    payload
}

pub(super) fn stopped_task_status(cleanup: Option<&Value>) -> &'static str {
    if let Some(completion) = cleanup.and_then(|value| value.get("host_connection_completion")) {
        if completion["completed"] != true {
            return "cleanup_unknown";
        }
        if completion["success"] != true {
            return "cleanup_failed";
        }
    }
    match cleanup {
        Some(cleanup) if cleanup["cleanup_pending"] == false && cleanup["active"] == false => {
            if cleanup["success"] == true {
                "stopped"
            } else {
                "cleanup_failed"
            }
        }
        _ => "cleanup_unknown",
    }
}

#[cfg(test)]
mod tests;
