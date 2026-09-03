use super::{ALL_OPENAI_MODELS, openrouter};
use crate::auth;
use crate::provider::models::provider_for_model;
use jcode_provider_core::pricing as core_pricing;
use jcode_provider_core::{RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Route-catalog builds call the pricing helpers once per route, and the
/// Anthropic/OpenAI ones re-read credential files (and re-parse config.toml
/// for external-source trust checks) on every call. Across a 2000+ route
/// catalog that dominated build time, so the auth-derived inputs are memoized
/// here with a short TTL. Invalidated eagerly via
/// [`invalidate_auth_pricing_memos`] whenever auth state changes.
const AUTH_PRICING_MEMO_TTL: Duration = Duration::from_secs(5);

static SUBSCRIPTION_TYPE_MEMO: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);
static OPENAI_AUTH_MODE_MEMO: Mutex<Option<(Instant, &'static str)>> = Mutex::new(None);

/// Monotonic generation bumped on every auth invalidation. Route-catalog memos
/// snapshot this at build time so `AuthStatus::invalidate_cache()` immediately
/// invalidates every provider's memoized catalog, not just the pricing inputs.
static AUTH_PRICING_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn auth_pricing_generation() -> u64 {
    AUTH_PRICING_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Drop memoized auth-derived pricing inputs (subscription type, effective
/// OpenAI auth mode). Called from `AuthStatus::invalidate_cache()` so pricing
/// labels update immediately after logins/logouts instead of after the TTL.
pub(crate) fn invalidate_auth_pricing_memos() {
    AUTH_PRICING_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut memo) = SUBSCRIPTION_TYPE_MEMO.lock() {
        *memo = None;
    }
    if let Ok(mut memo) = OPENAI_AUTH_MODE_MEMO.lock() {
        *memo = None;
    }
}

#[cfg(test)]
pub(crate) fn anthropic_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    core_pricing::anthropic_api_pricing(model)
}

/// Memoization is skipped in test builds: test sandboxes swap `JCODE_HOME`
/// and credential env vars between cases without going through
/// `AuthStatus::invalidate_cache()`, so a TTL'd memo would leak state across
/// tests. `test-support` covers downstream crates' test targets via feature
/// unification.
fn auth_pricing_memos_enabled() -> bool {
    !cfg!(any(test, feature = "test-support"))
}

fn anthropic_oauth_subscription_type() -> Option<String> {
    if auth_pricing_memos_enabled()
        && let Ok(memo) = SUBSCRIPTION_TYPE_MEMO.lock()
        && let Some((cached_at, subscription)) = memo.as_ref()
        && cached_at.elapsed() < AUTH_PRICING_MEMO_TTL
    {
        return subscription.clone();
    }
    let subscription =
        auth::claude::get_subscription_type().map(|raw| raw.trim().to_ascii_lowercase());
    if let Ok(mut memo) = SUBSCRIPTION_TYPE_MEMO.lock() {
        *memo = Some((Instant::now(), subscription.clone()));
    }
    subscription
}

pub(crate) fn anthropic_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    let subscription = anthropic_oauth_subscription_type();
    core_pricing::anthropic_oauth_pricing(model, subscription.as_deref())
}

pub(crate) fn openai_effective_auth_mode() -> &'static str {
    if auth_pricing_memos_enabled()
        && let Ok(memo) = OPENAI_AUTH_MODE_MEMO.lock()
        && let Some((cached_at, mode)) = memo.as_ref()
        && cached_at.elapsed() < AUTH_PRICING_MEMO_TTL
    {
        return mode;
    }
    let mode = match auth::codex::load_credentials() {
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
    };
    if let Ok(mut memo) = OPENAI_AUTH_MODE_MEMO.lock() {
        *memo = Some((Instant::now(), mode));
    }
    mode
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
    let (model, pinned_provider) = openrouter::parse_model_spec(model);
    let provider = pinned_provider
        .as_ref()
        .map(|pin| pin.name.as_str())
        .unwrap_or(provider);
    let cache = openrouter::load_endpoints_disk_cache_public(&model);
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

    openrouter::load_model_pricing_disk_cache_public(&model).and_then(|pricing| {
        openrouter_pricing_from_model_pricing(
            &pricing,
            RouteCostSource::OpenRouterCatalog,
            RouteCostConfidence::Medium,
            Some("OpenRouter model catalog pricing".to_string()),
        )
    })
}

fn usd_to_micros(usd: f64) -> u64 {
    (usd * 1_000_000.0).round() as u64
}

