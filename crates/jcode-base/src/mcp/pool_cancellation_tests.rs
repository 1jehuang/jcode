#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    async fn pending_once<F: Future>(future: std::pin::Pin<&mut F>) {
        let mut future = future;
        poll_fn(|context| match future.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("expected pending cancellation boundary"),
        })
        .await;
    }
    #[tokio::test]
    async fn cancelled_shared_leader_wakes_waiter_and_allows_retry() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let ConnectAttempt::Leader(attempt) = pool.begin_connect("cancel").await else {
            panic!("leader")
        };
        let mut connection = Box::pin(pool.connect_as_leader(
            "cancel",
            Arc::clone(&attempt),
            std::future::pending::<Result<McpClient>>(),
        ));
        pending_once(connection.as_mut()).await;
        let mut waiter = Box::pin(pool.wait_for_connect("cancel", &attempt));
        pending_once(waiter.as_mut()).await;
        drop(connection);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap();
        assert!(
            attempt
                .result()
                .unwrap()
                .unwrap_err()
                .contains("cancelled; retry connection")
        );
        assert!(!pool.lock_connecting().contains_key("cancel"));
        assert!(matches!(
            pool.begin_connect("cancel").await,
            ConnectAttempt::Leader(_)
        ));
        assert!(pool.clients.lock().await.is_empty());
        assert!(pool.handles.read().await.is_empty());
    }
    #[tokio::test]
    async fn stale_cancel_without_runtime_preserves_new_generation_and_old_waiter_result() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let ConnectAttempt::Leader(old) = pool.begin_connect("race").await else {
            panic!("leader")
        };
        let guard = SharedConnectionAttemptGuard::new(
            Arc::clone(&pool.connecting),
            "race",
            Arc::clone(&old),
        );
        let mut waiter = Box::pin(pool.wait_for_connect("race", &old));
        pending_once(waiter.as_mut()).await;
        let mut registry = pool.lock_connecting();
        let (started, ready) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            assert!(tokio::runtime::Handle::try_current().is_err());
            started.send(()).unwrap();
            drop(guard);
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let fresh = Arc::new(SharedConnectionAttempt::new(SharedOperation::Connect));
        registry.insert("race".into(), Arc::clone(&fresh));
        drop(registry);
        worker.join().unwrap();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap();
        assert!(
            old.result()
                .unwrap()
                .unwrap_err()
                .contains("cancelled; retry connection")
        );
        assert!(fresh.result().is_none());
        assert!(Arc::ptr_eq(
            pool.lock_connecting().get("race").unwrap(),
            &fresh
        ));
        let error = pool
            .finish_connect("race", &old, Err(anyhow::anyhow!("stale failure")))
            .await
            .unwrap_err();
        assert!(error.contains("superseded"));
        assert!(pool.last_errors.read().await.is_empty());
        assert!(
            old.result()
                .unwrap()
                .unwrap_err()
                .contains("cancelled; retry connection")
        );
        assert!(Arc::ptr_eq(
            pool.lock_connecting().get("race").unwrap(),
            &fresh
        ));
    }
    #[tokio::test]
    async fn cancelled_shared_publication_before_mutation_leaves_no_partial_state() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let ConnectAttempt::Leader(attempt) = pool.begin_connect("publish").await else {
            panic!("leader")
        };
        let errors = pool.last_errors.write().await;
        let mut connection = Box::pin(pool.connect_as_leader(
            "publish",
            Arc::clone(&attempt),
            std::future::ready(Err(anyhow::anyhow!("fixture failure"))),
        ));
        pending_once(connection.as_mut()).await;
        drop(connection);
        drop(errors);
        assert!(!pool.lock_connecting().contains_key("publish"));
        assert!(pool.clients.lock().await.is_empty());
        assert!(pool.handles.read().await.is_empty());
        assert!(pool.last_errors.read().await.is_empty());
        assert!(matches!(
            pool.begin_connect("publish").await,
            ConnectAttempt::Leader(_)
        ));
    }
    #[test]
    fn shared_guard_drop_requires_no_runtime() {
        assert!(tokio::runtime::Handle::try_current().is_err());
        let registry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let attempt = Arc::new(SharedConnectionAttempt::new(SharedOperation::Connect));
        registry
            .lock()
            .unwrap()
            .insert("offline".into(), Arc::clone(&attempt));
        drop(SharedConnectionAttemptGuard::new(
            Arc::clone(&registry),
            "offline",
            Arc::clone(&attempt),
        ));
        assert!(registry.lock().unwrap().is_empty());
        assert!(attempt.result().unwrap().is_err());
    }
    #[test]
    fn shared_guard_drop_retires_after_runtime_teardown() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let pool = SharedMcpPool::new(McpConfig::default());
        let ConnectAttempt::Leader(attempt) = runtime.block_on(pool.begin_connect("teardown"))
        else {
            panic!("leader")
        };
        let guard = SharedConnectionAttemptGuard::new(
            Arc::clone(&pool.connecting),
            "teardown",
            Arc::clone(&attempt),
        );
        runtime.shutdown_background();
        drop(guard);
        assert!(!pool.lock_connecting().contains_key("teardown"));
        assert!(attempt.result().unwrap().is_err());
    }
    #[cfg(all(windows, feature = "test-support"))]
    #[tokio::test]
    async fn slow_owned_shutdown_does_not_block_unrelated_name() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("slow-independent.jsonl");
        let config = McpServerConfig {
            command: "C:\\Python314\\python.exe".into(),
            args: vec![
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/natural_mcp_fixture.py").into(),
                record.display().to_string(),
                "slow_shutdown".into(),
            ],
            env: HashMap::new(),
            shared: false,
            transport: None,
            url: None,
            headers: HashMap::new(),
            enabled: None,
            disabled: None,
            timeout_secs: Some(3),
        };
        let client = McpClient::connect_natural_test("slow".into(), &config)
            .await
            .unwrap();
        let pool = SharedMcpPool::new(McpConfig::default());
        pool.test_adopt_natural_client("slow".into(), client).await;
        let owner = pool.clients.lock().await.get("slow").cloned().unwrap();
        let mut shutdown = Box::pin(pool.disconnect_server("slow"));
        pending_once(shutdown.as_mut()).await;
        assert!(
            owner.try_lock().is_err(),
            "slow shutdown must hold its own client lock"
        );
        let ConnectAttempt::Leader(other) =
            tokio::time::timeout(Duration::from_millis(200), pool.begin_connect("unrelated"))
                .await
                .expect("unrelated reservation must proceed")
        else {
            panic!("unrelated must lead")
        };
        pool.complete_attempt("unrelated", &other, Err("test complete".into()));
        drop(shutdown);
        assert!(owner.lock().await.cleanup_pending());
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if std::fs::read_to_string(&record)
                    .unwrap_or_default()
                    .contains("natural_exit")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        pool.disconnect_server("slow").await.unwrap();
        assert!(!pool.clients.lock().await.contains_key("slow"));
    }
}
