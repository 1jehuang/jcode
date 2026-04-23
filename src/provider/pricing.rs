use super::{ALL_OPENAI_MODELS, openrouter};
use crate::auth;
use crate::provider::models::provider_for_model;
use jcode_provider_core::{RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource};

fn usd_to_micros(usd: f64) -> u64 {
    (usd * 1_000_000.0).round() as u64
}

fn usd_per_token_str_to_micros_per_mtok(raw: &str) -> Option<u64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .map(|usd_per_token| (usd_per_token * 1_000_000_000_000.0).round() as u64)
}

pub(crate) fn anthropic_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let long_context = model.ends_with("[1m]");
    match base {
        "claude-opus-4-6" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(if long_context { 10.0 } else { 5.0 }),
            usd_to_micros(if long_context { 37.5 } else { 25.0 }),
            Some(usd_to_micros(if long_context { 1.0 } else { 0.5 })),
            Some(if long_context {
                "Anthropic API long-context pricing".to_string()
            } else {
                "Anthropic API pricing".to_string()
            }),
        )),
        "claude-sonnet-4-6" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(if long_context { 6.0 } else { 3.0 }),
            usd_to_micros(if long_context { 22.5 } else { 15.0 }),
            Some(usd_to_micros(if long_context { 0.6 } else { 0.3 })),
            Some(if long_context {
                "Anthropic API long-context pricing".to_string()
            } else {
                "Anthropic API pricing".to_string()
            }),
        )),
        "claude-haiku-4-5" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(1.0),
            usd_to_micros(5.0),
            Some(usd_to_micros(0.1)),
            Some("Anthropic API pricing".to_string()),
        )),
        "claude-opus-4-5" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Medium,
            usd_to_micros(5.0),
            usd_to_micros(25.0),
            Some(usd_to_micros(0.5)),
            Some("Estimated from Opus 4.6 API pricing".to_string()),
        )),
        "claude-sonnet-4-5" | "claude-sonnet-4-20250514" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Medium,
            usd_to_micros(3.0),
            usd_to_micros(15.0),
            Some(usd_to_micros(0.3)),
            Some("Estimated from Sonnet 4.6 API pricing".to_string()),
        )),
        _ => None,
    }
}

fn anthropic_oauth_subscription_type() -> Option<String> {
    auth::claude::get_subscription_type().map(|raw| raw.trim().to_ascii_lowercase())
}

pub(crate) fn anthropic_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    let subscription = anthropic_oauth_subscription_type();
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let is_opus = base.contains("opus");
    let is_1m = model.ends_with("[1m]");

    match subscription.as_deref() {
        Some("max") => RouteCheapnessEstimate::subscription(
            RouteCostSource::RuntimePlan,
            RouteCostConfidence::Medium,
            usd_to_micros(100.0),
            None,
            Some(if is_opus {
                "Claude Max plan; Opus access included; 1M context".to_string()
            } else {
                "Claude Max plan; 1M context".to_string()
            }),
        ),
        Some("pro") => RouteCheapnessEstimate::subscription(
            RouteCostSource::RuntimePlan,
            RouteCostConfidence::Medium,
            usd_to_micros(20.0),
            None,
            Some(if is_1m {
                "Claude Pro plan; 1M context requires extra usage".to_string()
            } else {
                "Claude Pro plan".to_string()
            }),
        ),
        Some(other) => RouteCheapnessEstimate::subscription(
            RouteCostSource::RuntimePlan,
            RouteCostConfidence::Low,
            usd_to_micros(20.0),
            None,
            Some(format!(
                "Claude OAuth plan '{}'; assumed Pro-like pricing",
                other
            )),
        ),
        None => RouteCheapnessEstimate::subscription(
            RouteCostSource::PublicPlanPricing,
            RouteCostConfidence::Low,
            usd_to_micros(if is_opus { 100.0 } else { 20.0 }),
            None,
            Some(if is_opus {
                "Opus access implies Claude Max-like subscription pricing".to_string()
            } else {
                "Claude OAuth subscription pricing (plan not detected)".to_string()
            }),
        ),
    }
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
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    match base {
        "gpt-5.5" | "gpt-5.4" | "gpt-5.4-pro" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::High,
            usd_to_micros(2.5),
            usd_to_micros(15.0),
            Some(usd_to_micros(0.25)),
            Some("OpenAI API pricing".to_string()),
        )),
        "gpt-5.3-codex" | "gpt-5.2-codex" | "gpt-5.2" | "gpt-5.1" | "gpt-5.1-codex" => {
            Some(RouteCheapnessEstimate::metered(
                RouteCostSource::Heuristic,
                RouteCostConfidence::Low,
                usd_to_micros(2.5),
                usd_to_micros(15.0),
                Some(usd_to_micros(0.25)),
                Some("Estimated from GPT-5.4 API pricing".to_string()),
            ))
        }
        "gpt-5.3-codex-spark" | "gpt-5.1-codex-mini" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Low,
            usd_to_micros(0.25),
            usd_to_micros(2.0),
            Some(usd_to_micros(0.025)),
            Some("Estimated from GPT-5 mini API pricing".to_string()),
        )),
        "gpt-5.1-codex-max"
        | "gpt-5.2-pro"
        | "gpt-5-chat-latest"
        | "gpt-5.1-chat-latest"
        | "gpt-5.2-chat-latest"
        | "gpt-5-codex"
        | "gpt-5" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Low,
            usd_to_micros(2.5),
            usd_to_micros(15.0),
            Some(usd_to_micros(0.25)),
            Some("Estimated from GPT-5.4 API pricing".to_string()),
        )),
        _ => None,
    }
}

