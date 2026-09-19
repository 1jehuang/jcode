use super::*;

/// The KV-cache baseline must keep the canonical provider family: the TTL and
/// expiry classifier recognises families such as `openrouter`, not profile
/// labels, so storing a label there silently disables `Expired` detection for
/// direct OpenAI-compatible profiles. The human-facing label is a separate
/// field (#1286, review feedback on #1288).
#[test]
fn kv_cache_baseline_identity_stays_canonical_while_the_label_is_the_profile() {
    struct SlotProvider;

    #[async_trait::async_trait]
    impl Provider for SlotProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            unimplemented!("this test never completes a call")
        }

        fn name(&self) -> &str {
            "openrouter"
        }

        fn display_name(&self) -> String {
            "DeepSeek".to_string()
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(SlotProvider)
        }
    }

    ensure_test_jcode_home_if_unset();
    let provider: Arc<dyn Provider> = Arc::new(SlotProvider);
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let registry = rt.block_on(Registry::new(provider.clone()));
    let app = App::new_for_test_harness(provider, registry);

    assert_eq!(
        app.kv_cache_provider_name(),
        "openrouter",
        "the baseline must keep the canonical provider family"
    );
    assert_eq!(app.kv_cache_provider_label(), "DeepSeek");
    assert_eq!(
        crate::tui::cache_ttl_for_provider_model(&app.kv_cache_provider_name(), None),
        Some(300),
        "the TTL classifier must still recognise the baseline identity"
    );
    assert_eq!(
        crate::tui::cache_ttl_for_provider_model(&app.kv_cache_provider_label(), None),
        None,
        "a profile label is not a TTL family: that is why the two are split"
    );
}
