//! Keep verification timing around the existing provider call only.
use super::*;

pub(super) async fn verify(
    host: &mut HostSession,
    session_id: &str,
    mode: SnapshotTransport,
    expect: Value,
    timeout_ms: Option<u64>,
    stable_samples: Option<u64>,
    include_screenshot: bool,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let verification = timed(
        Phase::Verification,
        host.session
            .verify_state(expect, timeout_ms, stable_samples, include_screenshot),
    )
    .await;
    let verification = host.finish_observation_sensitive_attempt(verification)?;
    let image_transport = verification
        .image
        .map(|image| image_response(image, mode, &mut host.latest_shared_image))
        .transpose()?;
    let mut response = json!({
        "type": "state_verified", "session_id": session_id, "result": verification.value,
    });
    let attachment = if let Some((descriptor, attachment)) = image_transport {
        response["image"] = descriptor;
        attachment
    } else {
        None
    };
    Ok((response, attachment))
}
