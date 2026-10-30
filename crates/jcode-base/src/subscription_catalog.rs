use crate::provider_catalog;

pub const JCODE_API_KEY_ENV: &str = "JCODE_API_KEY";
pub const JCODE_API_BASE_ENV: &str = "JCODE_API_BASE";
pub const JCODE_ACCOUNT_ID_ENV: &str = "JCODE_ACCOUNT_ID";
pub const JCODE_ACCOUNT_EMAIL_ENV: &str = "JCODE_ACCOUNT_EMAIL";
pub const JCODE_TIER_ENV: &str = "JCODE_TIER";
pub const JCODE_ENV_FILE: &str = "jcode-subscription.env";
pub const JCODE_CACHE_NAMESPACE: &str = "jcode-subscription";
pub const JCODE_SUBSCRIPTION_ACTIVE_ENV: &str = "JCODE_SUBSCRIPTION_ACTIVE";
pub const DEFAULT_JCODE_API_BASE: &str = "https://api.jcode.sh/v1";
pub const JCODE_PRICING_URL: &str = "https://jcode.sh/pricing";
pub const JCODE_ACCOUNT_URL: &str = "https://jcode.sh/account";
/// User-facing runtime identity. Keep "Subscription" in the name so picker,
/// header, status, and diagnostics never resemble a generic model host.
pub const JCODE_PROVIDER_DISPLAY_NAME: &str = "Jcode Subscription";
pub const JCODE_ROUTE_API_METHOD: &str = "jcode-subscription";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JcodeTier {
    Plus,
    Pro,
    Max,
    Ultra,
    Flagship,
}

impl JcodeTier {
    pub const ALL: &'static [JcodeTier] = &[
        JcodeTier::Plus,
        JcodeTier::Pro,
        JcodeTier::Max,
        JcodeTier::Ultra,
        JcodeTier::Flagship,
    ];

    pub fn retail_price_usd(self) -> u32 {
        match self {
            Self::Plus => 10,
            Self::Pro => 20,
            Self::Max => 100,
            Self::Ultra => 200,
            Self::Flagship => 1000,
        }
    }

    pub fn usable_budget_usd(self) -> f64 {
        f64::from(self.retail_price_usd()) * 2.0
    }

    /// Provider-price inference included with the monthly plan. The included
    /// portion is billed at 50%, so every subscription dollar buys two dollars
    /// of inference. Usage after this allowance is billed at provider API price.
    pub fn included_inference_usd(self) -> f64 {
        self.usable_budget_usd()
    }

    pub fn overage_api_price_multiplier(self) -> f64 {
        1.0
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Plus => "Plus",
            Self::Pro => "Pro",
            Self::Max => "Max",
            Self::Ultra => "Ultra",
            Self::Flagship => "Solo",
        }
    }

    /// Stable machine identifier used for wire values and local persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plus => "plus",
            Self::Pro => "pro",
            Self::Max => "max",
            Self::Ultra => "ultra",
            Self::Flagship => "flagship",
        }
    }

    /// Parse a tier from a wire/persisted value (case-insensitive).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "plus" => Some(Self::Plus),
            "pro" => Some(Self::Pro),
            "max" => Some(Self::Max),
            "ultra" => Some(Self::Ultra),
            "flagship" | "solo" => Some(Self::Flagship),
            _ => None,
        }
    }

    /// Whether an account on this tier may use a model gated at `required`.
    pub fn allows(self, required: JcodeTier) -> bool {
        self >= required
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamRoutingPolicy {
    /// Routing is decided server-side by the jcode router (model -> provider +
    /// org key). The client does not pick upstreams; this is the only policy for
    /// the managed subscription.
    ServerManaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CuratedModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub default_enabled: bool,
    pub routing_policy: UpstreamRoutingPolicy,
    /// Minimum subscription tier that may use this model.
    pub min_tier: JcodeTier,
    pub note: &'static str,
}

pub const CURATED_MODELS: &[CuratedModel] = &[
    // Preserve existing selections while also accepting exact deployed router IDs.
    CuratedModel {
        id: "claude-opus-5-5",
        display_name: "Claude Opus 5.5",
        aliases: &["claude-opus-5-5", "opus-5-5", "opus 5.5", "claude opus 5.5"],
        default_enabled: true,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Frontier model; routed server-side to Anthropic by the jcode router.",
    },
    CuratedModel {
        id: "claude-sonnet-5-5",
        display_name: "Claude Sonnet 5.5",
        aliases: &[
            "claude-sonnet-5-5",
            "sonnet-5-5",
            "sonnet 5.5",
            "claude sonnet 5.5",
        ],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Balanced frontier model; routed server-side to Anthropic by the jcode router.",
    },
    CuratedModel {
        id: "claude-haiku-5-5",
        display_name: "Claude Haiku 5.5",
        aliases: &[
            "claude-haiku-5-5",
            "haiku-5-5",
            "haiku 5.5",
            "claude haiku 5.5",
        ],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Fast, low-cost model; routed server-side to Anthropic by the jcode router.",
    },
    CuratedModel {
        id: "gpt-6.1-sol",
        display_name: "GPT-6.1 Sol",
        aliases: &["gpt-6.1-sol", "gpt 6.1 sol", "sol"],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Frontier model; routed server-side to OpenAI by the jcode router.",
    },
    CuratedModel {
        id: "claude-sonnet-4-6",
        display_name: "Claude Sonnet 4.6",
        aliases: &[
            "claude-sonnet-4-6",
            "sonnet-4-6",
            "sonnet 4.6",
            "claude sonnet 4.6",
        ],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Deployed managed model; routed server-side to Anthropic by the jcode router.",
    },
];

pub fn curated_models() -> &'static [CuratedModel] {
    CURATED_MODELS
}

