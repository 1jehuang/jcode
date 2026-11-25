use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone)]
struct ExtractionProvider {
    calls: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        anyhow::ensure!(!self.fail.load(Ordering::SeqCst), "fixture failure");
        Ok(Box::pin(futures::stream::once(async {
            Ok(crate::message::StreamEvent::TextDelta(
                "fact|Use the checked-in build instructions.|high".into(),
            ))
        })))
    }

    fn name(&self) -> &str {
        "extraction-fixture"
    }

    fn fork(&self) -> Arc<dyn crate::provider::Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn successful_transcript_checkpoint_deduplicates_concurrent_and_restored_managers() {
    // Isolate the process-global active provider from other sidecar tests.
    const CHILD: &str = "JCODE_MEMORY_EXTRACTION_CHECKPOINT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "memory::extraction::tests::successful_transcript_checkpoint_deduplicates_concurrent_and_restored_managers",
                "--nocapture",
            ])
            .env(CHILD, "1")
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
        return;
    }
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let fail = Arc::new(AtomicBool::new(false));
    crate::provider::set_active_provider(Arc::new(ExtractionProvider {
        calls: calls.clone(),
        fail: fail.clone(),
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
