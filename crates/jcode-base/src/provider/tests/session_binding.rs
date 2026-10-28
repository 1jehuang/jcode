use super::*;

type SessionObservations = Arc<Mutex<Vec<(&'static str, Option<String>)>>>;

#[derive(Default)]
struct SessionRecordingProvider {
    session: RwLock<Option<String>>,
    observed: SessionObservations,
}

impl SessionRecordingProvider {
    fn record(&self, operation: &'static str) {
        self.observed
            .lock()
            .unwrap()
            .push((operation, self.session.read().unwrap().clone()));
    }
}

#[async_trait::async_trait]
impl Provider for SessionRecordingProvider {
    fn set_session_id(&self, session_id: &str) {
        *self.session.write().unwrap() = Some(session_id.to_owned());
    }

    async fn prewarm(&self, _tools: &[ToolDefinition], _system_static: &str) {
        self.record("prewarm");
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        self.record("complete");
        Ok(Box::pin(futures::stream::empty()))
    }

    fn name(&self) -> &str {
        "session-recorder"
    }

    fn set_model(&self, _model: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            session: RwLock::new(self.session.read().unwrap().clone()),
            observed: Arc::clone(&self.observed),
        })
    }
}

#[test]
fn session_binding_reaches_prewarm_and_both_dispatch_modes_after_runtime_replacement() {
    let runtime = enter_test_runtime();
    runtime.block_on(async {
        let router = test_multi_provider_with_cursor();
        // Bind before a runtime exists, as startup/account failover can do.
        *router.cursor.write().unwrap() = None;
        router.set_session_id("session-stable");
        for _ in 0..2 {
            let recording = Arc::new(SessionRecordingProvider::default());
            *router.cursor.write().unwrap() = Some(recording.clone());
            router.prewarm(&[], "static").await;
            let _stream = router
                .complete_on_provider(ActiveProvider::Cursor, &[], &[], "system", None)
                .await
                .unwrap();
            let _split_stream = router
                .complete_split_on_provider(
                    ActiveProvider::Cursor,
                    &[],
                    &[],
                    "static",
                    "dynamic",
                    None,
                )
                .await
                .unwrap();
            assert_eq!(
                *recording.observed.lock().unwrap(),
                vec![
                    ("prewarm", Some("session-stable".into())),
                    ("complete", Some("session-stable".into())),
                    ("complete", Some("session-stable".into())),
                ]
            );
        }
    });
}

#[test]
fn session_binding_fork_retains_identity_and_rebinding_is_local_to_router() {
    let runtime = enter_test_runtime();
    runtime.block_on(async {
        let router = test_multi_provider_with_cursor();
        *router.cursor.write().unwrap() = None;
        *router.active.write().unwrap() = ActiveProvider::Copilot;
        let recording = Arc::new(SessionRecordingProvider::default());
        *router.copilot_api.write().unwrap() = Some(recording.clone());
        router.set_session_id("original");
        let fork = router.fork();
        router.set_session_id("rebound-parent");
        fork.prewarm(&[], "static").await;
        router.prewarm(&[], "static").await;
        fork.prewarm(&[], "static").await;
        assert_eq!(
            *recording.observed.lock().unwrap(),
            vec![
                ("prewarm", Some("original".into())),
                ("prewarm", Some("rebound-parent".into())),
                ("prewarm", Some("original".into())),
            ]
        );
    });
}

#[test]
fn session_binding_new_session_fork_starts_unbound() {
    static OBSERVED: std::sync::LazyLock<SessionObservations> =
        std::sync::LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        runtime.block_on(async {
            crate::env::set_var("OPENAI_API_KEY", "sk-test-session-binding");
            crate::config::Config::set_default_model(Some("gpt-5.5"), Some("openai-api")).unwrap();
            external::register_external_provider(external::OPENAI_RUNTIME, || {
                Arc::new(SessionRecordingProvider {
                    session: RwLock::new(None),
                    observed: Arc::clone(&OBSERVED),
                })
            });
            OBSERVED.lock().unwrap().clear();
            let router = MultiProvider::new_fast();
            router.set_session_id("parent-session");
            router.prewarm(&[], "static").await;
            let fresh = router.fork_for_new_session();
            fresh.prewarm(&[], "static").await;
            fresh.set_session_id("child-session");
            fresh.prewarm(&[], "static").await;
            assert_eq!(
                *OBSERVED.lock().unwrap(),
                vec![
                    ("prewarm", Some("parent-session".into())),
                    ("prewarm", None),
                    ("prewarm", Some("child-session".into())),
                ]
            );
            register_test_external_runtimes();
        });
    });
}
