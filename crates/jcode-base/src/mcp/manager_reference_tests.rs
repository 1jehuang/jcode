#[cfg(all(windows, feature = "test-support"))]
async fn natural_pool(mode: &str) -> (Arc<SharedMcpPool>, McpServerConfig, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command":"C:\\Python314\\python.exe", "args":[concat!(env!("CARGO_MANIFEST_DIR"), "/tests/natural_mcp_fixture.py"), temp.path().join("fixture.jsonl").display().to_string(), mode], "shared":true
        })).unwrap();
    let client = McpClient::connect_natural_test("shared".into(), &config)
        .await
        .unwrap();
    let pool = Arc::new(SharedMcpPool::new(McpConfig::default()));
    pool.test_adopt_natural_client("shared".into(), client)
        .await;
    (pool, config, temp)
}

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn shared_manager_refs_are_idempotent_and_cancel_before_publish_is_atomic() {
    let (pool, config, _temp) = natural_pool("normal").await;
    let first = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    let second = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    first.connect("shared", &config).await.unwrap();
    first.connect("shared", &config).await.unwrap();
    second.connect("shared", &config).await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    assert_eq!(pool.acquire_handles("same-session").await.len(), 1);
    assert_eq!(pool.acquire_handles("same-session").await.len(), 1);
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&3));
    pool.release_handles("unrelated", &["shared".into()]).await;
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&3));
    pool.release_handles("same-session", &["shared".into()])
        .await;
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    let cancelled = McpManager::with_shared_pool(Arc::clone(&pool), "cancelled".into());
    let refs = pool.test_lock_ref_counts().await;
    let mut opening = Box::pin(cancelled.connect("shared", &config));
    pending_once(opening.as_mut()).await;
    assert!(cancelled.pool_handles.try_write().is_ok());
    drop(opening);
    assert!(cancelled.pool_handles.read().await.is_empty());
    assert!(!pool.test_manager_reference(cancelled.reference_id));
    drop(refs);
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    let refs = pool.test_lock_ref_counts().await;
    let mut disconnecting = Box::pin(first.disconnect("shared"));
    pending_once(disconnecting.as_mut()).await;
    drop(disconnecting);
    assert!(first.pool_handles.read().await.contains_key("shared"));
    drop(refs);
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    first.disconnect("shared").await.unwrap();
    first.disconnect("shared").await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    assert!(second.live_handle("shared").await.unwrap().is_some());
    second.disconnect_all().await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&0));
    pool.disconnect_all().await.unwrap();
}

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn stale_manager_release_cannot_remove_same_session_new_generation_reference() {
    let (pool, config, _temp) = natural_pool("normal").await;
    let first = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    let second = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    first.connect("shared", &config).await.unwrap();
    second.connect("shared", &config).await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    pool.disconnect_all().await.unwrap();
    assert!(!second.has_connections().await);
    let profile = second.debug_memory_profile();
    assert_eq!(profile.pooled_handles, 1);
    assert_eq!(profile.connected_servers, 0);
    assert_eq!(profile.available_tools, 0);
    let client = McpClient::connect_natural_test("shared".into(), &config)
        .await
        .unwrap();
    pool.test_adopt_natural_client("shared".into(), client)
        .await;
    let current = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    current.connect("shared", &config).await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    first.disconnect_all().await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    assert!(current.has_connections().await);
    second.connect("shared", &config).await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    current.disconnect_all().await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    second.disconnect_all().await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&0));
    pool.disconnect_all().await.unwrap();
}

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn manager_drop_without_runtime_releases_claim_even_when_reference_lock_is_busy() {
    let (pool, config, _temp) = natural_pool("normal").await;
    let immediate = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    let deferred = McpManager::with_shared_pool(Arc::clone(&pool), "same-session".into());
    immediate.connect("shared", &config).await.unwrap();
    deferred.connect("shared", &config).await.unwrap();
    let immediate_id = immediate.reference_id;
    let deferred_id = deferred.reference_id;
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&2));
    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        drop(immediate);
    })
    .join()
    .unwrap();
    assert!(!pool.test_manager_reference(immediate_id));
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    let refs = pool.test_lock_ref_counts().await;
    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        drop(deferred);
    })
    .join()
    .unwrap();
    assert_eq!(refs.get("shared"), Some(&1));
    assert!(pool.test_manager_reference(deferred_id));
    drop(refs);
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&0));
    assert!(!pool.test_manager_reference(deferred_id));
    assert!(pool.get_handle("shared").await.is_some());
    let final_manager = McpManager::with_shared_pool(Arc::clone(&pool), "final".into());
    final_manager.connect("shared", &config).await.unwrap();
    let retained = pool.get_handle("shared").await.unwrap();
    let final_id = final_manager.reference_id;
    let refs = pool.test_lock_ref_counts().await;
    std::thread::spawn(move || drop(final_manager))
        .join()
        .unwrap();
    assert!(pool.test_manager_reference(final_id));
    assert_eq!(pool.test_pending_drop_releases(), 1);
    drop(refs);
    pool.disconnect_all().await.unwrap();
    assert_eq!(pool.test_pending_drop_releases(), 0);
    assert!(retained.is_closed());
    assert!(!pool.test_manager_reference(final_id));
    assert!(pool.ref_counts().await.is_empty());
}

