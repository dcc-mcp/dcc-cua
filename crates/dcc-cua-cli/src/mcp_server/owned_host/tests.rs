use super::*;
use tokio::sync::oneshot;

#[tokio::test]
async fn timeout_retains_the_original_host_and_completion_is_idempotent() {
    let (release, wait) = oneshot::channel();
    let mut host = OwnedHostTask::spawn(async move {
        wait.await.unwrap();
        Ok(())
    });
    assert_eq!(host.finish(Duration::ZERO).await["completed"], false);
    assert!(host.handle.is_some());
    release.send(()).unwrap();
    let completion = host.finish(Duration::from_secs(1)).await;
    assert_eq!(completion, json!({"completed":true,"success":true}));
    assert!(host.handle.is_none());
    assert_eq!(host.finish(Duration::ZERO).await, completion);
}

#[tokio::test]
async fn cancelled_wait_does_not_abort_the_host() {
    let (release, wait) = oneshot::channel();
    let mut host = OwnedHostTask::spawn(async move {
        wait.await.unwrap();
        Ok(())
    });
    let mut finish = Box::pin(host.finish(Duration::from_secs(60)));
    assert!(matches!(
        futures_poll(finish.as_mut()),
        std::task::Poll::Pending
    ));
    drop(finish);
    release.send(()).unwrap();
    assert_eq!(host.finish(Duration::from_secs(1)).await["success"], true);
}

fn futures_poll<F: Future>(future: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    let waker = std::task::Waker::noop();
    future.poll(&mut std::task::Context::from_waker(waker))
}

#[tokio::test]
async fn dropping_one_owner_does_not_abort_admitted_work_or_another_host() {
    let (release, wait) = oneshot::channel();
    let (done, completion) = oneshot::channel();
    let first = OwnedHostTask::spawn(async move {
        wait.await.unwrap();
        done.send(()).unwrap();
        Ok(())
    });
    let mut second = OwnedHostTask::spawn(async { Ok(()) });
    drop(first);
    assert_eq!(second.finish(Duration::from_secs(1)).await["success"], true);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), completion)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn host_error_and_panic_are_not_successful_cleanup() {
    let mut failed = OwnedHostTask::spawn(async { Err("cleanup failed".into()) });
    let failure = failed.finish(Duration::from_secs(1)).await;
    assert_eq!(failure["completed"], true);
    assert_eq!(failure["success"], false);
    assert_eq!(failure["error"], "cleanup failed");
    let mut panicked = OwnedHostTask::spawn(async { panic!("synthetic Host panic") });
    let failure = panicked.finish(Duration::from_secs(1)).await;
    assert_eq!(failure["completed"], true);
    assert_eq!(failure["success"], false);
}