/// Default subscription model: the newest flagship in the curated catalog,
/// ranked by the same policy as every other provider's post-login selection.
/// Every curated entry is served by the jcode router, so adding a newer
/// flagship to [`CURATED_MODELS`] moves the default without editing flags.
/// `default_enabled` remains the fallback when nothing ranks.
pub fn default_model() -> &'static CuratedModel {
    static DEFAULT: std::sync::OnceLock<&'static CuratedModel> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(|| {
        let ids: Vec<&str> = CURATED_MODELS.iter().map(|model| model.id).collect();
        crate::auth::lifecycle::ranked_flagship_for_provider("claude", &ids)
            .and_then(|best| CURATED_MODELS.iter().find(|model| model.id == best))
            .or_else(|| CURATED_MODELS.iter().find(|model| model.default_enabled))
            .unwrap_or(&CURATED_MODELS[0])
    })
}

/// Normalize a model id for curated-catalog matching: strips any `@provider`
/// routing suffix, the `[1m]` long-context suffix, and lowercases.
fn normalize_model_key(model: &str) -> String {
    let base = model.trim().split('@').next().unwrap_or("").trim();
    jcode_provider_core::model_id::canonical(base)
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

/// Legacy tier metadata retained for compatibility with account servers that
/// still return tier-shaped responses. Metered hosted billing does not use it
/// for client-side model gates.
pub fn effective_tier() -> JcodeTier {
    cached_tier().unwrap_or(JcodeTier::Plus)
}

/// The last tier reported by the backend, if any was persisted.
pub fn cached_tier() -> Option<JcodeTier> {
    provider_catalog::load_env_value_from_env_or_config(JCODE_TIER_ENV, JCODE_ENV_FILE)
        .as_deref()
        .and_then(JcodeTier::parse)
}

/// Persist the last-known tier reported by the backend (`None` clears it).
pub fn store_cached_tier(tier: Option<JcodeTier>) -> anyhow::Result<()> {
    provider_catalog::save_env_value_to_env_file(
        JCODE_TIER_ENV,
        JCODE_ENV_FILE,
        tier.map(JcodeTier::as_str),
    )
}

/// Whether `model` is in the hosted catalog. Spending limits and model policy
/// are enforced by the router, never by stale client-side subscription tiers.
/// Non-curated models return `false`.
pub fn is_model_allowed_for_current_tier(model: &str) -> bool {
    is_curated_model(model)
}

pub fn routing_policy_detail(model: &CuratedModel) -> String {
    match model.routing_policy {
        UpstreamRoutingPolicy::ServerManaged => {
            "Jcode hosted routing · managed server-side".to_string()
        }
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

/// Persist an account API key and its non-secret account metadata in jcode's
/// owner-only hosted-account file.
pub fn persist_account_credentials(
    api_key: &str,
    account_id: Option<&str>,
    email: Option<&str>,
    tier: Option<&str>,
) -> anyhow::Result<()> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        anyhow::bail!("refusing to persist an empty jcode account API key");
    }

    provider_catalog::save_named_api_key(JCODE_ENV_FILE, JCODE_API_KEY_ENV, api_key)?;
    for (key, value) in [
        (JCODE_ACCOUNT_ID_ENV, nonempty(account_id)),
        (JCODE_ACCOUNT_EMAIL_ENV, nonempty(email)),
        (JCODE_TIER_ENV, nonempty(tier)),
    ] {
        provider_catalog::save_env_value_to_env_file(key, JCODE_ENV_FILE, value)?;
    }
    ensure_account_credential_permissions()
}

/// Remove the local account credential and cached account identity/tier. The
/// configured API base is intentionally retained because it is endpoint
/// configuration, not an authorization credential.
pub fn clear_account_credentials() -> anyhow::Result<()> {
    for key in [
        JCODE_API_KEY_ENV,
        JCODE_ACCOUNT_ID_ENV,
        JCODE_ACCOUNT_EMAIL_ENV,
        JCODE_TIER_ENV,
    ] {
        provider_catalog::save_env_value_to_env_file(key, JCODE_ENV_FILE, None)?;
    }
    clear_runtime_env();
    ensure_account_credential_permissions()
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub fn account_credential_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::storage::app_config_dir()?.join(JCODE_ENV_FILE))
}

