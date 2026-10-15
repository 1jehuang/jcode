//! Model catalog prefetch orchestration and browser-only discovery fallback.

use super::*;

pub(super) async fn prefetch_openai_or_web_catalog<F>(
    openai: Option<Arc<dyn Provider>>,
    browser_refresh: F,
) -> Result<()>
where
    F: std::future::Future<Output = Result<()>>,
{
    match openai {
        Some(provider) => provider.prefetch_models().await,
        None => browser_refresh.await,
    }
}

impl MultiProvider {
    pub(super) async fn prefetch_catalog_models(&self) -> Result<()> {
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

        let (
            anthropic_result,
            openai_result,
            openrouter_result,
            copilot_result,
            antigravity_result,
            gemini_result,
            cursor_result,
            bedrock_result,
        ) = tokio::join!(
            async {
                match anthropic {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                prefetch_openai_or_web_catalog(openai, async {
                    chatgpt_web::refresh_models().await.map(|_| ())
                })
                .await
            },
            async {
                match openrouter {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match copilot {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match antigravity {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match gemini {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match cursor {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match bedrock {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
        );

        let active_provider = self.active_provider();
        let mut errors = Vec::new();
        let mut optional_errors = Vec::new();
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
                let is_active = matches!(
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

        if !errors.is_empty() {
            return Err(anyhow!("{}", errors.join("; ")));
        }

        // Fresh catalogs may have arrived; retire every memoized copy.
        self.invalidate_routes_memo_globally();
        Ok(())
    }
}
