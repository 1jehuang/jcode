use crate::{RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource};

fn usd_to_micros(usd: f64) -> u64 {
    (usd * 1_000_000.0).round() as u64
}

fn usd_per_token_str_to_micros_per_mtok(raw: &str) -> Option<u64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .map(|usd_per_token| (usd_per_token * 1_000_000_000_000.0).round() as u64)
}

/// True when an Anthropic service tier value means fast mode. The Anthropic
/// API spells the latency-optimized tier `auto`; jcode also accepts `priority`
/// because `/fast on` is shared with OpenAI.
fn anthropic_tier_is_fast(service_tier: Option<&str>) -> bool {
    matches!(
        service_tier
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("auto") | Some("priority")
    )
}

/// Published Anthropic API pricing (docs.anthropic.com/en/docs/about-claude/pricing).
///
/// `[1m]` long-context variants bill at standard per-token rates: Anthropic
/// includes the full 1M context window at standard pricing for Fable 5,
/// Opus 4.8/4.7/4.6 and Sonnet 4.6, so the suffix never changes the estimate.
pub fn anthropic_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    anthropic_api_pricing_with_tier(model, None)
}

/// Anthropic API pricing honoring the active service tier.
///
/// Fast mode (research preview) bills premium per-token rates on Opus
/// 4.8/4.7/4.6; prompt-caching multipliers stack on top (cache read is 0.1x
/// the fast-mode input rate). Tiers on models without fast-mode pricing fall
/// back to standard rates.
pub fn anthropic_api_pricing_with_tier(
    model: &str,
    service_tier: Option<&str>,
) -> Option<RouteCheapnessEstimate> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let exact = |input_usd: f64, output_usd: f64, cache_read_usd: f64, note: &str| {
        Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(input_usd),
            usd_to_micros(output_usd),
            Some(usd_to_micros(cache_read_usd)),
            Some(note.to_string()),
        ))
    };

    if anthropic_tier_is_fast(service_tier) {
        match base {
            "claude-opus-4-8" => {
                return exact(10.0, 50.0, 1.0, "Anthropic API fast mode pricing");
            }
            "claude-opus-4-7" | "claude-opus-4-6" => {
                return exact(30.0, 150.0, 3.0, "Anthropic API fast mode pricing");
            }
            _ => {}
        }
    }

    match base {
        "claude-fable-5" => exact(10.0, 50.0, 1.0, "Anthropic API pricing"),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
        | "claude-opus-4-5" => exact(5.0, 25.0, 0.5, "Anthropic API pricing"),
        // Sonnet 5 introductory pricing ($2/$10) runs through 2026-08-31,
        // after which it moves to the standard Sonnet $3/$15 rates.
        "claude-sonnet-5" => exact(2.0, 10.0, 0.2, "Anthropic API introductory pricing"),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4-20250514" => {
            exact(3.0, 15.0, 0.3, "Anthropic API pricing")
        }
        "claude-haiku-4-5" => exact(1.0, 5.0, 0.1, "Anthropic API pricing"),
        _ => None,
    }
}

pub fn anthropic_oauth_pricing(model: &str, subscription: Option<&str>) -> RouteCheapnessEstimate {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let is_opus = base.contains("opus");
    let is_1m = model.ends_with("[1m]");

    match subscription
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
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

/// Published OpenAI API pricing (platform.openai.com/docs/pricing).
///
/// Standard tier, short-context prices. GPT-5.4+/5.5 bill a higher tier for
/// requests over ~272k input tokens; per-call estimates here use the standard
/// tier since jcode cannot see the per-request tier split.
pub fn openai_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    openai_api_pricing_with_tier(model, None)
}

