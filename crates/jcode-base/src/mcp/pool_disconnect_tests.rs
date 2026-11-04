#[cfg(test)]
mod disconnect_tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    async fn pending_once<F: Future>(future: std::pin::Pin<&mut F>) {
        let mut future = future;
        poll_fn(|context| match future.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("expected pending barrier"),
        })
        .await;
    }
    #[tokio::test]
    async fn cancelled_absent_owner_disconnect_reserves_name_before_map_wait() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let clients = pool.clients.lock().await;
        let mut disconnect = Box::pin(pool.disconnect_server("gap"));
        pending_once(disconnect.as_mut()).await;
        let ConnectAttempt::Wait(operation) = pool.begin_connect("gap").await else {
            panic!("connect must wait behind disconnect reservation")
        };
        assert!(operation.operation == SharedOperation::Disconnect);
        assert!(operation.result().is_none());
        drop(disconnect);
        drop(clients);
        assert!(operation.result().unwrap().is_err());
        assert!(!pool.lock_connecting().contains_key("gap"));
        assert!(matches!(
            pool.begin_connect("gap").await,
            ConnectAttempt::Leader(_)
        ));
    }
    #[tokio::test]
    async fn absent_owner_disconnect_blocks_publication_until_atomic_commit() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let handles = pool.handles.write().await;
        let mut disconnect = Box::pin(pool.disconnect_server("gap"));
        pending_once(disconnect.as_mut()).await;
        let ConnectAttempt::Wait(operation) = pool.begin_connect("gap").await else {
            panic!("connect must wait during commit")
        };
        assert!(operation.result().is_none());
        drop(handles);
        disconnect.await.unwrap();
        assert!(operation.result().unwrap().is_ok());
        assert!(matches!(
            pool.begin_connect("gap").await,
            ConnectAttempt::Leader(_)
        ));
    }
    #[tokio::test]
    async fn stale_disconnect_cannot_clear_new_generation_metadata() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let old = Arc::new(SharedConnectionAttempt::new(SharedOperation::Disconnect));
        let fresh = Arc::new(SharedConnectionAttempt::new(SharedOperation::Connect));
        pool.lock_connecting()
            .insert("race".into(), Arc::clone(&fresh));
        pool.ref_counts.lock().await.insert("race".into(), 7);
        pool.last_errors.write().await.insert(
            "race".into(),
            FailedConnectRecord {
                message: "new generation evidence".into(),
                failed_at: Instant::now(),
            },
        );
        let error = pool
            .commit_disconnect("race", &old, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("superseded"));
        assert_eq!(pool.ref_counts.lock().await.get("race"), Some(&7));
        assert_eq!(
            pool.last_errors.read().await.get("race").unwrap().message,
            "new generation evidence"
        );
        assert!(Arc::ptr_eq(
            pool.lock_connecting().get("race").unwrap(),
            &fresh
        ));
        assert!(fresh.result().is_none());
    }
    #[tokio::test]
    async fn connect_waiter_retries_after_disconnect_instead_of_reporting_connected() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("never-created-mcp.exe");
        assert!(!missing.exists());
        let configured: McpServerConfig = serde_json::from_value(
            serde_json::json!({"command":missing.display().to_string(),"shared":false}),
        )
        .unwrap();
        let pool = SharedMcpPool::new(McpConfig::default());
        let old = Arc::new(SharedConnectionAttempt::new(SharedOperation::Disconnect));
        pool.lock_connecting()
            .insert("waiter".into(), Arc::clone(&old));
        let mut connection = Box::pin(pool.connect_server("waiter", &configured));
        pending_once(connection.as_mut()).await;
        assert!(
            Arc::strong_count(&old) >= 3,
            "public connect must have registered as a waiter on this exact generation"
        );
        assert!(old.result().is_none());
        pool.commit_disconnect("waiter", &old, None).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), connection)
            .await
            .unwrap()
            .unwrap_err();
        assert!(format!("{error:#}").contains("never-created-mcp.exe"));
        assert!(old.result().unwrap().is_ok());
        assert!(pool.clients.lock().await.is_empty());
        assert!(pool.handles.read().await.is_empty());
    }
    #[cfg(all(windows, feature = "test-support"))]
    #[tokio::test]
    async fn owner_mismatch_disconnect_preserves_current_client_handle_refs_and_errors() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("owner-mismatch.jsonl");
        let configured:McpServerConfig=serde_json::from_value(serde_json::json!({"command":"C:\\Python314\\python.exe","args":[concat!(env!("CARGO_MANIFEST_DIR"),"/tests/natural_mcp_fixture.py"),record.display().to_string(),"normal"],"shared":false})).unwrap();
        let client = McpClient::connect_natural_test("owner".into(), &configured)
            .await
            .unwrap();
        let pool = SharedMcpPool::new(McpConfig::default());
        let old = Arc::new(SharedConnectionAttempt::new(SharedOperation::Disconnect));
        pool.lock_connecting()
            .insert("owner".into(), Arc::clone(&old));
        pool.test_adopt_natural_client("owner".into(), client).await;
        let current = pool.clients.lock().await.get("owner").cloned().unwrap();
        pool.ref_counts.lock().await.insert("owner".into(), 7);
        pool.last_errors.write().await.insert(
            "owner".into(),
            FailedConnectRecord {
                message: "current evidence".into(),
                failed_at: Instant::now(),
            },
        );
        let error = pool
            .commit_disconnect("owner", &old, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("owner was superseded"));
        assert!(Arc::ptr_eq(
            pool.clients.lock().await.get("owner").unwrap(),
            &current
        ));
        assert!(!pool.get_handle("owner").await.unwrap().is_closed());
        assert_eq!(pool.ref_counts.lock().await.get("owner"), Some(&7));
        assert_eq!(
            pool.last_errors.read().await.get("owner").unwrap().message,
            "current evidence"
        );
        pool.disconnect_server("owner").await.unwrap();
        assert!(
            std::fs::read_to_string(record)
                .unwrap()
                .contains("natural_exit")
        );
    }
}
