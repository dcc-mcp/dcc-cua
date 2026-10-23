//! Connection-local completion ownership, not a process or session pool.

use std::future::Future;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::task::JoinHandle;

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

#[cfg(test)]
mod tests;