/// OpenAI API pricing honoring the active service tier.
///
/// `priority` (fast mode) and `flex` bill different per-token rates on the
/// models that support them; other tier values and unsupported models fall
/// back to standard rates.
pub fn openai_api_pricing_with_tier(
    model: &str,
    service_tier: Option<&str>,
) -> Option<RouteCheapnessEstimate> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let exact = |input_usd: f64, output_usd: f64, cache_read_usd: Option<f64>, note: &str| {
        Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(input_usd),
            usd_to_micros(output_usd),
            cache_read_usd.map(usd_to_micros),
            Some(note.to_string()),
        ))
    };

    match service_tier
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("priority") => match base {
            "gpt-5.5" => return exact(12.5, 75.0, Some(1.25), "OpenAI API priority pricing"),
            "gpt-5.4" => return exact(5.0, 30.0, Some(0.5), "OpenAI API priority pricing"),
            "gpt-5.4-mini" => return exact(1.5, 9.0, Some(0.15), "OpenAI API priority pricing"),
            "gpt-5.3-codex" => return exact(3.5, 28.0, Some(0.35), "OpenAI API priority pricing"),
            _ => {}
        },
        Some("flex") => match base {
            "gpt-5.5" => return exact(2.5, 15.0, Some(0.25), "OpenAI API flex pricing"),
            "gpt-5.5-pro" => return exact(15.0, 90.0, None, "OpenAI API flex pricing"),
            "gpt-5.4" => return exact(1.25, 7.5, Some(0.13), "OpenAI API flex pricing"),
            "gpt-5.4-mini" => return exact(0.375, 2.25, Some(0.0375), "OpenAI API flex pricing"),
            "gpt-5.4-nano" => return exact(0.1, 0.625, Some(0.01), "OpenAI API flex pricing"),
            "gpt-5.4-pro" => return exact(15.0, 90.0, None, "OpenAI API flex pricing"),
            _ => {}
        },
        _ => {}
    }

    match base {
        "gpt-5.5" => exact(5.0, 30.0, Some(0.5), "OpenAI API pricing"),
        "gpt-5.5-pro" | "gpt-5.4-pro" => exact(30.0, 180.0, None, "OpenAI API pricing"),
        "gpt-5.4" => exact(2.5, 15.0, Some(0.25), "OpenAI API pricing"),
        "gpt-5.4-mini" => exact(0.75, 4.5, Some(0.075), "OpenAI API pricing"),
        "gpt-5.4-nano" => exact(0.2, 1.25, Some(0.02), "OpenAI API pricing"),
        "gpt-5.3-codex" | "gpt-5.3-codex-spark" | "gpt-5.3-chat-latest" => {
            exact(1.75, 14.0, Some(0.175), "OpenAI API pricing")
        }
        "gpt-5.2" | "gpt-5.2-codex" | "gpt-5.2-chat-latest" => {
            exact(1.75, 14.0, Some(0.175), "OpenAI API pricing")
        }
        "gpt-5.2-pro" => exact(21.0, 168.0, None, "OpenAI API pricing"),
        "gpt-5.1"
        | "gpt-5.1-codex"
        | "gpt-5.1-codex-max"
        | "gpt-5.1-chat-latest"
        | "gpt-5"
        | "gpt-5-codex"
        | "gpt-5-chat-latest" => exact(1.25, 10.0, Some(0.125), "OpenAI API pricing"),
        "gpt-5.1-codex-mini" | "gpt-5-mini" => exact(0.25, 2.0, Some(0.025), "OpenAI API pricing"),
        "gpt-5-nano" => exact(0.05, 0.4, Some(0.005), "OpenAI API pricing"),
        "gpt-5-pro" => exact(15.0, 120.0, None, "OpenAI API pricing"),
        _ => None,
    }
}

pub fn openai_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
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

pub fn copilot_pricing(model: &str, zero_premium_mode: bool) -> RouteCheapnessEstimate {
    let likely_premium_model =
        model.contains("opus") || model.contains("gpt-5.5") || model.contains("gpt-5.4");
    let monthly_price = if likely_premium_model {
        usd_to_micros(39.0)
    } else {
        usd_to_micros(10.0)
    };
    let included_requests = if likely_premium_model { 1_500 } else { 300 };
    let estimated_reference = if zero_premium_mode {
        Some(0)
    } else {
        Some(monthly_price / included_requests)
    };

    RouteCheapnessEstimate::included_quota(
        RouteCostSource::RuntimePlan,
        if zero_premium_mode {
            RouteCostConfidence::High
        } else {
            RouteCostConfidence::Medium
        },
        monthly_price,
        Some(included_requests),
        estimated_reference,
        Some(if zero_premium_mode {
            "Copilot zero-premium mode: jcode will send requests as agent/non-premium when possible"
                .to_string()
        } else if likely_premium_model {
            "Copilot premium-request estimate using Pro+/premium pricing".to_string()
        } else {
            "Copilot estimate using Pro included premium requests".to_string()
        }),
    )
}

