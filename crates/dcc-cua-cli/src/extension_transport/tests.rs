use super::*;
use rstest::rstest;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[rstest]
#[tokio::test]
async fn startup_timeout_aborts_and_drains_its_own_task() {
    let dropped = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&dropped);
    let (started, running) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let _guard = Dropped(observed);
        let _ = started.send(());
        std::future::pending::<Result<(), dcc_cua_host::HostError>>().await
    });
    running.await.unwrap();
    let listener = ProviderListener {
        handle,
        owner: "test".into(),
    };
    let (ready, received) = oneshot::channel();
    let error = listener
        .wait_ready(received, Duration::from_millis(5))
        .await
        .err()
        .unwrap();
    assert!(error.contains("timed out"));
    assert!(dropped.load(Ordering::SeqCst));
    drop(ready);
}

#[rstest]
#[tokio::test]
async fn cancelled_startup_aborts_its_owned_server() {
    let dropped = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&dropped);
    let (started, running) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let _guard = Dropped(observed);
        let _ = started.send(());
        std::future::pending::<Result<(), dcc_cua_host::HostError>>().await
    });
    running.await.unwrap();
    let listener = ProviderListener {
        handle,
        owner: "test".into(),
    };
    let (ready, received) = oneshot::channel();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(5),
            listener.wait_ready(received, Duration::from_secs(5))
        )
        .await
        .is_err()
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(ready);
}
