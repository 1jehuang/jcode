//! Shared speed-tier ladders.
//!
//! The TUI cycles speed with a hotkey (Alt+Up / Alt+Down by default), moving
//! along an ordered ladder of service tiers. Local and remote sessions both use
//! this module so they offer the same rungs for the same provider and model.
//!
//! Canonical tier values:
//! - `off`: Standard processing (no service tier override)
//! - `priority`: Fast mode (OpenAI accepts `priority` or `fast` on the wire)
//! - `ultrafast`: OpenAI Ultrafast, only for models that support it

/// Standard processing.
pub const SPEED_TIER_STANDARD: &str = "off";
/// Fast mode (formerly OpenAI Priority processing).
pub const SPEED_TIER_FAST: &str = "priority";
/// OpenAI Ultrafast.
pub const SPEED_TIER_ULTRAFAST: &str = "ultrafast";

const STANDARD_FAST: &[&str] = &[SPEED_TIER_STANDARD, SPEED_TIER_FAST];
const STANDARD_FAST_ULTRAFAST: &[&str] =
    &[SPEED_TIER_STANDARD, SPEED_TIER_FAST, SPEED_TIER_ULTRAFAST];

fn base_model(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    let model = model.strip_suffix("[1m]").unwrap_or(&model).to_string();
    // Route-qualified ids such as `openai/gpt-6-astra` keep only the model.
    model.rsplit('/').next().unwrap_or(&model).to_string()
}

/// Whether OpenAI offers the Ultrafast service tier for `model`.
///
/// Ultrafast is GA for GPT-6 Astra and in preview for GPT-5.6 Sol
/// (https://developers.openai.com/api/docs/guides/ultrafast-mode, 2026-10).
pub fn openai_model_supports_ultrafast(model: &str) -> bool {
    let model = base_model(model);
    matches!(model.as_str(), "gpt-6-astra" | "gpt-5.6-sol")
        || model.starts_with("gpt-6-astra-")
        || model.starts_with("gpt-5.6-sol-")
}

/// Map any provider-reported tier onto the canonical ladder value.
pub fn canonical_speed_tier(tier: Option<&str>) -> &'static str {
    match tier
        .map(|t| t.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("")
    {
        "priority" | "fast" => SPEED_TIER_FAST,
        "ultrafast" | "ultra" | "ultra-fast" => SPEED_TIER_ULTRAFAST,
        _ => SPEED_TIER_STANDARD,
    }
}

/// Ordered speed tiers (slowest first) available for a provider and model.
///
/// Returns an empty list when the provider has no speed tiers to switch.
pub fn speed_tier_ladder(
    provider_name: Option<&str>,
    model_name: Option<&str>,
) -> Vec<&'static str> {
    let provider = provider_name
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let model = base_model(model_name.unwrap_or_default());

    // Gateways and other providers do not forward a service tier.
    if [
        "openrouter",
        "copilot",
        "compatible",
        "bedrock",
        "gemini",
        "antigravity",
    ]
    .iter()
    .any(|gateway| provider.contains(gateway))
    {
        return Vec::new();
    }

    if provider.contains("cursor") {
        return STANDARD_FAST.to_vec();
    }

    let is_anthropic = provider.contains("claude")
        || provider.contains("anthropic")
        || (provider.is_empty() && model.starts_with("claude"));
    if is_anthropic {
        return if model.contains("claude-opus-4-8") {
            STANDARD_FAST.to_vec()
        } else {
            Vec::new()
        };
    }

    let is_openai =
        provider.contains("openai") || (provider.is_empty() && model.starts_with("gpt-"));
    if is_openai {
        return if openai_model_supports_ultrafast(&model) {
            STANDARD_FAST_ULTRAFAST.to_vec()
        } else {
            STANDARD_FAST.to_vec()
        };
    }

    Vec::new()
}

/// Step along `ladder` from `current` in `direction`, clamping at both ends.
///
/// Returns `None` when the ladder is empty. The returned flag is `true` when
/// the tier is already at the end in that direction.
pub fn step_speed_tier(
    ladder: &[&'static str],
    current: Option<&str>,
    direction: i8,
) -> Option<(usize, &'static str, bool)> {
    if ladder.is_empty() {
        return None;
    }
    let current = canonical_speed_tier(current);
    // A tier the ladder does not offer (for example Ultrafast after switching
    // to a model without it) counts as the highest rung below it.
    let index = ladder
        .iter()
        .position(|tier| *tier == current)
        .unwrap_or_else(|| {
            if current == SPEED_TIER_STANDARD {
                0
            } else {
                ladder.len() - 1
            }
        });
    let next = if direction > 0 {
        (index + 1).min(ladder.len() - 1)
    } else {
        index.saturating_sub(1)
    };
    let at_end = next == index && ladder[index] == current;
    Some((next, ladder[next], at_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_ladder_includes_ultrafast_only_for_supported_models() {
        assert_eq!(
            speed_tier_ladder(Some("openai"), Some("gpt-6-astra")),
            vec!["off", "priority", "ultrafast"]
        );
        assert_eq!(
            speed_tier_ladder(Some("OpenAI"), Some("gpt-5.6-sol[1m]")),
            vec!["off", "priority", "ultrafast"]
        );
        assert_eq!(
            speed_tier_ladder(Some("openai"), Some("gpt-5.5")),
            vec!["off", "priority"]
        );
        assert_eq!(
            speed_tier_ladder(None, Some("gpt-6-astra")),
            vec!["off", "priority", "ultrafast"]
        );
    }

    #[test]
    fn other_providers_get_fast_toggle_or_nothing() {
        assert_eq!(
            speed_tier_ladder(Some("Cursor"), Some("composer-2")),
            vec!["off", "priority"]
        );
        assert_eq!(
            speed_tier_ladder(Some("claude"), Some("claude-opus-4-8")),
            vec!["off", "priority"]
        );
        assert!(speed_tier_ladder(Some("claude"), Some("claude-sonnet-4-6")).is_empty());
        assert!(speed_tier_ladder(Some("openrouter"), Some("openai/gpt-6-astra")).is_empty());
        assert!(speed_tier_ladder(Some("copilot"), Some("gpt-6-astra")).is_empty());
    }

    #[test]
    fn stepping_clamps_at_both_ends() {
        let ladder = speed_tier_ladder(Some("openai"), Some("gpt-6-astra"));
        assert_eq!(
            step_speed_tier(&ladder, None, 1),
            Some((1, "priority", false))
        );
        assert_eq!(
            step_speed_tier(&ladder, Some("priority"), 1),
            Some((2, "ultrafast", false))
        );
        assert_eq!(
            step_speed_tier(&ladder, Some("ultrafast"), 1),
            Some((2, "ultrafast", true))
        );
        assert_eq!(
            step_speed_tier(&ladder, Some("ultrafast"), -1),
            Some((1, "priority", false))
        );
        assert_eq!(step_speed_tier(&ladder, None, -1), Some((0, "off", true)));
        assert_eq!(step_speed_tier(&[], None, 1), None);
    }

    #[test]
    fn unsupported_current_tier_steps_down_from_the_top_rung() {
        let ladder = speed_tier_ladder(Some("openai"), Some("gpt-5.5"));
        // Ultrafast left over from a previous model: Up clamps to Fast and
        // actually applies it instead of claiming it is already at max.
        assert_eq!(
            step_speed_tier(&ladder, Some("ultrafast"), 1),
            Some((1, "priority", false))
        );
        assert_eq!(
            step_speed_tier(&ladder, Some("ultrafast"), -1),
            Some((0, "off", false))
        );
    }
}
