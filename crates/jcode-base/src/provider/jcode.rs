use super::{EventStream, ModelRoute, MultiProvider, NativeToolResultSender, Provider, copilot};
use crate::message::{Message, ToolDefinition};
use crate::provider::models::ensure_model_allowed_for_subscription;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, RwLock};

pub struct JcodeProvider {
    inner: MultiProvider,
    selected_model: Arc<RwLock<String>>,
}

impl JcodeProvider {
    pub fn new() -> Self {
        crate::subscription_catalog::apply_runtime_env();
        Self::apply_runtime_profile();
        let inner = MultiProvider::new_fast();
        let default_model = crate::subscription_catalog::default_model().id.to_string();
        let _ = inner.set_model(&default_model);
        Self {
            inner,
            selected_model: Arc::new(RwLock::new(default_model)),
        }
    }

    fn apply_runtime_profile() {
        let _ = crate::provider::activation::ProviderActivation::jcode_subscription(
            crate::subscription_catalog::default_model().id,
        )
        .apply_env();
    }

    fn ensure_runtime_mode(&self) {
        if !crate::subscription_catalog::is_runtime_mode_enabled() {
            crate::subscription_catalog::apply_runtime_env();
        }
        Self::apply_runtime_profile();
    }

    fn entitled_models_for(
        tier: crate::subscription_catalog::JcodeTier,
    ) -> impl Iterator<Item = &'static crate::subscription_catalog::CuratedModel> {
        crate::subscription_catalog::curated_models()
            .iter()
            .filter(move |model| tier.allows(model.min_tier))
    }

    fn entitled_models() -> impl Iterator<Item = &'static crate::subscription_catalog::CuratedModel>
    {
        Self::entitled_models_for(crate::subscription_catalog::effective_tier())
    }

    fn model_routes_for(tier: crate::subscription_catalog::JcodeTier) -> Vec<ModelRoute> {
        Self::entitled_models_for(tier)
            .map(|model| ModelRoute {
                model: model.id.to_string(),
                provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
                api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
                available: true,
                detail: crate::subscription_catalog::routing_policy_detail(model),
                cheapness: None,
            })
            .collect()
    }
}

