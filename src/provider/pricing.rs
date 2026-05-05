use super::{ALL_OPENAI_MODELS, openrouter};
use crate::auth;
use crate::provider::models::provider_for_model;
use jcode_provider_core::pricing as core_pricing;
use jcode_provider_core::{RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource};

pub(crate) fn anthropic_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    core_pricing::anthropic_api_pricing(model)
}

fn anthropic_oauth_subscription_type() -> Option<String> {
    auth::claude::get_subscription_type().map(|raw| raw.trim().to_ascii_lowercase())
}

pub(crate) fn anthropic_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    let subscription = anthropic_oauth_subscription_type();
    core_pricing::anthropic_oauth_pricing(model, subscription.as_deref())
}

pub(crate) fn openai_effective_auth_mode() -> &'static str {
    match auth::codex::load_credentials() {
        Ok(creds) if !creds.refresh_token.is_empty() || creds.id_token.is_some() => "oauth",
        Ok(_) => "api-key",
        Err(_) => {
            if std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
            {
                "api-key"
            } else {
                "oauth"
            }
        }
    }
}

pub(crate) fn openai_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    core_pricing::openai_api_pricing(model)
}

pub(crate) fn openai_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    core_pricing::openai_oauth_pricing(model)
}

pub(crate) fn copilot_pricing(model: &str) -> RouteCheapnessEstimate {
    let zero_premium_mode = matches!(
        std::env::var("JCODE_COPILOT_PREMIUM").ok().as_deref(),
        Some("0")
    );
    core_pricing::copilot_pricing(model, zero_premium_mode)
}

pub(crate) fn openrouter_pricing_from_model_pricing(
    pricing: &openrouter::ModelPricing,
    source: RouteCostSource,
    confidence: RouteCostConfidence,
    note: Option<String>,
) -> Option<RouteCheapnessEstimate> {
    core_pricing::openrouter_pricing_from_token_prices(
        pricing.prompt.as_deref(),
        pricing.completion.as_deref(),
        pricing.input_cache_read.as_deref(),
        source,
        confidence,
        note,
    )
}

pub(crate) fn openrouter_route_pricing(
    model: &str,
    provider: &str,
) -> Option<RouteCheapnessEstimate> {
    let cache = openrouter::load_endpoints_disk_cache_public(model);
    if let Some((endpoints, _)) = cache.as_ref() {
        if provider == "auto"
            && let Some(best) = endpoints.first()
        {
            return openrouter_pricing_from_model_pricing(
                &best.pricing,
                RouteCostSource::OpenRouterEndpoint,
                RouteCostConfidence::High,
                Some(format!(
                    "OpenRouter auto route currently prefers {}",
                    best.provider_name
                )),
            );
        }
        if let Some(endpoint) = endpoints.iter().find(|ep| ep.provider_name == provider) {
            return openrouter_pricing_from_model_pricing(
                &endpoint.pricing,
                RouteCostSource::OpenRouterEndpoint,
                RouteCostConfidence::High,
                Some(format!("OpenRouter endpoint pricing for {}", provider)),
            );
        }
    }

    openrouter::load_model_pricing_disk_cache_public(model).and_then(|pricing| {
        openrouter_pricing_from_model_pricing(
            &pricing,
            RouteCostSource::OpenRouterCatalog,
            RouteCostConfidence::Medium,
            Some("OpenRouter model catalog pricing".to_string()),
        )
    })
}

