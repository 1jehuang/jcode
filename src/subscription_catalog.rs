use crate::provider_catalog;

pub const JCODE_API_KEY_ENV: &str = "JCODE_API_KEY";
pub const JCODE_API_BASE_ENV: &str = "JCODE_API_BASE";
pub const JCODE_ENV_FILE: &str = "jcode-subscription.env";
pub const JCODE_CACHE_NAMESPACE: &str = "jcode-subscription";
pub const JCODE_SUBSCRIPTION_ACTIVE_ENV: &str = "JCODE_SUBSCRIPTION_ACTIVE";
pub const DEFAULT_JCODE_API_BASE: &str = "https://subscription.jcode.invalid/v1";

const HEALER_ALPHA_PROVIDERS: &[&str] = &["Stealth"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JcodeTier {
    Starter20,
    Pro100,
}

impl JcodeTier {
    pub fn retail_price_usd(self) -> u32 {
        match self {
            Self::Starter20 => 20,
            Self::Pro100 => 100,
        }
    }

    pub fn usable_budget_usd(self) -> f64 {
        match self {
            Self::Starter20 => 18.12,
            Self::Pro100 => 91.75,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Starter20 => "$20 Starter",
            Self::Pro100 => "$100 Pro",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamRoutingPolicy {
    CacheCapableOnly,
    ProviderAllowlist(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CuratedModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub default_enabled: bool,
    pub routing_policy: UpstreamRoutingPolicy,
    pub note: &'static str,
}

pub const CURATED_MODELS: &[CuratedModel] = &[
    CuratedModel {
        id: "openrouter/healer-alpha",
        display_name: "Healer Alpha",
        aliases: &["healer-alpha", "openrouter/healer-alpha", "healer alpha"],
        default_enabled: true,
        routing_policy: UpstreamRoutingPolicy::ProviderAllowlist(HEALER_ALPHA_PROVIDERS),
        note: "Pinned to the Stealth upstream until a cache-capable route exists.",
    },
    CuratedModel {
        id: "moonshotai/kimi-k2.5",
        display_name: "Kimi K2.5",
        aliases: &[
            "moonshotai/kimi-k2.5",
            "kimi-k2.5",
            "kimi k2.5",
            "kimi/k2.5",
        ],
        default_enabled: true,
        routing_policy: UpstreamRoutingPolicy::CacheCapableOnly,
        note: "Cache-capable upstream providers only.",
    },
];

pub fn curated_models() -> &'static [CuratedModel] {
    CURATED_MODELS
}

pub fn default_model() -> &'static CuratedModel {
    CURATED_MODELS
        .iter()
        .find(|model| model.default_enabled)
        .unwrap_or(&CURATED_MODELS[0])
}

fn normalize_model_key(model: &str) -> String {
    model
        .trim()
        .split('@')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

pub fn find_curated_model(model: &str) -> Option<&'static CuratedModel> {
    let normalized = normalize_model_key(model);
    CURATED_MODELS.iter().find(|candidate| {
        candidate.id.eq_ignore_ascii_case(&normalized)
            || candidate
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&normalized))
    })
}

pub fn canonical_model_id(model: &str) -> Option<&'static str> {
    find_curated_model(model).map(|model| model.id)
}

pub fn is_curated_model(model: &str) -> bool {
    canonical_model_id(model).is_some()
}

pub fn routing_policy_detail(model: &CuratedModel) -> String {
    match model.routing_policy {
        UpstreamRoutingPolicy::CacheCapableOnly => {
            "jcode subscription routing · cache-capable upstreams only".to_string()
        }
        UpstreamRoutingPolicy::ProviderAllowlist(providers) => format!(
            "jcode subscription routing · curated upstream: {}",
            providers.join(", ")
        ),
    }
}

pub fn configured_api_key() -> Option<String> {
    provider_catalog::load_env_value_from_env_or_config(JCODE_API_KEY_ENV, JCODE_ENV_FILE)
}

pub fn configured_api_base() -> Option<String> {
    provider_catalog::load_env_value_from_env_or_config(JCODE_API_BASE_ENV, JCODE_ENV_FILE)
}

pub fn has_credentials() -> bool {
    configured_api_key().is_some()
}

pub fn has_router_base() -> bool {
    configured_api_base().is_some()
}

pub fn is_runtime_mode_enabled() -> bool {
    std::env::var(JCODE_SUBSCRIPTION_ACTIVE_ENV)
        .ok()
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub fn apply_runtime_env() {
    std::env::set_var(JCODE_SUBSCRIPTION_ACTIVE_ENV, "1");
    std::env::set_var(
        "JCODE_OPENROUTER_API_BASE",
        configured_api_base().unwrap_or_else(|| DEFAULT_JCODE_API_BASE.to_string()),
    );
    std::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", JCODE_API_KEY_ENV);
    std::env::set_var("JCODE_OPENROUTER_ENV_FILE", JCODE_ENV_FILE);
    std::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", JCODE_CACHE_NAMESPACE);
    std::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0");
    std::env::remove_var("JCODE_OPENROUTER_PROVIDER");
    std::env::remove_var("JCODE_OPENROUTER_NO_FALLBACK");
}

pub fn clear_runtime_env() {
    std::env::remove_var(JCODE_SUBSCRIPTION_ACTIVE_ENV);
    std::env::remove_var("JCODE_OPENROUTER_API_BASE");
    std::env::remove_var("JCODE_OPENROUTER_API_KEY_NAME");
    std::env::remove_var("JCODE_OPENROUTER_ENV_FILE");
    std::env::remove_var("JCODE_OPENROUTER_CACHE_NAMESPACE");
    std::env::remove_var("JCODE_OPENROUTER_PROVIDER_FEATURES");
    std::env::remove_var("JCODE_OPENROUTER_PROVIDER");
    std::env::remove_var("JCODE_OPENROUTER_NO_FALLBACK");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_model_aliases_resolve_to_canonical_ids() {
        assert_eq!(
            canonical_model_id("kimi/k2.5"),
            Some("moonshotai/kimi-k2.5")
        );
        assert_eq!(
            canonical_model_id("KIMI K2.5"),
            Some("moonshotai/kimi-k2.5")
        );
        assert_eq!(
            canonical_model_id("openrouter/healer-alpha"),
            Some("openrouter/healer-alpha")
        );
        assert_eq!(
            canonical_model_id("healer alpha"),
            Some("openrouter/healer-alpha")
        );
        assert_eq!(canonical_model_id("unknown-model"), None);
    }

    #[test]
    fn curated_model_lookup_ignores_openrouter_provider_pin_suffix() {
        assert_eq!(
            canonical_model_id("moonshotai/kimi-k2.5@Fireworks"),
            Some("moonshotai/kimi-k2.5")
        );
    }

    #[test]
    fn runtime_mode_flag_tracks_subscription_activation() {
        let _guard = crate::storage::lock_test_env();
        clear_runtime_env();
        assert!(!is_runtime_mode_enabled());

        apply_runtime_env();
        assert!(is_runtime_mode_enabled());

        clear_runtime_env();
        assert!(!is_runtime_mode_enabled());
    }
}