#[tokio::test]
async fn empty_pool_drain_flushes_deferred_manager_drop_before_reference_query() {
    let pool = Arc::new(SharedMcpPool::new(McpConfig::default()));
    let manager = McpManager::with_shared_pool(Arc::clone(&pool), "empty".into());
    let refs = pool.test_lock_ref_counts().await;
    drop(manager);
    assert_eq!(pool.test_pending_drop_releases(), 1);
    drop(refs);
    pool.disconnect_all().await.unwrap();
    assert_eq!(pool.test_pending_drop_releases(), 0);
}

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn held_shared_connection_does_not_block_another_name_publication() {
    let (pool, config, temp) = natural_pool("normal").await;
    let mut held_config = config.clone();
    held_config.args[1] = temp.path().join("held.jsonl").display().to_string();
    let client = McpClient::connect_natural_test("held".into(), &held_config)
        .await
        .unwrap();
    pool.test_adopt_natural_client("held".into(), client).await;
    let manager = McpManager::with_shared_pool(Arc::clone(&pool), "parallel".into());
    let (release, barrier) = tokio::sync::oneshot::channel();
    let mut held = Box::pin(async {
        let _admission = manager.admit().await?;
        manager
            .connect_shared_using("held", &held_config, async {
                barrier.await.unwrap();
            })
            .await
    });
    pending_once(held.as_mut()).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        manager.connect("shared", &config),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(manager.pool_handles.read().await.contains_key("shared"));
    assert!(!manager.pool_handles.read().await.contains_key("held"));
    release.send(()).unwrap();
    held.await.unwrap();
    assert!(manager.pool_handles.read().await.contains_key("held"));
    manager.disconnect_all().await.unwrap();
    pool.disconnect_all().await.unwrap();
}

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn cancelled_shared_publication_rolls_back_only_its_new_reference() {
    let (pool, config, _temp) = natural_pool("normal").await;
    let manager = McpManager::with_shared_pool(Arc::clone(&pool), "publish".into());
    let handles = manager.pool_handles.write().await;
    let mut first = Box::pin(manager.connect("shared", &config));
    pending_once(first.as_mut()).await;
    assert!(pool.test_manager_reference(manager.reference_id));
    let refs = pool.test_lock_ref_counts().await;
    let mut waiter = Box::pin(manager.connect("shared", &config));
    pending_once(waiter.as_mut()).await;
    drop(first);
    assert_eq!(pool.test_pending_drop_releases(), 1);
    pending_once(waiter.as_mut()).await;
    drop(refs);
    pending_once(waiter.as_mut()).await;
    assert_eq!(pool.test_pending_drop_releases(), 0);
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    drop(waiter);
    assert!(!pool.test_manager_reference(manager.reference_id));
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&0));
    drop(handles);
    manager.connect("shared", &config).await.unwrap();
    let handles = manager.pool_handles.write().await;
    let mut repeated = Box::pin(manager.connect("shared", &config));
    pending_once(repeated.as_mut()).await;
    drop(repeated);
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&1));
    drop(handles);
    manager.disconnect_all().await.unwrap();
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&0));
    pool.disconnect_all().await.unwrap();
}

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn pool_drain_waits_for_shared_publication_or_cancelled_lease() {
    let (pool, config, _temp) = natural_pool("normal").await;
    let manager = McpManager::with_shared_pool(Arc::clone(&pool), "drain-publish".into());
    let handles = manager.pool_handles.write().await;
    let mut opening = Box::pin(manager.connect("shared", &config));
    pending_once(opening.as_mut()).await;
    assert!(pool.test_manager_reference(manager.reference_id));
    let mut closing = Box::pin(pool.disconnect_all());
    pending_once(closing.as_mut()).await;
    assert!(!pool.cleanup_pending());
    drop(opening);
    assert!(!pool.test_manager_reference(manager.reference_id));
    assert_eq!(pool.ref_counts().await.get("shared"), Some(&0));
    drop(handles);
    tokio::time::timeout(std::time::Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap();
    assert!(manager.pool_handles.read().await.is_empty());
    assert!(pool.ref_counts().await.is_empty());
    manager.disconnect_all().await.unwrap();
}
