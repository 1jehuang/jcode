#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    async fn pending_once<F: Future>(future: std::pin::Pin<&mut F>) {
        let mut future = future;
        poll_fn(|context| match future.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("expected admission barrier"),
        })
        .await;
    }
    #[tokio::test]
    async fn global_close_prevents_new_name_admission_between_snapshot_and_final_check() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let (release, barrier) = tokio::sync::oneshot::channel();
        let mut closing = Box::pin(async {
            let _admission = pool.admission.write().await;
            pool.cleanup_pending
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pool.disconnect_all_inner(async {
                barrier.await.unwrap();
            })
            .await
            .unwrap();
            pool.cleanup_pending
                .store(false, std::sync::atomic::Ordering::SeqCst);
        });
        pending_once(closing.as_mut()).await;
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("never-created-admission.exe");
        assert!(!missing.exists());
        let configured: McpServerConfig = serde_json::from_value(
            serde_json::json!({"command":missing.display().to_string(),"shared":false}),
        )
        .unwrap();
        let mut opening = Box::pin(pool.connect_server("new-name", &configured));
        pending_once(opening.as_mut()).await;
        assert!(!pool.lock_connecting().contains_key("new-name"));
        assert!(pool.clients.lock().await.is_empty());
        release.send(()).unwrap();
        closing.await;
        assert!(pool.clients.lock().await.is_empty());
        assert!(!pool.cleanup_pending());
        let error = opening.await.unwrap_err();
        assert!(format!("{error:#}").contains("never-created-admission.exe"));
    }
    #[tokio::test]
    async fn cancelled_global_close_stays_fail_closed_until_successful_retry() {
        let pool = SharedMcpPool::new(McpConfig::default());
        let clients = pool.clients.lock().await;
        let mut closing = Box::pin(pool.disconnect_all());
        pending_once(closing.as_mut()).await;
        assert!(pool.cleanup_pending());
        drop(closing);
        drop(clients);
        assert!(pool.cleanup_pending());
        let configured: McpServerConfig =
            serde_json::from_value(serde_json::json!({"command":"not-admitted","shared":false}))
                .unwrap();
        let error = pool
            .connect_server("blocked", &configured)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("pool cleanup is unconfirmed"));
        assert!(pool.lock_connecting().is_empty());
        pool.disconnect_all().await.unwrap();
        assert!(!pool.cleanup_pending());
        assert!(pool.admission.try_read().is_ok());
    }
    #[tokio::test]
    async fn cancelled_reload_before_config_swap_preserves_config_and_blocks_admission() {
        let configured: McpConfig = serde_json::from_value(
            serde_json::json!({"mcpServers":{"original":{"command":"original","shared":false}}}),
        )
        .unwrap();
        let pool = SharedMcpPool::new(configured);
        let clients = pool.clients.lock().await;
        let mut reload = Box::pin(pool.reload());
        pending_once(reload.as_mut()).await;
        drop(reload);
        drop(clients);
        assert!(pool.cleanup_pending());
        assert!(pool.config().await.servers.contains_key("original"));
        pool.disconnect_all().await.unwrap();
        assert!(!pool.cleanup_pending());
        assert!(pool.config().await.servers.contains_key("original"));
    }
    #[cfg(all(windows, feature = "test-support"))]
    #[tokio::test]
    async fn failed_reload_keeps_config_and_admission_closed_until_drain_retry() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("reload-failure.jsonl");
        let configured:McpServerConfig=serde_json::from_value(serde_json::json!({"command":"C:\\Python314\\python.exe","args":[concat!(env!("CARGO_MANIFEST_DIR"),"/tests/natural_mcp_fixture.py"),record.display().to_string(),"slow_shutdown"],"shared":false})).unwrap();
        let client = McpClient::connect_natural_test("slow".into(), &configured)
            .await
            .unwrap();
        let original: McpConfig = serde_json::from_value(
            serde_json::json!({"mcpServers":{"original":{"command":"original","shared":false}}}),
        )
        .unwrap();
        let pool = SharedMcpPool::new(original);
        pool.test_adopt_natural_client("slow".into(), client).await;
        let error = pool.reload().await.unwrap_err();
        assert!(format!("{error:#}").contains("shutdown incomplete"));
        assert!(pool.cleanup_pending());
        assert!(pool.config().await.servers.contains_key("original"));
        assert!(pool.get_handle("slow").await.is_none());
        assert!(pool.acquire_handles("new-session").await.is_empty());
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
        pool.disconnect_all().await.unwrap();
        assert!(!pool.cleanup_pending());
        assert!(pool.clients.lock().await.is_empty());
        assert!(pool.config().await.servers.contains_key("original"));
    }
    #[tokio::test]
    async fn cancelled_reload_waiting_for_admission_leaves_existing_state_operational() {
        let original: McpConfig = serde_json::from_value(
            serde_json::json!({"mcpServers":{"original":{"command":"original","shared":false}}}),
        )
        .unwrap();
        let pool = SharedMcpPool::new(original);
        let reader = pool.admission.read().await;
        let mut reload = Box::pin(pool.reload_with(McpConfig::default, std::future::ready(())));
        pending_once(reload.as_mut()).await;
        assert!(!pool.cleanup_pending());
        drop(reload);
        drop(reader);
        assert!(!pool.cleanup_pending());
        assert!(pool.config().await.servers.contains_key("original"));
        assert!(pool.admission.try_read().is_ok());
    }
    #[cfg(all(windows, feature = "test-support"))]
    #[tokio::test]
    async fn cancelled_real_reconnect_retains_published_owner_and_retry_drains_it() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("partial-reload.jsonl");
        let held_record = temp.path().join("held-reload.jsonl");
        let original: McpConfig = serde_json::from_value(
            serde_json::json!({"mcpServers":{"original":{"command":"original","shared":false}}}),
        )
        .unwrap();
        let replacement:McpConfig=serde_json::from_value(serde_json::json!({"mcpServers":{
            "partial":{"command":"C:\\Python314\\python.exe","args":[concat!(env!("CARGO_MANIFEST_DIR"),"/tests/natural_mcp_fixture.py"),record.display().to_string(),"pending_until_shutdown"],"shared":true},
            "held":{"command":"C:\\Python314\\python.exe","args":[concat!(env!("CARGO_MANIFEST_DIR"),"/tests/natural_mcp_fixture.py"),held_record.display().to_string(),"hold_init"],"shared":true}
        }})).unwrap();
        let pool = SharedMcpPool::new(original);
        let connector = |name: String, configured: McpServerConfig| async move {
            McpClient::connect_natural_test(name, &configured).await
        };
        let mut reload =
            Box::pin(pool.reload_using(|| replacement, std::future::ready(()), connector));
        tokio::select! {
            result=&mut reload=>panic!("real concurrent reconnect finished before cancellation: {result:?}"),
            observed=tokio::time::timeout(Duration::from_secs(10),async{loop{if std::fs::read_to_string(&held_record).unwrap_or_default().contains("initialization_held")&&pool.clients.lock().await.contains_key("partial"){break;}tokio::task::yield_now().await;}})=>{observed.expect("real partial owner must publish before cancellation");}
        }
        let retained = pool.handles.read().await.get("partial").cloned().unwrap();
        assert!(pool.lock_connecting().contains_key("held"));
        assert!(
            std::fs::read_to_string(&held_record)
                .unwrap()
                .contains("initialization_held")
        );
        drop(reload);
        assert!(!pool.lock_connecting().contains_key("held"));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::read_to_string(&held_record)
                    .unwrap_or_default()
                    .contains("natural_exit")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled actual initialization must release fixture stdin and exit naturally");
        println!(
            "HELD_INIT_CANCEL {}",
            std::fs::read_to_string(&held_record).unwrap()
        );
        assert!(pool.cleanup_pending());
        assert!(pool.config().await.servers.contains_key("partial"));
        assert!(!pool.config().await.servers.contains_key("original"));
        assert!(pool.get_handle("partial").await.is_none());
        assert!(
            retained
                .request("fixture/echo", Some(serde_json::json!({"retained":true})))
                .await
                .is_ok(),
            "admission does not revoke an already-distributed live handle"
        );
        let mut pending = Box::pin(retained.request("fixture/pending", None));
        pending_once(pending.as_mut()).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if std::fs::read_to_string(&record)
                    .unwrap_or_default()
                    .contains("pending_request_received")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        pool.reload_with(McpConfig::default, std::future::ready(()))
            .await
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap_err();
        assert!(format!("{error:#}").contains("connection closed"));
        assert!(retained.is_closed());
        assert!(retained.request("fixture/echo", None).await.is_err());
        assert!(!pool.cleanup_pending());
        assert!(pool.clients.lock().await.is_empty());
        assert!(pool.config().await.servers.is_empty());
        assert!(
            std::fs::read_to_string(record)
                .unwrap()
                .contains("natural_exit")
        );
    }
}
