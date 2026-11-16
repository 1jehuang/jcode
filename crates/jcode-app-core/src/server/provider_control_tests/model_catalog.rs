use super::*;

#[tokio::test]
async fn failed_refresh_still_publishes_successful_partial_catalog() {
    struct PartialCatalogProvider(std::sync::atomic::AtomicBool);
    #[async_trait]
    impl Provider for PartialCatalogProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            unreachable!()
        }
        fn name(&self) -> &str {
            "partial-catalog"
        }
        fn model(&self) -> String {
            "fallback".into()
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self(std::sync::atomic::AtomicBool::new(
                self.0.load(Ordering::SeqCst),
            )))
        }
        fn available_models_display(&self) -> Vec<String> {
            if self.0.load(Ordering::SeqCst) {
                vec!["fallback".into(), "new-from-successful-provider".into()]
            } else {
                vec!["fallback".into()]
            }
        }
        async fn refresh_model_catalog(&self) -> anyhow::Result<ModelCatalogRefreshSummary> {
            self.0.store(true, Ordering::SeqCst);
            anyhow::bail!("another provider failed")
        }
    }
    let provider: Arc<dyn Provider> = Arc::new(PartialCatalogProvider(
        std::sync::atomic::AtomicBool::new(false),
    ));
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), Registry::empty())));
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_refresh_models(81, &provider, &agent, &tx).await;
    let mut saw_catalog = false;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(event) = rx.recv().await {
            match event {
                ServerEvent::AvailableModelsUpdated {
                    available_models, ..
                } => {
                    assert!(
                        available_models
                            .iter()
                            .any(|model| model == "new-from-successful-provider")
                    );
                    saw_catalog = true;
                }
                ServerEvent::Error {
                    id: 81, message, ..
                } => {
                    assert!(message.contains("another provider failed"));
                    return;
                }
                _ => {}
            }
        }
        panic!("refresh must report the provider error");
    })
    .await
    .expect("partial refresh must finish");
    assert!(
        saw_catalog,
        "partial successes must reach the picker even on error"
    );
}

#[tokio::test]
async fn refresh_models_emits_available_models_updated_after_prefetch() {
    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_refresh_models(7, &provider, &agent, &client_event_tx).await;

    let mut saw_done = false;
    let mut saw_models = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, client_event_rx.recv())
            .await
            .expect("receive server event before timeout");
        match event.expect("channel open") {
            ServerEvent::Done { id } => {
                assert_eq!(id, 7);
                saw_done = true;
            }
            ServerEvent::AvailableModelsUpdated {
                provider_name,
                provider_model,
                available_models,
                available_model_routes,
            } => {
                saw_models = Some((
                    provider_name,
                    provider_model,
                    available_models,
                    available_model_routes,
                ));
                break;
            }
            _ => {}
        }
    }

    assert!(saw_done, "expected immediate Done ack");
    let (provider_name, provider_model, available_models, available_model_routes) =
        saw_models.expect("expected AvailableModelsUpdated event");
    assert_eq!(provider_name.as_deref(), Some("mock-auth"));
    assert_eq!(provider_model.as_deref(), Some("logged-out-model"));
    assert_eq!(available_models, vec!["logged-out-model".to_string()]);
    assert!(available_model_routes.iter().any(|route| {
        route.model == "logged-out-model"
            && route.provider == "MockAuth"
            && route.api_method == "mock-auth"
    }));
}
