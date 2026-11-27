use super::*;
use crate::message::{Message, ToolDefinition};
use crate::provider::EventStream;
use crate::tool::Registry;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone)]
struct GatedProvider {
    model: Arc<Mutex<String>>,
    revision: Arc<AtomicU64>,
    gate: Arc<Mutex<Option<BuildGate>>>,
    builds: Arc<AtomicU64>,
    panic: Arc<AtomicBool>,
    shared: bool,
    churn: bool,
}

type BuildGate = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[async_trait::async_trait]
impl Provider for GatedProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        anyhow::bail!("unused")
    }

    fn name(&self) -> &str {
        "gated-catalog"
    }
    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        *self.model.lock().unwrap() = model.into();
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
    fn explicit_provider_pin_for_current_model(&self) -> Option<String> {
        Some("upstream".into())
    }
    fn service_tier(&self) -> Option<String> {
        Some(self.model())
    }
    fn available_models_display(&self) -> Vec<String> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.panic.load(Ordering::SeqCst),
            "intentional catalog build failure"
        );
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            release
                .recv_timeout(Duration::from_secs(5))
                .expect("release catalog build");
        }
        vec![self.model()]
    }
    fn catalog_sharing_identity(&self) -> Option<String> {
        if self.churn {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }
        self.shared.then(|| {
            format!(
                "{:p}/{}",
                Arc::as_ptr(&self.model),
                self.revision.load(Ordering::SeqCst)
            )
        })
    }
}

// Release a blocked worker even if an assertion or timeout unwinds the test.
struct Release(Option<std::sync::mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "isolated JCODE_HOME and deliberate busy-agent test"
)]
async fn offloaded_catalog_releases_agent_and_validates_selection_before_metadata() {
    let _env = crate::storage::lock_test_env();
    let dir = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());
    for scenario in ["unchanged", "changed", "aba", "busy", "catalog-revision"] {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut release = Release(Some(release_tx));
        let provider = Arc::new(GatedProvider {
            model: Arc::new(Mutex::new("model-a".into())),
            revision: Arc::new(AtomicU64::new(0)),
            gate: Arc::new(Mutex::new(None)),
            builds: Arc::new(AtomicU64::new(0)),
            panic: Arc::new(AtomicBool::new(false)),
            shared: scenario == "catalog-revision",
            churn: false,
        });
        let session =
            crate::session::Session::create_with_id(format!("catalog-{scenario}"), None, None);
        let agent = Arc::new(tokio::sync::Mutex::new(Agent::new_with_session(
            provider.clone(),
            Registry::empty(),
            session,
            None,
        )));
        // Agent initialization may inspect catalogs. Gate only the request.
        provider.builds.store(0, Ordering::SeqCst);
        *provider.gate.lock().unwrap() = Some((entered_tx, release_rx));
        let task_agent = Arc::clone(&agent);
        let task = tokio::spawn(async move {
            try_for_agent(&task_agent, |agent| agent.provider_handle().service_tier()).await
        });
        tokio::time::timeout(Duration::from_secs(2), entered_rx)
            .await
            .unwrap()
            .unwrap();
        // On the current-thread runtime this also proves the catalog isn't
        // running synchronously on the async worker. The gate is still closed.
        let mut guard = agent
            .try_lock()
            .expect("cold build must release the agent lock");
        match scenario {
            "changed" => guard.provider_handle().set_model("model-b").unwrap(),
            "aba" => {
                guard.set_model("model-b").unwrap();
                guard.set_model("model-a").unwrap();
            }
            "catalog-revision" => {
                provider.revision.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        let busy = if scenario == "busy" {
            Some(guard)
        } else {
            drop(guard);
            None
        };
        release.0.take().unwrap().send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(busy);
        if scenario != "busy" {
            let (catalog, tier) = result.expect("stable live catalog");
            let model = if scenario == "changed" {
                "model-b"
            } else {
                "model-a"
            };
            assert_eq!(
                catalog.snapshot().provider_model.as_deref(),
                Some(format!("{model}@upstream").as_str())
            );
            assert_eq!(tier.as_deref(), Some(model));
            assert_eq!(
                provider.builds.load(Ordering::SeqCst),
                if scenario == "unchanged" { 1 } else { 2 }
            );
        } else {
            assert!(
                result.is_none(),
                "{scenario} catalog must not publish mixed state"
            );
        }
    }
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[tokio::test]
async fn offloaded_catalog_join_failure_and_refresh_churn_are_not_empty_successes() {
    for churn in [false, true] {
        let provider = Arc::new(GatedProvider {
            model: Arc::new(Mutex::new("model-a".into())),
            revision: Arc::new(AtomicU64::new(0)),
            gate: Arc::new(Mutex::new(None)),
            builds: Arc::new(AtomicU64::new(0)),
            panic: Arc::new(AtomicBool::new(false)),
            shared: churn,
            churn,
        });
        let session = crate::session::Session::create_with_id("catalog-panic".into(), None, None);
        let agent = Arc::new(tokio::sync::Mutex::new(Agent::new_with_session(
            provider.clone(),
            Registry::empty(),
            session,
            None,
        )));
        provider.panic.store(!churn, Ordering::SeqCst);
        assert!(try_for_agent(&agent, |_| ()).await.is_err());
        assert!(agent.try_lock().is_ok());
        assert!(
            provider.builds.load(Ordering::SeqCst) <= 12,
            "refresh retries must be bounded"
        );
    }
}