/// Unified metered per-token pricing resolver for any provider/model pair.
///
/// `source_key` is the cross-provider activity key (see
/// [`crate::provider_activity::source_key_for_provider_label`]), e.g.
/// `claude:api-key`, `openai:api-key`, `openrouter`,
/// `openai-compatible:deepseek`, `bedrock`.
///
/// Resolution order:
///   1. Curated static tables (exact, hand-reviewed) for Anthropic/OpenAI.
///   2. OpenRouter endpoint/catalog disk caches for OpenRouter routes.
///   3. The live models.dev pricing catalog cache (140+ providers).
///
/// Returns `None` when nothing can price the route, so callers can
/// distinguish "unknown" from "free" instead of silently guessing.
pub fn metered_pricing_for_source(source_key: &str, model: &str) -> Option<RouteCheapnessEstimate> {
    metered_pricing_for_source_with_tier(source_key, model, None)
}

/// Like [`metered_pricing_for_source`] but honoring the active service tier
/// (`/fast on` priority tier, OpenAI flex) which changes per-token rates on
/// the dual-auth providers' premium models.
pub fn metered_pricing_for_source_with_tier(
    source_key: &str,
    model: &str,
    service_tier: Option<&str>,
) -> Option<RouteCheapnessEstimate> {
    // 1. Curated static tables.
    let static_estimate = match source_key {
        "claude:api-key" => core_pricing::anthropic_api_pricing_with_tier(model, service_tier),
        "openai:api-key" => core_pricing::openai_api_pricing_with_tier(model, service_tier),
        _ => None,
    };
    if static_estimate.is_some() {
        return static_estimate;
    }

    // 2. OpenRouter's own caches carry per-endpoint pricing, which is more
    // precise than any catalog average for the route actually used.
    if source_key == "openrouter"
        && let Some(estimate) = openrouter_route_pricing(model, "auto")
    {
        return Some(estimate);
    }

    // 3. Live models.dev catalog (disk cache; refreshes in the background).
    let cost = crate::model_pricing::lookup(source_key, model)?;
    Some(RouteCheapnessEstimate::metered(
        RouteCostSource::ModelsDevCatalog,
        RouteCostConfidence::High,
        usd_to_micros(cost.input_usd_per_mtok),
        usd_to_micros(cost.output_usd_per_mtok),
        cost.cache_read_usd_per_mtok.map(usd_to_micros),
        Some("models.dev pricing catalog".to_string()),
    ))
}

