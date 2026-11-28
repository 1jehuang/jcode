use super::*;

struct TrustOverride(Option<std::ffi::OsString>);
impl TrustOverride {
    fn clear() -> Self {
        let previous = std::env::var_os("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
        crate::env::set_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", "");
        Self(previous)
    }
}
impl Drop for TrustOverride {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => crate::env::set_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", value),
            None => crate::env::remove_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES"),
        }
    }
}

#[derive(Clone)]
struct BudgetRuntime {
    input: usize,
}
#[async_trait]
impl Provider for BudgetRuntime {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("offline budget fixture")
    }
    fn name(&self) -> &'static str {
        "budget-fixture"
    }
    fn model(&self) -> String {
        "arbitrary-future-model".into()
    }
    fn set_model(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn available_models(&self) -> Vec<&'static str> {
        vec!["arbitrary-future-model"]
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
    fn context_window(&self) -> usize {
        400_000
    }
    fn input_token_limit(&self) -> usize {
        self.input
    }
}

#[test]
fn input_token_limit_follows_each_active_runtime_and_keeps_context_separate() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let _trust = TrustOverride::clear();
    let provider = MultiProvider::new_fast();
    for active in [
        ActiveProvider::Claude,
        ActiveProvider::OpenAI,
        ActiveProvider::Copilot,
        ActiveProvider::Antigravity,
        ActiveProvider::Gemini,
        ActiveProvider::Cursor,
        ActiveProvider::OpenRouter,
    ] {
        let slot = match active {
            ActiveProvider::Claude => &provider.anthropic,
            ActiveProvider::OpenAI => &provider.openai,
            ActiveProvider::Copilot => &provider.copilot_api,
            ActiveProvider::Antigravity => &provider.antigravity,
            ActiveProvider::Gemini => &provider.gemini,
            ActiveProvider::Cursor => &provider.cursor,
            ActiveProvider::OpenRouter => &provider.openrouter,
            ActiveProvider::Bedrock => unreachable!(),
        };
        *slot.write().unwrap() = Some(Arc::new(BudgetRuntime { input: 272_000 }));
        provider.set_active_provider(active);
        assert_eq!(provider.context_window(), 400_000, "{active:?}");
        assert_eq!(provider.input_token_limit(), 272_000, "{active:?}");
        for (input, expected) in [(0, 400_000), (900_000, 400_000)] {
            *slot.write().unwrap() = Some(Arc::new(BudgetRuntime { input }));
            assert_eq!(provider.input_token_limit(), expected, "{active:?}");
        }
    }
}

#[test]
fn input_token_limit_uses_the_active_compatible_profile_and_missing_runtime_fallback() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let _trust = TrustOverride::clear();
    let provider = MultiProvider::new_fast();
    *provider.openrouter.write().unwrap() = Some(Arc::new(BudgetRuntime { input: 272_000 }));
    ProviderRegistry::new(&provider)
        .install_compatible_profile("named", Arc::new(BudgetRuntime { input: 32_000 }));
    ProviderRegistry::new(&provider).set_active_compatible_profile("named");
    provider.set_active_provider(ActiveProvider::OpenRouter);
    assert_eq!(provider.input_token_limit(), 32_000);
    assert_eq!(provider.context_window(), 400_000);
    ProviderRegistry::new(&provider).clear_active_compatible_profile();
    assert_eq!(provider.input_token_limit(), 272_000);
    *provider.openrouter.write().unwrap() = None;
    assert_eq!(provider.input_token_limit(), DEFAULT_CONTEXT_LIMIT);
    assert_eq!(provider.context_window(), DEFAULT_CONTEXT_LIMIT);
}