impl Default for JcodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for JcodeProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.ensure_runtime_mode();
        self.inner
            .complete(messages, tools, system, resume_session_id)
            .await
    }

    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.ensure_runtime_mode();
        self.inner
            .complete_split(
                messages,
                tools,
                system_static,
                system_dynamic,
                resume_session_id,
            )
            .await
    }

    fn name(&self) -> &str {
        crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
    }

    fn model(&self) -> String {
        self.selected_model
            .read()
            .map(|model| model.clone())
            .unwrap_or_else(|_| crate::subscription_catalog::default_model().id.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.ensure_runtime_mode();
        ensure_model_allowed_for_subscription(model)?;
        self.inner.set_model(model)?;
        if let Ok(mut selected_model) = self.selected_model.write() {
            *selected_model = crate::subscription_catalog::canonical_model_id(model)
                .unwrap_or(model)
                .to_string();
        }
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        self.ensure_runtime_mode();
        Self::entitled_models().map(|model| model.id).collect()
    }

    fn available_models_display(&self) -> Vec<String> {
        self.ensure_runtime_mode();
        Self::entitled_models()
            .map(|model| model.id.to_string())
            .collect()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.ensure_runtime_mode();
        Self::entitled_models()
            .map(|model| model.id.to_string())
            .collect()
    }

    fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        self.inner.available_providers_for_model(model)
    }

    fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        self.inner.provider_details_for_model(model)
    }

    fn preferred_provider(&self) -> Option<String> {
        self.inner.preferred_provider()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.ensure_runtime_mode();
        Self::model_routes_for(crate::subscription_catalog::effective_tier())
    }

    async fn prefetch_models(&self) -> Result<()> {
        self.ensure_runtime_mode();
        self.inner.prefetch_models().await
    }

    fn on_auth_changed(&self) {
        self.ensure_runtime_mode();
        self.inner.on_auth_changed();
        let selected_model = self.model();
        let _ = self.inner.set_model(&selected_model);
    }

    fn auth_model_refresh_pending(&self) -> bool {
        self.inner.auth_model_refresh_pending()
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.inner.reasoning_effort()
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        self.inner.set_reasoning_effort(effort)
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        self.inner.available_efforts()
    }

    fn native_compaction_mode(&self) -> Option<String> {
        self.inner.native_compaction_mode()
    }

    fn native_compaction_threshold_tokens(&self) -> Option<usize> {
        self.inner.native_compaction_threshold_tokens()
    }

    fn transport(&self) -> Option<String> {
        self.inner.transport()
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        self.inner.set_transport(transport)
    }

    fn available_transports(&self) -> Vec<&'static str> {
        self.inner.available_transports()
    }

    fn handles_tools_internally(&self) -> bool {
        self.inner.handles_tools_internally()
    }

    async fn invalidate_credentials(&self) {
        self.inner.invalidate_credentials().await;
    }

    fn set_premium_mode(&self, mode: copilot::PremiumMode) {
        self.inner.set_premium_mode(mode);
    }

    fn premium_mode(&self) -> copilot::PremiumMode {
        self.inner.premium_mode()
    }

    fn supports_compaction(&self) -> bool {
        self.inner.supports_compaction()
    }

    fn uses_jcode_compaction(&self) -> bool {
        self.inner.uses_jcode_compaction()
    }

    async fn native_compact(
        &self,
        messages: &[Message],
        existing_summary_text: Option<&str>,
        existing_openai_encrypted_content: Option<&str>,
    ) -> Result<crate::provider::NativeCompactionResult> {
        self.inner
            .native_compact(
                messages,
                existing_summary_text,
                existing_openai_encrypted_content,
            )
            .await
    }

    fn context_window(&self) -> usize {
        self.inner.context_window()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        self.ensure_runtime_mode();
        let forked = Self::new();
        let selected_model = self.model();
        let _ = forked.set_model(&selected_model);
        Arc::new(forked)
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        self.inner.native_result_sender()
    }

    fn drain_startup_notices(&self) -> Vec<String> {
        self.inner.drain_startup_notices()
    }

    fn switch_active_provider_to(&self, provider: &str) -> Result<()> {
        self.ensure_runtime_mode();
        self.inner.switch_active_provider_to(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jcode_provider_enables_subscription_runtime_mode() {
        let _guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

        runtime.block_on(async {
            let provider = JcodeProvider::new();
            assert!(crate::subscription_catalog::is_runtime_mode_enabled());
            assert!(
                provider
                    .available_models_display()
                    .into_iter()
                    .all(|model| crate::subscription_catalog::is_curated_model(&model))
            );
        });

        crate::subscription_catalog::clear_runtime_env();
    }

    #[test]
    fn jcode_provider_name_and_default_model_are_curated() {
        let _guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

        runtime.block_on(async {
            let provider = JcodeProvider::new();
            assert_eq!(provider.name(), "Jcode Subscription");
            let model = provider.model();
            assert!(
                crate::subscription_catalog::is_curated_model(&model),
                "expected curated model, got {model}"
            );
        });

        crate::subscription_catalog::clear_runtime_env();
    }

    #[test]
    fn jcode_provider_exposes_only_explicit_subscription_routes() {
        use crate::subscription_catalog::JcodeTier;

        let plus_routes = JcodeProvider::model_routes_for(JcodeTier::Plus);
        let gpt_route = plus_routes
            .iter()
            .find(|route| route.model == "gpt-5.5")
            .expect("Plus tier includes GPT-5.5");
        let route_selection = jcode_provider_core::RouteSelection::from_model_route(gpt_route);
        let flagship_routes = JcodeProvider::model_routes_for(JcodeTier::Flagship);
        let expected_models = vec![
            "claude-opus-4-8",
            "claude-sonnet-4-6",
            "gpt-5.5",
            "gpt-5.6-sol",
            "qwen3-coder-next",
            "devstral-2-123b",
            "deepseek-v3.2",
            "nova-2-lite",
            "llama-4-maverick",
            "llama-4-scout",
            "minimax-m2.5",
            "mistral-large-3",
            "kimi-k2.5",
            "kimi-k2-thinking",
            "nemotron-nano-3-30b",
            "nemotron-super-3-120b",
            "gpt-oss-120b",
            "gpt-oss-20b",
            "qwen3-next-80b",
            "glm-5",
            "glm-4.7-flash",
        ];

        assert_eq!(
            plus_routes
                .iter()
                .map(|route| route.model.as_str())
                .collect::<Vec<_>>(),
            expected_models
        );
        assert!(plus_routes.iter().all(|route| {
            route.provider == "Jcode Subscription"
                && route.api_method == "jcode-subscription"
                && route.available
        }));
        assert_eq!(
            JcodeProvider::entitled_models_for(JcodeTier::Plus)
                .map(|model| model.id.to_string())
                .collect::<Vec<_>>(),
            expected_models
                .iter()
                .map(|model| (*model).to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(route_selection.routed_model_spec(), "gpt-5.5");
        assert_eq!(
            route_selection.runtime_key,
            jcode_provider_core::RuntimeKey::JcodeSubscription
        );
        assert_eq!(route_selection.api_method, "jcode-subscription");
        assert_eq!(route_selection.provider_label, "Jcode Subscription");
        assert_eq!(flagship_routes.len(), 22);
        assert!(
            flagship_routes
                .iter()
                .any(|route| route.model == "claude-fable-5")
        );
    }
}
