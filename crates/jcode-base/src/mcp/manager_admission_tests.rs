#[cfg(test)]
mod manager_admission_tests {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };

    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                crate::env::set_var("JCODE_HOME", previous);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    #[tokio::test]
    async fn manager_cached_discovery_is_hidden_only_while_cleanup_unconfirmed() {
        let _environment = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let _restore = HomeGuard(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        let configured: McpServerConfig =
            serde_json::from_value(serde_json::json!({"command":"unused","shared":false})).unwrap();
        let mut config = McpConfig::default();
        config.servers.insert("cached".into(), configured.clone());
        let mut cache = super::super::McpSchemaCache::default();
        cache.update(
            "cached",
            &configured,
            vec![McpToolDef {
                name: "visible".into(),
                description: None,
                input_schema: serde_json::json!({"type":"object"}),
            }],
        );
        cache.save();
        let manager = McpManager::with_config(config);
        assert_eq!(manager.searchable_tools().await.len(), 1);
        let clients = manager.owned_clients.write().await;
        let mut closing = Box::pin(manager.disconnect_all());
        pending_once(closing.as_mut()).await;
        drop(closing);
        drop(clients);
        assert!(manager.cleanup_pending());
        assert!(manager.searchable_tools().await.is_empty());
        manager.disconnect_all().await.unwrap();
        assert_eq!(manager.searchable_tools().await.len(), 1);
    }

    async fn pending_once<F: Future>(mut future: std::pin::Pin<&mut F>) {
        poll_fn(|context| match future.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("expected blocked operation"),
        })
        .await;
    }

    #[tokio::test]
    async fn manager_drain_blocks_new_name_until_snapshot_commit() {
        let manager = McpManager::with_config(McpConfig::default());
        let (release, barrier) = tokio::sync::oneshot::channel();
        let mut closing = Box::pin(manager.disconnect_all_with(async {
            barrier.await.unwrap();
        }));
        pending_once(closing.as_mut()).await;
        let configured: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command":"unused", "shared":false, "transport":"http", "url":"http://[invalid"
        }))
        .unwrap();
        let mut opening = Box::pin(manager.connect("new-name", &configured));
        pending_once(opening.as_mut()).await;
        assert!(manager.connecting.lock().unwrap().is_empty());
        assert!(manager.owned_clients.read().await.is_empty());
        release.send(()).unwrap();
        closing.await.unwrap();
        assert!(!manager.cleanup_pending());
        assert!(opening.await.is_err());
        assert!(manager.owned_clients.read().await.is_empty());
    }

    #[tokio::test]
    async fn manager_cancelled_drain_blocks_connect_and_catalog_until_retry() {
        let manager = McpManager::with_config(McpConfig::default());
        let clients = manager.owned_clients.write().await;
        let mut closing = Box::pin(manager.disconnect_all());
        pending_once(closing.as_mut()).await;
        assert!(manager.cleanup_pending());
        drop(closing);
        drop(clients);
        assert!(
            manager
                .connect_all()
                .await
                .unwrap_err()
                .to_string()
                .contains("cleanup")
        );
        assert!(manager.connected_servers().await.is_empty());
        assert!(manager.all_tools().await.is_empty());
        assert!(manager.server_tools("blocked").await.is_none());
        assert!(manager.searchable_tools().await.is_empty());
        assert!(
            manager
                .live_handle("blocked")
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("cleanup")
        );
        manager.disconnect_all().await.unwrap();
        assert_eq!(manager.connect_all().await.unwrap(), (0, vec![]));
        assert!(!manager.cleanup_pending());
    }

    #[test]
    fn owned_attempt_drop_without_runtime_preserves_new_generation() {
        let registry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (old, old_waiter) = watch::channel(None);
        let (new, new_waiter) = watch::channel(None);
        registry.lock().unwrap().insert("same".into(), new.clone());
        drop(OwnedConnectionAttemptGuard::new(
            Arc::clone(&registry),
            "same",
            old,
        ));
        assert!(
            registry
                .lock()
                .unwrap()
                .get("same")
                .unwrap()
                .same_channel(&new)
        );
        assert!(old_waiter.borrow().as_ref().unwrap().is_err());
        assert!(new_waiter.borrow().is_none());
    }

    #[test]
    fn owned_attempt_drop_after_runtime_teardown_notifies_waiter() {
        let registry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (sender, waiter) = watch::channel(None);
        registry
            .lock()
            .unwrap()
            .insert("teardown".into(), sender.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let guard = runtime.block_on(async {
            OwnedConnectionAttemptGuard::new(Arc::clone(&registry), "teardown", sender)
        });
        drop(runtime);
        drop(guard);
        assert!(registry.lock().unwrap().is_empty());
        assert!(waiter.borrow().as_ref().unwrap().is_err());
    }

    #[test]
    fn owned_attempt_drop_contended_registry_needs_no_runtime() {
        let registry = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (sender, waiter) = watch::channel(None);
        registry
            .lock()
            .unwrap()
            .insert("contended".into(), sender.clone());
        let guard = OwnedConnectionAttemptGuard::new(Arc::clone(&registry), "contended", sender);
        let held = registry.lock().unwrap();
        let (started, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            drop(guard);
        });
        receiver.recv().unwrap();
        assert!(waiter.borrow().is_none());
        drop(held);
        worker.join().unwrap();
        assert!(registry.lock().unwrap().is_empty());
        assert!(waiter.borrow().as_ref().unwrap().is_err());
    }

    include!("manager_reference_tests.rs");

    #[tokio::test]
    async fn manager_pool_reload_failure_keeps_old_config_and_requires_retry() {
        let original: McpConfig = serde_json::from_value(
            serde_json::json!({"mcpServers":{"old":{"command":"unused","enabled":false}}}),
        )
        .unwrap();
        let mut manager = McpManager::with_config(original);
        let error = manager
            .reload_using(
                McpConfig::default,
                std::future::ready(Err(anyhow::anyhow!("pool drain failed"))),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("pool drain failed"));
        assert!(manager.config().servers.contains_key("old"));
        assert!(manager.cleanup_pending());
        manager.disconnect_all().await.unwrap();
        assert!(!manager.cleanup_pending());
    }

    #[tokio::test]
    async fn manager_reload_reports_pool_only_connection_failures() {
        let mut manager = McpManager::with_config(McpConfig::default());
        let result = manager
            .reload_using(
                McpConfig::default,
                std::future::ready(Ok((
                    0,
                    vec![("pool-only".into(), "synthetic handshake failure".into())],
                ))),
            )
            .await
            .unwrap();
        assert_eq!(
            result,
            (
                0,
                vec![("pool-only".into(), "synthetic handshake failure".into())]
            )
        );
        assert!(!manager.cleanup_pending());
    }

    #[tokio::test]
    async fn manager_cancelled_pool_reload_keeps_old_config_and_closes_admission() {
        let original: McpConfig = serde_json::from_value(
            serde_json::json!({"mcpServers":{"old":{"command":"unused","enabled":false}}}),
        )
        .unwrap();
        let mut manager = McpManager::with_config(original);
        let mut reload = Box::pin(manager.reload_using(McpConfig::default, std::future::pending()));
        pending_once(reload.as_mut()).await;
        drop(reload);
        assert!(manager.config().servers.contains_key("old"));
        assert!(manager.cleanup_pending());
        assert!(manager.connect_all().await.is_err());
        manager.disconnect_all().await.unwrap();
    }

    #[cfg(all(windows, feature = "test-support"))]
    #[tokio::test]
    async fn admitted_tool_call_does_not_hold_drain_and_reports_transport_close() {
        let (pool, config, temp) = natural_pool("pending_until_shutdown").await;
        let manager = McpManager::with_shared_pool(Arc::clone(&pool), "calling".into());
        manager.connect("shared", &config).await.unwrap();
        let mut calling = Box::pin(manager.call_tool("shared", "pending", serde_json::json!({})));
        let observed = async {
            loop {
                if std::fs::read_to_string(temp.path().join("fixture.jsonl"))
                    .unwrap_or_default()
                    .contains("pending_tool_request_received")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::select! {
                result = &mut calling => panic!("pending tool returned before drain: {result:?}"),
                _ = observed => {},
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), pool.disconnect_all())
            .await
            .unwrap()
            .unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), calling)
            .await
            .unwrap()
            .unwrap_err();
        assert!(format!("{error:#}").contains("connection closed"));
        assert!(manager.live_handle("shared").await.unwrap().is_none());
        manager.disconnect_all().await.unwrap();
    }
}
