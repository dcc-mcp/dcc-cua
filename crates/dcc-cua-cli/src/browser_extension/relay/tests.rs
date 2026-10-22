use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn paired_native_stream_round_trips_only_its_first_task_owner() {
    let (mut output, mut browser) = tokio::io::duplex(8192);
    let (response_tx, mut responses) = mpsc::channel(1);
    let (reply, returned) = oneshot::channel();
    let browser_reply = async {
        let command = read_native_message(&mut browser).await.unwrap();
        assert_eq!(command["request_id"], "request-1");
        response_tx
            .send(Some(
                json!({"request_id":"request-1","ok":true,"result":{"snapshot_id":"fresh"}}),
            ))
            .await
            .unwrap();
    };
    let mut owner = None;
    let result = dispatch(
        Command {
            deadline: u64::MAX,
            owner: "connection-one:task-one".into(),
            body: json!({"request_id":"request-1","method":"snapshot"}),
            reply,
        },
        &mut owner,
        &mut output,
        &mut responses,
    );
    let (result, _) = tokio::join!(result, browser_reply);
    result.unwrap();
    assert_eq!(returned.await.unwrap()["result"]["snapshot_id"], "fresh");
    for other in ["connection-two:task-one", "connection-one:task-two"] {
        let (reply, returned) = oneshot::channel();
        dispatch(
            Command {
                deadline: u64::MAX,
                owner: other.into(),
                body: json!({"request_id":"forbidden"}),
                reply,
            },
            &mut owner,
            &mut output,
            &mut responses,
        )
        .await
        .unwrap();
        assert_eq!(returned.await.unwrap()["error"]["code"], "pairing_owned");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), read_native_message(&mut browser))
            .await
            .is_err()
    );
}

#[rstest]
#[tokio::test]
async fn native_disconnect_never_reassigns_the_pairing() {
    let (mut output, _browser) = tokio::io::duplex(8192);
    let (tx, mut responses) = mpsc::channel(1);
    tx.send(None).await.unwrap();
    let (reply, _returned) = oneshot::channel();
    let mut owner = None;
    assert!(
        dispatch(
            Command {
                deadline: u64::MAX,
                owner: "connection:task".into(),
                body: json!({"request_id":"uncertain"}),
                reply
            },
            &mut owner,
            &mut output,
            &mut responses
        )
        .await
        .is_err()
    );
    assert_eq!(owner.as_deref(), Some("connection:task"));
}

#[rstest]
#[tokio::test]
async fn expired_native_command_is_neither_claimed_nor_dispatched() {
    let (mut output, mut browser) = tokio::io::duplex(8192);
    let (_tx, mut responses) = mpsc::channel(1);
    let (reply, returned) = oneshot::channel();
    let mut owner = None;
    dispatch(
        Command {
            owner: "expired-task".into(),
            deadline: 0,
            body: json!({"request_id":"expired"}),
            reply,
        },
        &mut owner,
        &mut output,
        &mut responses,
    )
    .await
    .unwrap();
    assert_eq!(returned.await.unwrap()["error"]["code"], "command_expired");
    assert!(owner.is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), read_native_message(&mut browser))
            .await
            .is_err()
    );
}

#[rstest]
fn native_window_binding_requires_one_foreground_window_of_the_parent() {
    let row = json!({"pid":42,"window_id":77,"is_foreground":true});
    assert_eq!(
        super::super::paired_native_window(std::slice::from_ref(&row), 42).unwrap(),
        77
    );
    assert!(super::super::paired_native_window(std::slice::from_ref(&row), 43).is_err());
    assert!(super::super::paired_native_window(&[row.clone(), row], 42).is_err());
    assert!(
        super::super::paired_native_window(
            &[json!({"pid":42,"window_id":77,"is_foreground":false})],
            42
        )
        .is_err()
    );
}

#[rstest]
#[tokio::test]
async fn blocked_native_write_obeys_the_command_deadline_without_retry() {
    let (mut output, _browser) = tokio::io::duplex(1);
    let (_tx, mut responses) = mpsc::channel(1);
    let (reply, _returned) = oneshot::channel();
    let mut owner = None;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let error = dispatch(
        Command {
            owner: "task".into(),
            deadline: now + 20,
            body: json!({"request_id":"blocked"}),
            reply,
        },
        &mut owner,
        &mut output,
        &mut responses,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("unknown"));
    assert_eq!(owner.as_deref(), Some("task"));
}
