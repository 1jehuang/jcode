fn quality_first_openai_test_app() -> App {
    let provider: Arc<dyn Provider> = Arc::new(QualityFirstOpenAiProvider {
        model: std::sync::Arc::new(std::sync::RwLock::new("gpt-5.1".to_string())),
    });
    let runtime = tokio::runtime::Runtime::new().expect("registry runtime");
    let registry = runtime.block_on(crate::tool::Registry::new(provider.clone()));
    App::new_for_test_harness(provider, registry)
}