/// Re-harden and verify the hosted account file after every credential mutation.
/// This is deliberately an explicit postcondition even though the shared secret
/// writer also applies owner-only permissions.
pub fn ensure_account_credential_permissions() -> anyhow::Result<()> {
    let path = account_credential_path()?;
    if !path.exists() {
        return Ok(());
    }
    crate::storage::harden_secret_file_permissions(&path);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "jcode account credential file has unsafe permissions {:03o}; expected owner-only access",
                mode
            );
        }
    }
    Ok(())
}

pub fn has_router_base() -> bool {
    configured_api_base().is_some()
}

pub fn is_runtime_mode_enabled() -> bool {
    std::env::var(JCODE_SUBSCRIPTION_ACTIVE_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

pub fn apply_runtime_env() {
    crate::env::set_var(JCODE_SUBSCRIPTION_ACTIVE_ENV, "1");
    crate::env::set_var(
        "JCODE_OPENROUTER_API_BASE",
        configured_api_base().unwrap_or_else(|| DEFAULT_JCODE_API_BASE.to_string()),
    );
    crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", JCODE_API_KEY_ENV);
    crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", JCODE_ENV_FILE);
    crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", JCODE_CACHE_NAMESPACE);
    crate::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0");
    crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "jcode-subscription");
    crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
    crate::env::remove_var("JCODE_OPENROUTER_PROVIDER");
    crate::env::remove_var("JCODE_OPENROUTER_NO_FALLBACK");
}

pub fn clear_runtime_env() {
    crate::env::remove_var(JCODE_SUBSCRIPTION_ACTIVE_ENV);
    crate::env::remove_var("JCODE_OPENROUTER_API_BASE");
    crate::env::remove_var("JCODE_OPENROUTER_API_KEY_NAME");
    crate::env::remove_var("JCODE_OPENROUTER_ENV_FILE");
    crate::env::remove_var("JCODE_OPENROUTER_CACHE_NAMESPACE");
    crate::env::remove_var("JCODE_OPENROUTER_PROVIDER_FEATURES");
    crate::env::remove_var("JCODE_OPENROUTER_TRANSPORT_STATE");
    crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
    crate::env::remove_var("JCODE_OPENROUTER_PROVIDER");
    crate::env::remove_var("JCODE_OPENROUTER_NO_FALLBACK");
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_PLUS_MODELS: &[&str] = &[
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "gpt-6.1-sol",
        "claude-sonnet-4-6",
    ];

    #[test]
    fn curated_model_aliases_resolve_to_canonical_ids() {
        assert_eq!(canonical_model_id("opus 5.5"), Some("claude-opus-5-5"));
        assert_eq!(
            canonical_model_id("Claude Opus 5.5"),
            Some("claude-opus-5-5")
        );
        assert_eq!(
            canonical_model_id("Claude Sonnet 5.5"),
            Some("claude-sonnet-5-5")
        );
        assert_eq!(canonical_model_id("sonnet 5.5"), Some("claude-sonnet-5-5"));
        assert_eq!(canonical_model_id("haiku-5-5"), Some("claude-haiku-5-5"));
        assert_eq!(
            canonical_model_id("Claude Haiku 5.5"),
            Some("claude-haiku-5-5")
        );
        assert_eq!(canonical_model_id("sol"), Some("gpt-6.1-sol"));
        assert_eq!(canonical_model_id("GPT 6.1 Sol"), Some("gpt-6.1-sol"));
        assert_eq!(canonical_model_id("unknown-model"), None);
    }

    #[test]
    fn deployed_sonnet_id_is_preserved_for_managed_selection() {
        for selection in [
            "claude-sonnet-4-6",
            "sonnet 4.6",
            "Claude Sonnet 4.6",
            "claude-sonnet-4-6@anthropic",
            "claude-sonnet-4-6[1m]",
        ] {
            assert_eq!(canonical_model_id(selection), Some("claude-sonnet-4-6"));
            assert!(is_model_allowed_for_current_tier(selection));
        }
        let model = find_curated_model("claude-sonnet-4-6").unwrap();
        assert_eq!(model.routing_policy, UpstreamRoutingPolicy::ServerManaged);
        assert_eq!(model.min_tier, JcodeTier::Plus);
        let route = crate::provider::ModelRoute {
            model: model.id.to_string(),
            provider: JCODE_PROVIDER_DISPLAY_NAME.to_string(),
            api_method: JCODE_ROUTE_API_METHOD.to_string(),
            available: true,
            detail: routing_policy_detail(model),
            usage: None,
            cheapness: None,
        };
        let selection = jcode_provider_core::RouteSelection::from_model_route(&route);
        assert_eq!(selection.routed_model_spec(), "claude-sonnet-4-6");
        assert_eq!(
            selection.runtime_key,
            jcode_provider_core::RuntimeKey::JcodeSubscription
        );
    }

    #[test]
    fn retired_models_and_unsupported_generations_are_not_curated() {
        for model in [
            "claude-opus-4-8",
            "opus 4.8",
            "claude-opus-5",
            "claude-fable-5",
            "gpt-5.4",
            "gpt-5.5",
            "gpt-5.6-sol",
            "gpt-6-sol",
            "gpt-6-astra",
            "gpt-6-luna",
            "gpt-6.1-sol-pro",
            "qwen3-coder-next",
            "devstral-2-123b",
            "deepseek-v3.2",
            "nova-2-lite",
            "minimax-m2.5",
            "mistral-large-3",
            "kimi-k2.5",
            "kimi-k2-thinking",
            "nemotron-nano-3-30b",
            "gpt-oss-120b",
            "gpt-oss-20b",
            "qwen3-next-80b",
            "glm-5",
            "glm-4.7-flash",
        ] {
            assert_eq!(canonical_model_id(model), None, "{model}");
            assert_eq!(
                canonical_model_id(&format!("{model}@openai")),
                None,
                "{model}"
            );
            assert!(!is_model_allowed_for_current_tier(model), "{model}");
        }
    }

    #[test]
    fn curated_model_lookup_ignores_provider_pin_suffix() {
        assert_eq!(
            canonical_model_id("claude-opus-5-5@anthropic"),
            Some("claude-opus-5-5")
        );
        assert_eq!(
            canonical_model_id("gpt-6.1-sol@openai"),
            Some("gpt-6.1-sol")
        );
    }

    #[test]
    fn default_model_is_newest_curated_flagship() {
        let ids: Vec<&str> = CURATED_MODELS.iter().map(|model| model.id).collect();
        let expected = crate::auth::lifecycle::ranked_flagship_for_provider("claude", &ids)
            .expect("curated catalog has a ranked Claude flagship");
        assert_eq!(default_model().id, expected);
        assert_eq!(default_model().id, "claude-opus-5-5");
    }

    #[test]
    fn curated_catalog_has_exact_hosted_set_without_legacy_model_gates() {
        assert_eq!(
            CURATED_MODELS
                .iter()
                .filter(|model| model.min_tier == JcodeTier::Plus)
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            EXPECTED_PLUS_MODELS
        );
        assert_eq!(
            CURATED_MODELS
                .iter()
                .filter(|model| model.min_tier == JcodeTier::Ultra)
                .map(|model| model.id)
                .collect::<Vec<_>>(),
            Vec::<&str>::new()
        );
        assert!(
            CURATED_MODELS
                .iter()
                .all(|model| model.min_tier != JcodeTier::Flagship)
        );
        assert_eq!(CURATED_MODELS.len(), 5);
        assert!(find_curated_model("magistral-small-1.2").is_none());
        assert!(find_curated_model("gemma-3-27b").is_none());
        assert!(find_curated_model("llama-4-maverick").is_none());
        assert!(find_curated_model("llama-4-scout").is_none());
        assert!(find_curated_model("nemotron-super-3-120b").is_none());
    }

    #[test]
    fn tier_pricing_matches_launched_plans() {
        let expected = [
            (JcodeTier::Plus, "plus", "Plus", 10, 20.00),
            (JcodeTier::Pro, "pro", "Pro", 20, 40.00),
            (JcodeTier::Max, "max", "Max", 100, 200.00),
            (JcodeTier::Ultra, "ultra", "Ultra", 200, 400.00),
            (JcodeTier::Flagship, "flagship", "Solo", 1000, 2000.00),
        ];

        assert_eq!(JcodeTier::ALL, expected.map(|(tier, ..)| tier));
        for (tier, id, display_name, retail_price, usable_budget) in expected {
            assert_eq!(tier.as_str(), id);
            assert_eq!(tier.display_name(), display_name);
            assert_eq!(tier.retail_price_usd(), retail_price);
            assert_eq!(tier.usable_budget_usd(), usable_budget);
            assert_eq!(tier.included_inference_usd(), usable_budget);
            assert_eq!(tier.overage_api_price_multiplier(), 1.0);
        }
    }

    #[test]
    fn tier_parse_round_trips() {
        for tier in JcodeTier::ALL {
            assert_eq!(JcodeTier::parse(tier.as_str()), Some(*tier));
        }
        assert_eq!(JcodeTier::parse("PLUS"), Some(JcodeTier::Plus));
        assert_eq!(JcodeTier::parse(" Pro "), Some(JcodeTier::Pro));
        assert_eq!(JcodeTier::parse("MAX"), Some(JcodeTier::Max));
        assert_eq!(JcodeTier::parse(" ultra "), Some(JcodeTier::Ultra));
        assert_eq!(JcodeTier::parse(" Flagship "), Some(JcodeTier::Flagship));
        assert_eq!(JcodeTier::parse(" Solo "), Some(JcodeTier::Flagship));
        assert_eq!(JcodeTier::parse("starter"), None);
    }

    #[test]
    fn tier_gating_follows_catalog_order() {
        for (account_index, account_tier) in JcodeTier::ALL.iter().copied().enumerate() {
            for (required_index, required_tier) in JcodeTier::ALL.iter().copied().enumerate() {
                assert_eq!(
                    account_tier.allows(required_tier),
                    account_index >= required_index,
                    "{} gating {}",
                    account_tier.display_name(),
                    required_tier.display_name()
                );
            }
        }
    }

    #[test]
    fn every_hosted_model_is_available_from_the_base_subscription() {
        for model in CURATED_MODELS {
            assert_eq!(model.min_tier, JcodeTier::Plus, "{}", model.id);
        }

        for tier in JcodeTier::ALL {
            for model in EXPECTED_PLUS_MODELS {
                assert!(tier.allows(find_curated_model(model).unwrap().min_tier));
            }
        }
    }

    #[test]
    fn hosted_catalog_access_does_not_depend_on_legacy_cached_tier() {
        let _guard = crate::storage::lock_test_env();
        crate::env::remove_var(JCODE_TIER_ENV);
        let temp = tempfile::tempdir().expect("temp home");
        crate::env::set_var("JCODE_HOME", temp.path().to_string_lossy().to_string());

        assert_eq!(cached_tier(), None);
        assert_eq!(effective_tier(), JcodeTier::Plus);
        for model in EXPECTED_PLUS_MODELS {
            assert!(is_model_allowed_for_current_tier(model));
        }
        assert!(is_model_allowed_for_current_tier("claude-opus-5-5"));

        crate::env::set_var(JCODE_TIER_ENV, "mystery");
        assert_eq!(cached_tier(), None);
        assert_eq!(effective_tier(), JcodeTier::Plus);

        for tier in [JcodeTier::Pro, JcodeTier::Max] {
            crate::env::set_var(JCODE_TIER_ENV, tier.as_str());
            assert_eq!(effective_tier(), tier);
            for model in EXPECTED_PLUS_MODELS {
                assert!(is_model_allowed_for_current_tier(model));
            }
            assert!(is_model_allowed_for_current_tier("claude-haiku-5-5"));
        }

        crate::env::set_var(JCODE_TIER_ENV, JcodeTier::Ultra.as_str());
        assert_eq!(effective_tier(), JcodeTier::Ultra);
        assert!(!is_model_allowed_for_current_tier("not-a-hosted-model"));
        // Saving writes only the file (#1389); drop the env override to read it.
        crate::env::remove_var(JCODE_TIER_ENV);
        store_cached_tier(Some(JcodeTier::Flagship)).expect("persist tier");
        assert_eq!(cached_tier(), Some(JcodeTier::Flagship));
        assert!(is_model_allowed_for_current_tier("claude-sonnet-5-5"));
        assert!(!is_model_allowed_for_current_tier("gpt-5.6-sol"));

        store_cached_tier(None).expect("clear tier");
        assert_eq!(cached_tier(), None);

        crate::env::remove_var("JCODE_HOME");
        crate::env::remove_var(JCODE_TIER_ENV);
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
