use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use crate::contracts::{ComputerUseObservation, OBSERVATION_COUNTER};
use crate::runtime::ComputerUseSession;
use crate::window_target::WindowTarget;

pub(crate) fn semantic_observation(
    session_id: &str,
    target: &WindowTarget,
    accessibility: &Value,
) -> ComputerUseObservation {
    let backend = accessibility["backend"]
        .as_str()
        .unwrap_or("cua-driver-sdk")
        .to_owned();
    ComputerUseObservation {
        observation_id: format!(
            "{session_id}-{}",
            OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
        ),
        window_handle: target.window_id,
        process_id: target.pid,
        window_title: target.title.clone(),
        width: target.bounds[2].max(0) as u32,
        height: target.bounds[3].max(0) as u32,
        source_rect: target.bounds,
        capture_backend: backend.clone(),
        capture_provenance: json!({
            "backend": backend,
            "pixels_captured": false,
            "scope": "window",
            "accessibility_available": accessibility["accessibility_available"]
                .as_bool()
                .unwrap_or(true),
            "accessibility_backend": accessibility["backend"],
            "process_id": target.pid,
            "window_handle": target.window_id,
        }),
        session_id: session_id.to_owned(),
    }
}

impl ComputerUseSession {
    /// Return the latest observation metadata for callers that act on it.
    pub fn latest_observation(&self) -> Option<&ComputerUseObservation> {
        self.observation.as_ref()
    }

    /// Return the latest pixel or semantic observation fence.
    pub fn latest_observation_id(&self) -> Option<&str> {
        self.observation
            .as_ref()
            .map(|observation| observation.observation_id.as_str())
    }
}
