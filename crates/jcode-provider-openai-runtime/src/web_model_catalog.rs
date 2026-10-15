//! Browser catalog integration, separate from credential-scoped API catalogs.

use super::*;
use jcode_base::provider::chatgpt_web as web_catalog;

static OPTIONAL_REFRESH_RUNNING: AtomicBool = AtomicBool::new(false);

struct OptionalRefreshGuard;
impl Drop for OptionalRefreshGuard {
    fn drop(&mut self) {
        OPTIONAL_REFRESH_RUNNING.store(false, AtomicOrdering::Release);
    }
}

impl OpenAIProvider {
    pub(super) fn initial_browser_selection() -> (String, bool) {
        if let Ok(model) = std::env::var("JCODE_OPENAI_MODEL") {
            let model = model.trim();
            if is_chatgpt_web_model(model) {
                return (model.to_string(), false);
            }
        }
        match web_catalog::cached_models()
            .into_iter()
            .find(|entry| entry.available)
        {
            Some(entry) => (entry.model, false),
            None => (CHATGPT_WEB_MODEL.to_string(), true),
        }
    }
    pub(super) fn models_with_browser_catalog(&self) -> Vec<String> {
        let web_models = web_catalog::cached_models()
            .into_iter()
            .map(|entry| entry.model);
        if self.is_browser_only() {
            return web_models.collect();
        }
        let mut models =
            jcode_base::provider::cached_openai_model_ids_for_scope(&self.catalog_scope())
                .unwrap_or_else(|| vec![self.model()]);
        models.retain(|model| !is_chatgpt_web_model(model));
        // Preserve discovery order without duplicating a stale web route.
        let mut result: Vec<String> = web_models.collect();
        result.extend(models);
        if jcode_base::provider::openai_platform_api_key_configured() {
            for pro in jcode_provider_core::OPENAI_API_ONLY_PRO_MODELS {
                if !result.iter().any(|model| model == pro) {
                    result.push((*pro).to_string());
                }
            }
        }
        result
    }

    /// True means the selected transport needs no credential-backed catalog.
    pub(super) async fn prefetch_browser_models(&self) -> Result<bool> {
        self.prefetch_browser_models_with(web_catalog::refresh_models())
            .await
    }

    pub(super) async fn prefetch_browser_models_with<F>(&self, refresh: F) -> Result<bool>
    where
        F: std::future::Future<Output = Result<Vec<web_catalog::ChatGptWebModel>>> + Send + 'static,
    {
        if self.is_browser_only() || is_chatgpt_web_model(&self.model()) {
            return self.apply_browser_catalog_refresh(refresh.await);
        }
        // Optional browser discovery must never delay a credential-backed catalog.
        // Coalesce refreshes while discovery is waiting on an active browser turn.
        if OPTIONAL_REFRESH_RUNNING
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_ok()
        {
            let guard = OptionalRefreshGuard;
            tokio::spawn(async move {
                let _guard = guard;
                if let Err(error) = refresh.await {
                    jcode_base::logging::info(&format!(
                        "ChatGPT web models are unavailable: {error:#}"
                    ));
                }
            });
        }
        Ok(false)
    }

    pub(super) fn apply_browser_catalog_refresh(
        &self,
        web: Result<Vec<web_catalog::ChatGptWebModel>>,
    ) -> Result<bool> {
        if self.is_browser_only() || is_chatgpt_web_model(&self.model()) {
            let models = web?;
            if self.browser_model_unset.load(AtomicOrdering::Acquire)
                && self.model() == CHATGPT_WEB_MODEL
                && !models
                    .iter()
                    .any(|entry| entry.available && entry.model == CHATGPT_WEB_MODEL)
            {
                let entry = models.iter().find(|entry| entry.available).context(
                    "No selectable ChatGPT web models were observed in the connected browser",
                )?;
                self.set_model(&entry.model)?;
            }
            self.revalidate_reasoning_effort();
            return Ok(true);
        }
        if let Err(error) = web {
            jcode_base::logging::info(&format!("ChatGPT web models are unavailable: {error:#}"));
        }
        Ok(false)
    }

    pub(super) fn browser_efforts(&self) -> Vec<&'static str> {
        if let Some((_, power)) = web_catalog::legacy_web_alias(&self.model()) {
            return vec![power, "swarm", "swarm-deep"];
        }
        let mut efforts: Vec<_> = web_catalog::cached_reasoning_options(&self.model())
            .into_iter()
            .filter(|option| option.available)
            .filter_map(|option| jcode_provider_core::canonical_reasoning_effort(&option.value))
            .collect();
        efforts.extend(["swarm", "swarm-deep"]);
        efforts
    }

    /// Browser models default only to observed Medium Power. API defaults stay unchanged.
    pub(super) fn default_reasoning_effort_for_model(model: &str) -> Option<String> {
        if let Some((_, power)) = web_catalog::legacy_web_alias(model) {
            return Some(power.to_string());
        }
        if is_chatgpt_web_model(model) {
            return web_catalog::cached_reasoning_options(model)
                .iter()
                .any(|option| option.available && option.value == "medium")
                .then(|| "medium".to_string());
        }
        let key = jcode_provider_core::model_id::canonical(model);
        key.starts_with("gpt-5.6-sol").then(|| "low".to_string())
    }
}
