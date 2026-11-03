//! Model catalog refresh fanout across runtime slots and compatible profiles.
use super::*;

async fn refresh_child_catalog(provider: &dyn Provider, force: bool) -> Result<()> {
    if force {
        provider.refresh_model_catalog().await.map(|_| ())
    } else {
        provider.prefetch_models().await
    }
}

impl MultiProvider {
    pub(super) async fn refresh_catalogs(&self, force: bool) -> Result<()> {
        let anthropic = self.anthropic_provider();
        let openai = self.openai_provider();
        let openrouter = self.openrouter_provider();
        let copilot = self
            .copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let antigravity = self.antigravity_provider();
        let gemini = self.gemini_provider();
        let cursor = self.cursor_provider();
        let bedrock = self.bedrock_provider();
        // Compatible profiles (including Grok Build) have independent catalogs
        // and do not occupy the OpenRouter slot.
        let compatible_profiles: Vec<_> = self
            .openai_compatible_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, provider)| (id.clone(), Arc::clone(provider)))
            .collect();

        let (
            anthropic_result,
            openai_result,
            openrouter_result,
            copilot_result,
            antigravity_result,
            gemini_result,
            cursor_result,
            bedrock_result,
            compatible_results,
        ) = tokio::join!(
            async {
                match anthropic {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match openai {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match openrouter {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match copilot {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match antigravity {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match gemini {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match cursor {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            async {
                match bedrock {
                    Some(provider) => refresh_child_catalog(provider.as_ref(), force).await,
                    None => Ok(()),
                }
            },
            futures::future::join_all(compatible_profiles.into_iter().map(
                |(id, provider)| async move {
                    (id, refresh_child_catalog(provider.as_ref(), force).await)
                }
            )),
        );

        let active_provider = self.active_provider();
        let mut errors = Vec::new();
        let mut optional_errors = Vec::new();
        let active_profile = ProviderRegistry::new(self).active_compatible_profile_id();
        for (id, result) in compatible_results {
            if let Err(err) = result {
                let message = format!("{id}: {err}");
                if active_profile.as_deref() == Some(id.as_str()) {
                    errors.push(message);
                } else {
                    optional_errors.push(message);
                }
            }
        }
        for (provider_name, result) in [
            ("anthropic", anthropic_result),
            ("openai", openai_result),
            ("openrouter", openrouter_result),
            ("copilot", copilot_result),
            ("antigravity", antigravity_result),
            ("gemini", gemini_result),
            ("cursor", cursor_result),
            ("bedrock", bedrock_result),
        ] {
            if let Err(err) = result {
                let is_active = (provider_name != "openrouter" || active_profile.is_none())
                    && matches!(
                        (active_provider, provider_name),
                        (ActiveProvider::Claude, "anthropic" | "claude")
                            | (ActiveProvider::OpenAI, "openai")
                            | (ActiveProvider::OpenRouter, "openrouter")
                            | (ActiveProvider::Copilot, "copilot")
                            | (ActiveProvider::Antigravity, "antigravity")
                            | (ActiveProvider::Gemini, "gemini")
                            | (ActiveProvider::Cursor, "cursor")
                            | (ActiveProvider::Bedrock, "bedrock")
                    );
                if !is_active || matches!(provider_name, "bedrock") {
                    optional_errors.push(format!("{provider_name}: {err}"));
                } else {
                    errors.push(format!("{provider_name}: {err}"));
                }
            }
        }

        if !optional_errors.is_empty() {
            crate::logging::warn(&format!(
                "Optional model catalog refresh failed: {}",
                optional_errors.join("; ")
            ));
        }

        // Bedrock's built-in default is a 2024 model; once its catalog (live
        // or cached) is known, land on the newest usable Claude flagship.
        if let Some(bedrock) = self.bedrock_provider() {
            promote_bedrock_placeholder_default(&bedrock);
        }

        // Other providers may have refreshed successfully even if the active
        // provider failed. Never retain a stale memo after partial success.
        self.invalidate_routes_memo_globally();
        if !errors.is_empty() {
            return Err(anyhow!("{}", errors.join("; ")));
        }
        Ok(())
    }
}
