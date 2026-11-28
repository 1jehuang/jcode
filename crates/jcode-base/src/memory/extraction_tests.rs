use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone, Default)]
struct ExtractionProvider {
    calls: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    block_next: Arc<AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl crate::provider::Provider for ExtractionProvider {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.block_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        anyhow::ensure!(!self.fail.load(Ordering::SeqCst), "fixture failure");
        Ok(Box::pin(futures::stream::once(async move {
            Ok(crate::message::StreamEvent::TextDelta(format!(
                "fact|Fixture memory {call}.|high"
            )))
        })))
    }

    fn name(&self) -> &str {
        "extraction-fixture"
    }

    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(self.clone())
    }
}

fn run_in_child(name: &str) -> bool {
    if std::env::var_os("JCODE_MEMORY_EXTRACTION_CHECKPOINT_CHILD").is_some() {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("memory::extraction::tests::{name}"),
            "--nocapture",
        ])
        .env("JCODE_MEMORY_EXTRACTION_CHECKPOINT_CHILD", "1")
        .env("HOME", home.path())
        .env("JCODE_HOME", home.path().join("state"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn successful_transcript_checkpoint_deduplicates_concurrent_and_restored_managers() {
    if run_in_child(
        "successful_transcript_checkpoint_deduplicates_concurrent_and_restored_managers",
    ) {
        return;
    }
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(false));
    crate::provider::set_active_provider(Arc::new(ExtractionProvider {
        calls: calls.clone(),
        fail: fail.clone(),
        ..ExtractionProvider::default()
    }));
    assert_eq!(Sidecar::new().backend_name(), "provider");
    assert!(memory_llm_judge_available());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let project = sandbox.root().join("project");
        let manager = MemoryManager::new().with_project_dir(&project);
        let transcript = "User: keep the build instructions in memory.";
        let (first, concurrent) = tokio::join!(
            manager.extract_from_transcript(transcript, "checkpoint-session"),
            manager.extract_from_transcript(transcript, "checkpoint-session"),
        );
        assert_eq!(first.unwrap().len() + concurrent.unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(manager.list_all().unwrap().len(), 1);

        let restored = MemoryManager::new().with_project_dir(&project);
        assert!(
            restored
                .extract_from_transcript(transcript, "checkpoint-session")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let checkpoints: Vec<_> =
            std::fs::read_dir(sandbox.root().join("memory/extraction-checkpoints"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
        assert_eq!(checkpoints.len(), 1);
        let digest = storage::read_json::<String>(&checkpoints[0]).unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        std::fs::write(&checkpoints[0], "broken checkpoint").unwrap();
        restored
            .extract_from_transcript(transcript, "checkpoint-session")
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "corrupt optimization must not disable extraction"
        );
        manager
            .extract_from_transcript("User: new history", "checkpoint-session")
            .await
            .unwrap();
        manager
            .extract_from_transcript(transcript, "other-session")
            .await
            .unwrap();
        MemoryManager::new()
            .with_project_dir(sandbox.root().join("other-project"))
            .extract_from_transcript(transcript, "checkpoint-session")
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        fail.store(true, Ordering::SeqCst);
        for _ in 0..2 {
            assert!(
                manager
                    .extract_from_transcript("retry after error", "failed-session")
                    .await
                    .is_err()
            );
        }
        fail.store(false, Ordering::SeqCst);
        manager
            .extract_from_transcript("retry after error", "failed-session")
            .await
            .unwrap();
        assert!(
            manager
                .extract_from_transcript("retry after error", "failed-session")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 8);
    });
}

#[test]
fn unrelated_extractions_finish_while_one_checkpoint_is_blocked() {
    if run_in_child("unrelated_extractions_finish_while_one_checkpoint_is_blocked") {
        return;
    }
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let provider = ExtractionProvider::default();
    provider.block_next.store(true, Ordering::SeqCst);
    crate::provider::set_active_provider(Arc::new(provider.clone()));
    assert_eq!(Sidecar::new().backend_name(), "provider");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let first = MemoryManager::new().with_project_dir(sandbox.root().join("shared-project"));
        let other = MemoryManager::new().with_project_dir(sandbox.root().join("other-project"));
        let mut blocked =
            Box::pin(first.extract_from_transcript("blocked history", "blocked-session"));
        tokio::select! {
            result = &mut blocked => panic!("must wait in fake provider: {result:?}"),
            _ = provider.entered.notified() => {}
        }
        let mut duplicate =
            Box::pin(first.extract_from_transcript("blocked history", "blocked-session"));
        assert!(futures::poll!(&mut duplicate).is_pending());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let (same_project, other_project) = tokio::join!(
                first.extract_from_transcript("other session history", "other-session"),
                other.extract_from_transcript("other project history", "blocked-session"),
            );
            assert_eq!(same_project.unwrap().len(), 1);
            assert_eq!(other_project.unwrap().len(), 1);
        })
        .await
        .expect("unrelated checkpoint must not wait for a blocked sidecar");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
        provider.resume.notify_one();
        assert_eq!(blocked.await.unwrap().len(), 1);
        assert!(duplicate.await.unwrap().is_empty());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            first.list_all().unwrap().len(),
            2,
            "shared project retains both extraction writes"
        );
        assert_eq!(other.list_all().unwrap().len(), 1);
    });
}