pub(crate) fn cheapness_for_route(
    model: &str,
    provider: &str,
    api_method: &str,
) -> Option<RouteCheapnessEstimate> {
    match api_method {
        "claude-oauth" => Some(anthropic_oauth_pricing(model)),
        "api-key" if provider == "Anthropic" => anthropic_api_pricing(model),
        "openai-api-key" => {
            Some(openai_api_pricing(model).unwrap_or_else(|| openai_oauth_pricing(model)))
        }
        "openai-oauth" => {
            if openai_effective_auth_mode() == "api-key" {
                Some(openai_api_pricing(model).unwrap_or_else(|| openai_oauth_pricing(model)))
            } else {
                Some(openai_oauth_pricing(model))
            }
        }
        "copilot" => Some(copilot_pricing(model)),
        "openrouter" => {
            let model_id = if model.contains('/') {
                model.to_string()
            } else if provider_for_model(model) == Some("claude") {
                format!("anthropic/{}", model)
            } else if ALL_OPENAI_MODELS.contains(&model) {
                format!("openai/{}", model)
            } else {
                model.to_string()
            };
            openrouter_route_pricing(&model_id, provider)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env;
    use jcode_provider_core::{RouteBillingKind, RouteCostConfidence, RouteCostSource};

    fn with_clean_provider_test_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_openai_api_key = std::env::var_os("OPENAI_API_KEY");
        let prev_copilot_premium = std::env::var_os("JCODE_COPILOT_PREMIUM");
        crate::auth::claude::set_active_account_override(None);
        crate::auth::codex::set_active_account_override(None);
        env::set_var("JCODE_HOME", temp.path());
        env::remove_var("OPENAI_API_KEY");
        env::remove_var("JCODE_COPILOT_PREMIUM");

        let result = f();

        crate::auth::claude::set_active_account_override(None);
        crate::auth::codex::set_active_account_override(None);
        if let Some(prev_home) = prev_home {
            env::set_var("JCODE_HOME", prev_home);
        } else {
            env::remove_var("JCODE_HOME");
        }
        if let Some(prev_openai_api_key) = prev_openai_api_key {
            env::set_var("OPENAI_API_KEY", prev_openai_api_key);
        } else {
            env::remove_var("OPENAI_API_KEY");
        }
        if let Some(prev_copilot_premium) = prev_copilot_premium {
            env::set_var("JCODE_COPILOT_PREMIUM", prev_copilot_premium);
        } else {
            env::remove_var("JCODE_COPILOT_PREMIUM");
        }
        result
    }

    #[test]
    fn anthropic_api_pricing_handles_long_context_variants() {
        let estimate = anthropic_api_pricing("claude-opus-4-6[1m]").expect("priced model");
        assert_eq!(estimate.billing_kind, RouteBillingKind::Metered);
        assert_eq!(estimate.source, RouteCostSource::PublicApiPricing);
        assert_eq!(estimate.confidence, RouteCostConfidence::Exact);
        assert_eq!(estimate.input_price_per_mtok_micros, Some(10_000_000));
        assert_eq!(estimate.output_price_per_mtok_micros, Some(37_500_000));
        assert_eq!(estimate.cache_read_price_per_mtok_micros, Some(1_000_000));
    }

    #[test]
    fn openrouter_pricing_from_model_pricing_parses_token_prices() {
        let pricing = openrouter::ModelPricing {
            prompt: Some("0.0000025".to_string()),
            completion: Some("0.000015".to_string()),
            input_cache_read: Some("0.00000025".to_string()),
            input_cache_write: None,
        };
        let estimate = openrouter_pricing_from_model_pricing(
            &pricing,
            RouteCostSource::OpenRouterCatalog,
            RouteCostConfidence::Medium,
            Some("test".to_string()),
        )
        .expect("parsed pricing");

        assert_eq!(estimate.input_price_per_mtok_micros, Some(2_500_000));
        assert_eq!(estimate.output_price_per_mtok_micros, Some(15_000_000));
        assert_eq!(estimate.cache_read_price_per_mtok_micros, Some(250_000));
    }

    #[test]
    fn cheapness_for_openai_route_falls_back_to_subscription_for_unpriced_api_key_models() {
        with_clean_provider_test_env(|| {
            env::set_var("OPENAI_API_KEY", "test-key");
            let estimate = cheapness_for_route("gpt-5-mini", "OpenAI", "openai-oauth")
                .expect("cheapness estimate");
            assert_eq!(estimate.billing_kind, RouteBillingKind::Subscription);
            assert_eq!(estimate.source, RouteCostSource::PublicPlanPricing);
        });
    }

    #[test]
    fn cheapness_for_openai_route_prefers_metered_api_prices_when_available() {
        with_clean_provider_test_env(|| {
            env::set_var("OPENAI_API_KEY", "test-key");
            let estimate = cheapness_for_route("gpt-5.4", "OpenAI", "openai-oauth")
                .expect("cheapness estimate");
            assert_eq!(estimate.billing_kind, RouteBillingKind::Metered);
            assert_eq!(estimate.source, RouteCostSource::PublicApiPricing);
        });
    }

    #[test]
    fn copilot_zero_mode_marks_estimate_high_confidence_and_zero_reference_cost() {
        with_clean_provider_test_env(|| {
            env::set_var("JCODE_COPILOT_PREMIUM", "0");
            let estimate = copilot_pricing("claude-opus-4-6");
            assert_eq!(estimate.billing_kind, RouteBillingKind::IncludedQuota);
            assert_eq!(estimate.confidence, RouteCostConfidence::High);
            assert_eq!(estimate.estimated_reference_cost_micros, Some(0));
        });
    }
}
