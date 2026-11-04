#[cfg(all(test, windows, feature = "test-support"))]
mod handle_tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };

    async fn pending_once<F: Future>(mut future: std::pin::Pin<&mut F>) {
        poll_fn(|context| match future.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("expected blocked operation"),
        })
        .await;
    }

    #[tokio::test]
    async fn pool_handle_acquisition_keeps_admission_until_reference_commit() {
        let temp = tempfile::tempdir().unwrap();
        let config: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command":"C:\\Python314\\python.exe", "args":[concat!(env!("CARGO_MANIFEST_DIR"), "/tests/natural_mcp_fixture.py"), temp.path().join("fixture.jsonl").display().to_string(), "normal"], "shared":true
        })).unwrap();
        let client = McpClient::connect_natural_test("shared".into(), &config)
            .await
            .unwrap();
        let pool = SharedMcpPool::new(McpConfig::default());
        pool.test_adopt_natural_client("shared".into(), client)
            .await;
        let (release, barrier) = tokio::sync::oneshot::channel();
        let mut acquire = Box::pin(pool.connect_server_handle(42, "shared", &config, async {
            barrier.await.unwrap();
        }));
        pending_once(acquire.as_mut()).await;
        let mut closing = Box::pin(pool.disconnect_all());
        pending_once(closing.as_mut()).await;
        assert!(!pool.cleanup_pending());
        assert!(pool.ref_counts().await.is_empty());
        release.send(()).unwrap();
        let lease = acquire.await.unwrap();
        let handle = lease.handle();
        assert!(!handle.is_closed());
        assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
        pending_once(closing.as_mut()).await;
        lease.commit();
        closing.await.unwrap();
        assert!(handle.is_closed());
        assert!(pool.ref_counts().await.is_empty());
        assert!(pool.session_refs.lock().unwrap().is_empty());
    }
}