pub(crate) fn cheapness_for_route(
    model: &str,
    provider: &str,
    api_method: &str,
) -> Option<RouteCheapnessEstimate> {
    use jcode_provider_core::{AuthMode, AuthRoute, DualAuthProvider};

    // Dual-auth (Anthropic/OpenAI OAuth-vs-API) methods are recognized through
    // the single shared parser so pricing never disagrees with the routing
    // layer about whether a route is subscription (OAuth) or metered (API key).
    if let Some(route) = AuthRoute::parse(api_method) {
        return match (route.provider, route.mode) {
            (DualAuthProvider::Anthropic, AuthMode::Oauth) => Some(anthropic_oauth_pricing(model)),
            (DualAuthProvider::Anthropic, AuthMode::ApiKey) => {
                // Bare `api-key` only means Anthropic when the route's provider
                // label says so; otherwise fall through to the non-dual arms.
                if provider == "Anthropic" {
                    metered_pricing_for_source("claude:api-key", model)
                } else {
                    None
                }
            }
            (DualAuthProvider::OpenAI, AuthMode::ApiKey) => Some(
                metered_pricing_for_source("openai:api-key", model)
                    .unwrap_or_else(|| openai_oauth_pricing(model)),
            ),
            (DualAuthProvider::OpenAI, AuthMode::Oauth) => {
                // An "OAuth" route still bills per token when only an API key is
                // actually configured, so honor the live effective auth mode.
                if openai_effective_auth_mode() == "api-key" {
                    Some(
                        metered_pricing_for_source("openai:api-key", model)
                            .unwrap_or_else(|| openai_oauth_pricing(model)),
                    )
                } else {
                    Some(openai_oauth_pricing(model))
                }
            }
        };
    }

    if let Some(profile_id) = api_method.strip_prefix("openai-compatible:") {
        return metered_pricing_for_source(&format!("openai-compatible:{}", profile_id), model);
    }

    match api_method {
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
                .or_else(|| metered_pricing_for_source("openrouter", &model_id))
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
    fn anthropic_api_pricing_long_context_uses_standard_rates() {
        // Anthropic bills the 1M context window at standard per-token rates.
        let estimate = anthropic_api_pricing("claude-opus-4-6[1m]").expect("priced model");
        assert_eq!(estimate.billing_kind, RouteBillingKind::Metered);
        assert_eq!(estimate.source, RouteCostSource::PublicApiPricing);
        assert_eq!(estimate.confidence, RouteCostConfidence::Exact);
        assert_eq!(estimate.input_price_per_mtok_micros, Some(5_000_000));
        assert_eq!(estimate.output_price_per_mtok_micros, Some(25_000_000));
        assert_eq!(estimate.cache_read_price_per_mtok_micros, Some(500_000));
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
    fn openrouter_pinned_endpoint_pricing_strips_pin_and_uses_endpoint_price() {
        // Regression for #1095: `model@Provider` pins used to miss both the
        // endpoint cache and the catalog and fall back to generic defaults.
        let _guard = crate::storage::lock_test_env();
        let prev_ns = std::env::var_os("JCODE_OPENROUTER_CACHE_NAMESPACE");
        let namespace = format!("pricing-test-{}", std::process::id());
        env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", &namespace);

        let model = "deepseek/deepseek-v4-pro-0813";
        let endpoints: Vec<openrouter::EndpointInfo> = serde_json::from_value(serde_json::json!([
            {
                "provider_name": "DeepInfra",
                "pricing": { "prompt": "0.0000003", "completion": "0.0000012" }
            },
            {
                "provider_name": "Sail Research",
                "pricing": { "prompt": "0.0000005", "completion": "0.0000015" }
            }
        ]))
        .expect("endpoints");
        openrouter::save_endpoints_disk_cache(model, &endpoints);

        let pinned = openrouter_route_pricing(&format!("{model}@Sail Research"), "auto")
            .expect("pinned endpoint priced");
        assert_eq!(pinned.source, RouteCostSource::OpenRouterEndpoint);
        assert_eq!(pinned.input_price_per_mtok_micros, Some(500_000));
        assert_eq!(pinned.output_price_per_mtok_micros, Some(1_500_000));

        let bare = openrouter_route_pricing(model, "auto").expect("auto route priced");
        assert_eq!(bare.input_price_per_mtok_micros, Some(300_000));

        let _ = std::fs::remove_file(
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".jcode")
                .join("cache")
                .join(format!(
                    "{namespace}_endpoints_deepseek__deepseek-v4-pro-0813.json"
                )),
        );
        match prev_ns {
            Some(prev) => env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", prev),
            None => env::remove_var("JCODE_OPENROUTER_CACHE_NAMESPACE"),
        }
    }

    #[test]
    fn cheapness_for_openai_route_falls_back_to_subscription_for_unpriced_api_key_models() {
        with_clean_provider_test_env(|| {
            env::set_var("OPENAI_API_KEY", "test-key");
            let estimate = cheapness_for_route("gpt-4.1-mini", "OpenAI", "openai-oauth")
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

    #[test]
    fn unified_resolver_prefers_static_tables_then_models_dev_catalog() {
        with_clean_provider_test_env(|| {
            crate::model_pricing::clear_memory_cache_for_tests();
            crate::model_pricing::save_test_cache(&[
                (
                    "anthropic",
                    "claude-sonnet-4-6",
                    crate::model_pricing::ModelCost {
                        // Deliberately wrong so the test proves the curated
                        // static table wins over the catalog.
                        input_usd_per_mtok: 99.0,
                        output_usd_per_mtok: 99.0,
                        cache_read_usd_per_mtok: None,
                        cache_write_usd_per_mtok: None,
                    },
                ),
                (
                    "deepseek",
                    "deepseek-v4-flash",
                    crate::model_pricing::ModelCost {
                        input_usd_per_mtok: 0.14,
                        output_usd_per_mtok: 0.28,
                        cache_read_usd_per_mtok: Some(0.0028),
                        cache_write_usd_per_mtok: None,
                    },
                ),
            ]);

            // Static table wins for curated Anthropic models.
            let sonnet = metered_pricing_for_source("claude:api-key", "claude-sonnet-4-6")
                .expect("priced model");
            assert_eq!(sonnet.source, RouteCostSource::PublicApiPricing);
            assert_eq!(sonnet.input_price_per_mtok_micros, Some(3_000_000));

            // Compatible profiles resolve through the models.dev catalog.
            let flash =
                metered_pricing_for_source("openai-compatible:deepseek", "deepseek-v4-flash")
                    .expect("priced model");
            assert_eq!(flash.source, RouteCostSource::ModelsDevCatalog);
            assert_eq!(flash.input_price_per_mtok_micros, Some(140_000));
            assert_eq!(flash.output_price_per_mtok_micros, Some(280_000));
            assert_eq!(flash.cache_read_price_per_mtok_micros, Some(2_800));

            // Unknown models return None instead of a fabricated price.
            assert!(
                metered_pricing_for_source("openai-compatible:deepseek", "unknown-model").is_none()
            );

            crate::model_pricing::clear_memory_cache_for_tests();
        });
    }
}