pub(crate) fn openai_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let likely_pro = base.contains("pro") || matches!(base, "gpt-5.5" | "gpt-5.4");
    RouteCheapnessEstimate::subscription(
        RouteCostSource::PublicPlanPricing,
        RouteCostConfidence::Low,
        usd_to_micros(if likely_pro { 200.0 } else { 20.0 }),
        None,
        Some(if likely_pro {
            "ChatGPT subscription estimate; advanced GPT-5 access treated as Pro-like".to_string()
        } else {
            "ChatGPT subscription estimate".to_string()
        }),
    )
}

pub(crate) fn copilot_pricing(model: &str) -> RouteCheapnessEstimate {
    let mode = std::env::var("JCODE_COPILOT_PREMIUM").ok();
    let is_zero = matches!(mode.as_deref(), Some("0"));
    let likely_premium_model =
        model.contains("opus") || model.contains("gpt-5.5") || model.contains("gpt-5.4");
    let monthly_price = if likely_premium_model {
        usd_to_micros(39.0)
    } else {
        usd_to_micros(10.0)
    };
    let included_requests = if likely_premium_model { 1_500 } else { 300 };
    let estimated_reference = if is_zero {
        Some(0)
    } else {
        Some(monthly_price / included_requests)
    };

    RouteCheapnessEstimate::included_quota(
        RouteCostSource::RuntimePlan,
        if is_zero {
            RouteCostConfidence::High
        } else {
            RouteCostConfidence::Medium
        },
        monthly_price,
        Some(included_requests),
        estimated_reference,
        Some(if is_zero {
            "Copilot zero-premium mode: jcode will send requests as agent/non-premium when possible"
                .to_string()
        } else if likely_premium_model {
            "Copilot premium-request estimate using Pro+/premium pricing".to_string()
        } else {
            "Copilot estimate using Pro included premium requests".to_string()
        }),
    )
}

pub(crate) fn openrouter_pricing_from_model_pricing(
    pricing: &openrouter::ModelPricing,
    source: RouteCostSource,
    confidence: RouteCostConfidence,
    note: Option<String>,
) -> Option<RouteCheapnessEstimate> {
    let input = pricing
        .prompt
        .as_deref()
        .and_then(usd_per_token_str_to_micros_per_mtok)?;
    let output = pricing
        .completion
        .as_deref()
        .and_then(usd_per_token_str_to_micros_per_mtok)?;
    let cache = pricing
        .input_cache_read
        .as_deref()
        .and_then(usd_per_token_str_to_micros_per_mtok);
    Some(RouteCheapnessEstimate::metered(
        source, confidence, input, output, cache, note,
    ))
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
