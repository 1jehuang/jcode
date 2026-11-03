use super::*;
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestHome {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl TestHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        Self {
            _dir: dir,
            previous,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

struct NoRequests;

#[async_trait]
impl Provider for NoRequests {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("ownership tests must never call a provider")
    }
    fn name(&self) -> &str {
        "ownership-test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

async fn agent(session_id: &str) -> AgentHandle {
    let provider: Arc<dyn Provider> = Arc::new(NoRequests);
    let registry = crate::tool::Registry::new(Arc::clone(&provider)).await;
    let session = crate::session::Session::create_with_id(session_id.to_owned(), None, None);
    Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )))
}

#[tokio::test]
async fn concurrent_get_or_start_runs_only_one_factory() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    let calls = AtomicUsize::new(0);
    let (first, second) = tokio::join!(
        manager.get_or_start("same", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(agent("same").await)
        }),
        manager.get_or_start("same", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(agent("same").await)
        }),
    );
    assert!(Arc::ptr_eq(&first?, &second?));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn failed_or_cancelled_factory_releases_reservation() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = Arc::new(SessionManager::new());
    assert!(
        manager
            .get_or_start("target", || async { anyhow::bail!("failed restore") })
            .await
            .is_err()
    );
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let task_manager = Arc::clone(&manager);
    let task = tokio::spawn(async move {
        task_manager
            .get_or_start("target", || async {
                let _ = entered_tx.send(());
                std::future::pending::<Result<AgentHandle>>().await
            })
            .await
    });
    entered_rx.await?;
    task.abort();
    match task.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("aborted factory unexpectedly completed"),
    }
    let owner = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        manager.get_or_start("target", || async { Ok(agent("target").await) }),
    )
    .await??;
    assert!(Arc::ptr_eq(
        &owner,
        manager.read().await.get("target").unwrap()
    ));
    Ok(())
}

#[tokio::test]
async fn publication_never_overwrites_even_after_removal() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    let original = agent("target").await;
    let replacement = agent("target").await;
    manager
        .register_new("target".into(), Arc::clone(&original))
        .await?;
    assert!(
        manager
            .register_new("target".into(), Arc::clone(&replacement))
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(
        manager.read().await.get("target").unwrap(),
        &original
    ));
    let removed = manager.remove("target").await.unwrap();
    assert!(manager.reserve("target").await.ensure_vacant().is_err());
    assert!(
        manager
            .register_new("target".into(), Arc::clone(&replacement))
            .await
            .is_err()
    );
    // A retiring owner must block restoration before its factory can persist.
    assert!(
        manager
            .get_or_start("target", || async {
                panic!("retained owner must block restoration")
            })
            .await
            .is_err()
    );
    drop(original);
    drop(removed);
    manager.register_new("target".into(), replacement).await?;
    Ok(())
}

#[tokio::test]
async fn reservations_are_per_manager_and_target() {
    let manager = SessionManager::new();
    let other = SessionManager::new();
    let held = manager.reserve("target").await;
    let same = manager.reserve("target");
    tokio::pin!(same);
    assert!(futures::poll!(&mut same).is_pending());
    let _other_target = manager.reserve("other").await;
    let _other_manager = other.reserve("target").await;
    drop(held);
    assert!(futures::poll!(&mut same).is_ready());
}

#[tokio::test]
async fn rejected_rekey_keeps_both_original_owners() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    let first = agent("first").await;
    let second = agent("second").await;
    manager
        .register_new("first".into(), Arc::clone(&first))
        .await?;
    manager
        .register_new("second".into(), Arc::clone(&second))
        .await?;
    assert!(
        manager
            .reserve("second")
            .await
            .publish_rekeyed(Some("first"), Arc::clone(&first))
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(
        manager.read().await.get("first").unwrap(),
        &first
    ));
    assert!(Arc::ptr_eq(
        manager.read().await.get("second").unwrap(),
        &second
    ));
    let updated = Arc::try_unwrap(agent("new").await)
        .ok()
        .expect("unique test handle")
        .into_inner();
    *first.lock().await = updated;
    assert!(
        manager
            .register_new("new".into(), Arc::clone(&first))
            .await
            .is_err()
    );
    assert!(manager.read().await.contains_key("first"));
    manager
        .reserve("new")
        .await
        .publish_rekeyed(Some("first"), Arc::clone(&first))
        .await?;
    assert!(!manager.read().await.contains_key("first"));
    assert!(Arc::ptr_eq(
        manager.read().await.get("new").unwrap(),
        &first
    ));
    assert!(manager.reserve("first").await.ensure_vacant().is_ok());
    Ok(())
}

#[tokio::test]
async fn publication_rejects_wrong_session_identity() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    assert!(
        manager
            .register_new("wrong".into(), agent("actual").await)
            .await
            .is_err()
    );
    assert!(manager.read().await.is_empty());
    assert!(manager.turn_observer("wrong").is_none());
    Ok(())
}

#[tokio::test]
async fn observer_is_available_while_runtime_is_busy_and_retires_with_entry() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    let runtime = agent("target").await;
    let generation = runtime
        .lock()
        .await
        .server_turn_observer()
        .runtime_generation()
        .to_owned();
    manager
        .register_new("target".into(), Arc::clone(&runtime))
        .await?;
    let busy = runtime.lock().await;
    assert_eq!(
        manager
            .turn_observer("target")
            .unwrap()
            .runtime_generation(),
        generation
    );
    assert!(manager.remove_if_same("target", &runtime).await);
    assert!(manager.turn_observer("target").is_none());
    assert!(manager.reserve("target").await.ensure_vacant().is_err());
    drop(busy);
    Ok(())
}

#[tokio::test]
async fn accepted_turn_blocks_restore_even_before_worker_locks_agent() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    let source = Arc::try_unwrap(agent("source").await)
        .ok()
        .expect("owned candidate")
        .into_inner();
    let observer = source.server_turn_observer();
    let lease = source.claim_server_turn()?;
    let error = manager
        .reserve("missing-target")
        .await
        .restore_new(source, None, false)
        .await
        .err()
        .expect("accepted turn excludes restoration");
    assert!(error.to_string().contains("Already processing"));
    assert!(observer.active().is_some());
    drop(lease);
    assert!(observer.active().is_none());
    Ok(())
}

#[tokio::test]
async fn failed_cold_restore_keeps_published_and_retiring_sources_unchanged() -> Result<()> {
    let _env_lock = crate::storage::lock_test_env();
    let _home = TestHome::new();
    let manager = SessionManager::new();
    let source = agent("source").await;
    manager
        .register_new("source".into(), Arc::clone(&source))
        .await?;
    for removed in [false, true] {
        if removed {
            manager.remove("source").await;
        }
        let candidate = Arc::try_unwrap(agent("candidate").await)
            .ok()
            .expect("owned candidate")
            .into_inner();
        assert!(
            manager
                .reserve("missing-target")
                .await
                .restore_new(candidate, None, false)
                .await
                .is_err()
        );
        assert_eq!(source.lock().await.session_id(), "source");
    }
    Ok(())
}