pub fn openrouter_pricing_from_token_prices(
    prompt: Option<&str>,
    completion: Option<&str>,
    input_cache_read: Option<&str>,
    source: RouteCostSource,
    confidence: RouteCostConfidence,
    note: Option<String>,
) -> Option<RouteCheapnessEstimate> {
    let input = prompt.and_then(usd_per_token_str_to_micros_per_mtok)?;
    let output = completion.and_then(usd_per_token_str_to_micros_per_mtok)?;
    let cache = input_cache_read.and_then(usd_per_token_str_to_micros_per_mtok);
    Some(RouteCheapnessEstimate::metered(
        source, confidence, input, output, cache, note,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RouteBillingKind;

    #[test]
    fn anthropic_api_pricing_long_context_uses_standard_rates() {
        // Anthropic includes the 1M context window at standard pricing, so the
        // `[1m]` suffix must not change the estimate.
        let estimate = anthropic_api_pricing("claude-opus-4-6[1m]").expect("priced model");
        assert_eq!(estimate.billing_kind, RouteBillingKind::Metered);
        assert_eq!(estimate.source, RouteCostSource::PublicApiPricing);
        assert_eq!(estimate.confidence, RouteCostConfidence::Exact);
        assert_eq!(estimate.input_price_per_mtok_micros, Some(5_000_000));
        assert_eq!(estimate.output_price_per_mtok_micros, Some(25_000_000));
        assert_eq!(estimate.cache_read_price_per_mtok_micros, Some(500_000));
        assert_eq!(
            anthropic_api_pricing("claude-opus-4-6"),
            anthropic_api_pricing("claude-opus-4-6[1m]")
        );
        assert_eq!(
            anthropic_api_pricing("claude-sonnet-4-6"),
            anthropic_api_pricing("claude-sonnet-4-6[1m]")
        );
    }

    #[test]
    fn anthropic_api_pricing_matches_published_rates() {
        let fable = anthropic_api_pricing("claude-fable-5").expect("priced model");
        assert_eq!(fable.input_price_per_mtok_micros, Some(10_000_000));
        assert_eq!(fable.output_price_per_mtok_micros, Some(50_000_000));
        assert_eq!(fable.cache_read_price_per_mtok_micros, Some(1_000_000));

        let sonnet = anthropic_api_pricing("claude-sonnet-4-6").expect("priced model");
        assert_eq!(sonnet.input_price_per_mtok_micros, Some(3_000_000));
        assert_eq!(sonnet.output_price_per_mtok_micros, Some(15_000_000));
        assert_eq!(sonnet.cache_read_price_per_mtok_micros, Some(300_000));

        let haiku = anthropic_api_pricing("claude-haiku-4-5").expect("priced model");
        assert_eq!(haiku.input_price_per_mtok_micros, Some(1_000_000));
        assert_eq!(haiku.output_price_per_mtok_micros, Some(5_000_000));
        assert_eq!(haiku.cache_read_price_per_mtok_micros, Some(100_000));
    }

    #[test]
    fn openrouter_token_pricing_parses_token_prices() {
        let estimate = openrouter_pricing_from_token_prices(
            Some("0.0000025"),
            Some("0.000015"),
            Some("0.00000025"),
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
    fn anthropic_fast_mode_tier_bills_premium_rates() {
        // Opus 4.6 fast mode: $30/$150, cache read 0.1x fast input.
        let fast =
            anthropic_api_pricing_with_tier("claude-opus-4-6", Some("auto")).expect("priced model");
        assert_eq!(fast.input_price_per_mtok_micros, Some(30_000_000));
        assert_eq!(fast.output_price_per_mtok_micros, Some(150_000_000));
        assert_eq!(fast.cache_read_price_per_mtok_micros, Some(3_000_000));

        // Opus 4.8 fast mode: $10/$50. `priority` spelling also accepted.
        let opus48 = anthropic_api_pricing_with_tier("claude-opus-4-8", Some("priority"))
            .expect("priced model");
        assert_eq!(opus48.input_price_per_mtok_micros, Some(10_000_000));
        assert_eq!(opus48.output_price_per_mtok_micros, Some(50_000_000));

        // standard_only (off) and models without fast pricing use standard rates.
        assert_eq!(
            anthropic_api_pricing_with_tier("claude-opus-4-6", Some("standard_only")),
            anthropic_api_pricing("claude-opus-4-6")
        );
        assert_eq!(
            anthropic_api_pricing_with_tier("claude-sonnet-4-6", Some("auto")),
            anthropic_api_pricing("claude-sonnet-4-6")
        );
    }

    #[test]
    fn openai_service_tiers_bill_published_rates() {
        // gpt-5.4 priority: $5/$30.
        let priority =
            openai_api_pricing_with_tier("gpt-5.4", Some("priority")).expect("priced model");
        assert_eq!(priority.input_price_per_mtok_micros, Some(5_000_000));
        assert_eq!(priority.output_price_per_mtok_micros, Some(30_000_000));

        // gpt-5.4 flex: $1.25/$7.50.
        let flex = openai_api_pricing_with_tier("gpt-5.4", Some("flex")).expect("priced model");
        assert_eq!(flex.input_price_per_mtok_micros, Some(1_250_000));
        assert_eq!(flex.output_price_per_mtok_micros, Some(7_500_000));

        // Models without a tier-specific price fall back to standard rates.
        assert_eq!(
            openai_api_pricing_with_tier("gpt-5.2", Some("priority")),
            openai_api_pricing("gpt-5.2")
        );
        assert_eq!(
            openai_api_pricing_with_tier("gpt-5.4", None),
            openai_api_pricing("gpt-5.4")
        );
    }

    #[test]
    fn copilot_zero_mode_marks_estimate_high_confidence_and_zero_reference_cost() {
        let estimate = copilot_pricing("claude-opus-4-6", true);
        assert_eq!(estimate.billing_kind, RouteBillingKind::IncludedQuota);
        assert_eq!(estimate.confidence, RouteCostConfidence::High);
        assert_eq!(estimate.estimated_reference_cost_micros, Some(0));
    }
}
