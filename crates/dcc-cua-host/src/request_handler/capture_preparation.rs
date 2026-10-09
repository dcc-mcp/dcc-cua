//! Exact capture-preparation request lifecycle and passive response contract.
use super::*;

pub(crate) fn passive_preparation_snapshot_response(
    session_id: &str,
    metadata: Value,
    image: PreparedImageTransport,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    if metadata["passive"] != true
        || metadata["input_authorized"] != false
        || [
            "observation_id",
            "accessibility_state_id",
            "element_token",
            "window_state_id",
        ]
        .iter()
        .any(|key| metadata.get(key).is_some())
    {
        return Err(HostError::Protocol(
            "prepared pixels cannot publish an actionable observation or input token".into(),
        ));
    }
    Ok((
        json!({
            "type":"capture_preparation_snapshot", "session_id":session_id,
            "passive":true, "input_authorized":false, "metadata":metadata,
            "image":image.primary, "images":image.attachments,
            "use_shared_memory":image.use_shared_memory,
        }),
        image.attachment,
    ))
}

pub(super) async fn begin(
    sessions: &mut std::collections::HashMap<String, HostSession>,
    params: CapturePreparationBeginParams,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let CapturePreparationBeginParams {
        session_id,
        task_grant_id,
        window_capability,
        request,
    } = params;
    let generation = interrupt_generation();
    let host =
        authorized_session(sessions, &session_id, &task_grant_id, &window_capability).await?;
    let prior_epoch = host.session.action_evidence_epoch();
    let result = async {
        host.require_capture_preparation_grant(&session_id, "capture_preparation_begin")?;
        let _input_turn = RAW_INPUT_QUEUE.lock().await;
        revalidate_queued_window_mutation(host).await?;
        if interrupt_generation() != generation {
            return Err(HostError::ComputerUse(ComputerUseError::new(
                ComputerUseErrorCode::UserInterrupted,
                "capture preparation was interrupted while awaiting its trusted queue or lease",
            )));
        }
        let mut authorization = host
            .require_capture_preparation_grant(&session_id, "capture_preparation_begin")?
            .clone();
        let expires = host
            .task_authorization
            .as_ref()
            .expect("capture preparation requires a bound lease")
            .expires_at_unix_ms;
        authorization.max_lifetime_ms = authorization
            .max_lifetime_ms
            .min(expires.saturating_sub(crate::task_authorization::unix_time_millis()));
        let value = host
            .session
            .capture_preparation_begin(&request, &authorization, expires)
            .await;
        host.finish_observation_sensitive_attempt(value)
            .map_err(HostError::from)
    }
    .await;
    if host.session.action_evidence_epoch() == prior_epoch {
        host.session.invalidate_action_observations();
    }
    host.invalidate_observations();
    host.latest_preparation_image = None;
    Ok((
        json!({"type":"capture_preparation_begin", "session_id":session_id, "result":result?}),
        None,
    ))
}

pub(super) async fn state(
    sessions: &mut std::collections::HashMap<String, HostSession>,
    params: CapturePreparationBindings,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let CapturePreparationBindings {
        session_id,
        task_grant_id,
        window_capability,
    } = params;
    let host =
        authorized_session(sessions, &session_id, &task_grant_id, &window_capability).await?;
    host.require_capture_preparation_grant(&session_id, "capture_preparation_state")?;
    let result = host.session.capture_preparation_state();
    let result = host.finish_observation_sensitive_attempt(result)?;
    Ok((
        json!({"type":"capture_preparation_state", "session_id":session_id, "result":result}),
        None,
    ))
}

pub(super) async fn stop(
    sessions: &mut std::collections::HashMap<String, HostSession>,
    params: CapturePreparationBindings,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let CapturePreparationBindings {
        session_id,
        task_grant_id,
        window_capability,
    } = params;
    let generation = interrupt_generation();
    let host =
        authorized_session(sessions, &session_id, &task_grant_id, &window_capability).await?;
    host.require_capture_preparation_grant(&session_id, "capture_preparation_stop")?;
    let _input_turn = RAW_INPUT_QUEUE.lock().await;
    revalidate_queued_window_mutation(host).await?;
    if interrupt_generation() != generation {
        return Err(HostError::ComputerUse(ComputerUseError::new(
            ComputerUseErrorCode::UserInterrupted,
            "capture preparation stop was interrupted while awaiting its trusted queue or lease",
        )));
    }
    host.require_capture_preparation_grant(&session_id, "capture_preparation_stop")?;
    let result = host.session.capture_preparation_stop();
    let result = host.finish_observation_sensitive_attempt(result);
    host.invalidate_observations();
    host.latest_preparation_image = None;
    Ok((
        json!({"type":"capture_preparation_stop", "session_id":session_id, "result":result?}),
        None,
    ))
}

pub(super) async fn snapshot(
    sessions: &mut std::collections::HashMap<String, HostSession>,
    mode: SnapshotTransport,
    params: CapturePreparationBindings,
) -> Result<(Value, Option<Vec<u8>>), HostError> {
    let CapturePreparationBindings {
        session_id,
        task_grant_id,
        window_capability,
    } = params;
    let host =
        authorized_session(sessions, &session_id, &task_grant_id, &window_capability).await?;
    host.require_capture_preparation_grant(&session_id, "capture_preparation_snapshot")?;
    let result = host.session.capture_preparation_snapshot().await;
    let result = host.finish_observation_sensitive_attempt(result);
    host.invalidate_observations();
    let (image, metadata) = result?;
    let image = prepare_image_transport(vec![image], mode, &mut host.latest_preparation_image)?;
    passive_preparation_snapshot_response(&session_id, metadata, image)
}
