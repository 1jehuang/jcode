//! OpenAI model selection and associated runtime state reset.

use super::*;

impl OpenAIProvider {
    pub(super) fn select_model(&self, model: &str) -> Result<()> {
        let model = model.trim();
        if self.is_browser_only() && !is_chatgpt_web_model(model) {
            anyhow::bail!(
                "OpenAI API credentials are not available for '{}'. The browser-only runtime can use '{}'; run `jcode login --provider openai` before selecting API models.",
                model,
                CHATGPT_WEB_MODEL,
            );
        }
        let scope = self.catalog_scope();
        if !is_chatgpt_web_model(model)
            && !jcode_base::provider::known_openai_model_ids_for_scope(&scope)
                .iter()
                .any(|known| known == model)
        {
            anyhow::bail!(
                "Unsupported OpenAI model '{}'. Use /model to choose from the models available to your account.",
                model,
            );
        }
        let availability = jcode_base::provider::model_availability_for_scope(&scope, model);
        if !is_chatgpt_web_model(model)
            && availability.state
                == jcode_base::provider::AccountModelAvailabilityState::Unavailable
        {
            let detail =
                jcode_base::provider::format_account_model_availability_detail(&availability)
                    .unwrap_or_else(|| "not available for your account".to_string());
            anyhow::bail!(
                "The '{}' model is not available for your account right now ({}). \
                 Use /model to see available models.",
                model,
                detail
            );
        }
        // Platform-API-only GPT Pro models cannot run on ChatGPT/Codex OAuth
        // tokens. If the loaded credential is OAuth-shaped, switch to the
        // platform API key now so selecting the model from the picker just
        // works instead of failing at request time with a backend rejection.
        if jcode_provider_core::is_openai_api_only_pro_model(model) {
            let is_chatgpt_shaped = self
                .credentials
                .try_read()
                .map(|creds| Self::is_chatgpt_mode(&creds))
                .unwrap_or(false);
            if is_chatgpt_shaped {
                self.set_credential_mode(OpenAICredentialMode::ApiKey)
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "'{}' is only available on the OpenAI platform API and needs an \
                             OPENAI_API_KEY (ChatGPT/Codex OAuth cannot run it): {}",
                            model,
                            err
                        )
                    })?;
            }
        }
        if let Ok(mut current) = self.model.try_write() {
            let changed = current.as_str() != model;
            *current = model.to_string();
            self.browser_model_unset.store(false, AtomicOrdering::Release);
            jcode_base::provider::clear_model_unavailable_for_scope(&self.catalog_scope(), model);
            drop(current);
            if changed {
                self.clear_persistent_ws_try("manual OpenAI model change reset the response chain");
                self.revalidate_reasoning_effort();
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }
}
