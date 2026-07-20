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
        match self {
            Self::Plus => 18.00,
            Self::Pro => 40.00,
            Self::Max => 225.00,
            Self::Ultra => 500.00,
            Self::Flagship => 3000.00,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Plus => "Plus",
            Self::Pro => "Pro",
            Self::Max => "Max",
            Self::Ultra => "Ultra",
            Self::Flagship => "Flagship",
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
            "flagship" => Some(Self::Flagship),
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
    CuratedModel {
        id: "claude-opus-4-8",
        display_name: "Claude Opus 4.8",
        aliases: &["claude-opus-4-8", "opus-4-8", "opus 4.8", "claude opus 4.8"],
        default_enabled: true,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Frontier model; routed server-side to Anthropic by the jcode router.",
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
        note: "Frontier model; routed server-side to Amazon Bedrock by the jcode router.",
    },
    CuratedModel {
        id: "gpt-5.5",
        display_name: "GPT-5.5",
        aliases: &["gpt-5.5", "gpt-5-5", "gpt 5.5"],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Frontier model; routed server-side to OpenAI by the jcode router.",
    },
    CuratedModel {
        id: "claude-fable-5",
        display_name: "Claude Fable 5",
        aliases: &["claude-fable-5", "fable-5", "fable 5", "claude fable 5"],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Flagship,
        note: "Flagship-tier model; routed server-side to Anthropic by the jcode router.",
    },
    CuratedModel {
        id: "gpt-5.6-sol",
        display_name: "GPT-5.6 Sol",
        aliases: &["gpt-5.6-sol", "gpt 5.6 sol", "sol"],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Frontier model; routed server-side to OpenAI by the jcode router.",
    },
    CuratedModel {
        id: "qwen3-coder-next",
        display_name: "Qwen3 Coder Next",
        aliases: &["qwen3-coder-next", "qwen 3 coder next", "qwen3 coder next"],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Open-weight coding model; routed server-side to Amazon Bedrock by the jcode router.",
    },
    CuratedModel {
        id: "devstral-2-123b",
        display_name: "Devstral 2 123B",
        aliases: &[
            "devstral-2-123b",
            "devstral 2 123b",
            "mistral devstral 2 123b",
        ],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Open-weight coding model; routed server-side to Amazon Bedrock by the jcode router.",
    },
    CuratedModel {
        id: "deepseek-v3.2",
        display_name: "DeepSeek V3.2",
        aliases: &["deepseek-v3.2", "deepseek v3.2", "deepseek 3.2"],
        default_enabled: false,
        routing_policy: UpstreamRoutingPolicy::ServerManaged,
        min_tier: JcodeTier::Plus,
        note: "Open-weight reasoning and coding model; routed server-side to Amazon Bedrock by the jcode router.",
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

/// The effective subscription tier for gating decisions.
///
/// `/v1/me` is the source of truth; the last-known tier is persisted to
/// `jcode-subscription.env` (`JCODE_TIER`). Unknown/absent tier behaves like
/// Plus for backward compatibility.
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

/// Whether the current (cached) tier is allowed to use `model`.
/// Non-curated models return `false`.
pub fn is_model_allowed_for_current_tier(model: &str) -> bool {
    find_curated_model(model)
        .map(|curated| effective_tier().allows(curated.min_tier))
        .unwrap_or(false)
}

pub fn routing_policy_detail(model: &CuratedModel) -> String {
    match model.routing_policy {
        UpstreamRoutingPolicy::ServerManaged => {
            "jcode subscription routing · managed server-side".to_string()
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
/// owner-only subscription file.
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

    for (key, value) in [
        (JCODE_API_KEY_ENV, Some(api_key)),
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

/// Re-harden and verify the subscription file after every credential mutation.
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

    #[test]
    fn curated_model_aliases_resolve_to_canonical_ids() {
        assert_eq!(canonical_model_id("opus 4.8"), Some("claude-opus-4-8"));
        assert_eq!(
            canonical_model_id("Claude Opus 4.8"),
            Some("claude-opus-4-8")
        );
        assert_eq!(canonical_model_id("gpt-5.5"), Some("gpt-5.5"));
        assert_eq!(canonical_model_id("GPT 5.5"), Some("gpt-5.5"));
        assert_eq!(
            canonical_model_id("Claude Sonnet 4.6"),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(canonical_model_id("sonnet 4.6"), Some("claude-sonnet-4-6"));
        assert_eq!(canonical_model_id("fable-5"), Some("claude-fable-5"));
        assert_eq!(canonical_model_id("Claude Fable 5"), Some("claude-fable-5"));
        assert_eq!(canonical_model_id("sol"), Some("gpt-5.6-sol"));
        assert_eq!(canonical_model_id("GPT 5.6 Sol"), Some("gpt-5.6-sol"));
        assert_eq!(
            canonical_model_id("Qwen 3 Coder Next"),
            Some("qwen3-coder-next")
        );
        assert_eq!(
            canonical_model_id("Mistral Devstral 2 123B"),
            Some("devstral-2-123b")
        );
        assert_eq!(canonical_model_id("DeepSeek 3.2"), Some("deepseek-v3.2"));
        assert_eq!(canonical_model_id("unknown-model"), None);
    }

    #[test]
    fn curated_model_lookup_ignores_provider_pin_suffix() {
        assert_eq!(
            canonical_model_id("claude-opus-4-8@anthropic"),
            Some("claude-opus-4-8")
        );
        assert_eq!(canonical_model_id("gpt-5.5@openai"), Some("gpt-5.5"));
    }

    #[test]
    fn default_model_is_opus() {
        assert_eq!(default_model().id, "claude-opus-4-8");
    }

    #[test]
    fn tier_pricing_matches_launched_plans() {
        let expected = [
            (JcodeTier::Plus, "plus", "Plus", 10, 18.00),
            (JcodeTier::Pro, "pro", "Pro", 20, 40.00),
            (JcodeTier::Max, "max", "Max", 100, 225.00),
            (JcodeTier::Ultra, "ultra", "Ultra", 200, 500.00),
            (JcodeTier::Flagship, "flagship", "Flagship", 1000, 3000.00),
        ];

        assert_eq!(JcodeTier::ALL, expected.map(|(tier, ..)| tier));
        for (tier, id, display_name, retail_price, usable_budget) in expected {
            assert_eq!(tier.as_str(), id);
            assert_eq!(tier.display_name(), display_name);
            assert_eq!(tier.retail_price_usd(), retail_price);
            assert_eq!(tier.usable_budget_usd(), usable_budget);
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
    fn model_entitlements_match_paid_tiers() {
        for model in CURATED_MODELS {
            match model.id {
                "claude-fable-5" => assert_eq!(model.min_tier, JcodeTier::Flagship),
                _ => assert_eq!(model.min_tier, JcodeTier::Plus),
            }
        }

        for tier in JcodeTier::ALL {
            assert!(tier.allows(find_curated_model("claude-opus-4-8").unwrap().min_tier));
            assert!(tier.allows(find_curated_model("claude-sonnet-4-6").unwrap().min_tier));
            assert!(tier.allows(find_curated_model("gpt-5.5").unwrap().min_tier));
            assert!(tier.allows(find_curated_model("gpt-5.6-sol").unwrap().min_tier));
            assert!(tier.allows(find_curated_model("qwen3-coder-next").unwrap().min_tier));
            assert!(tier.allows(find_curated_model("devstral-2-123b").unwrap().min_tier));
            assert!(tier.allows(find_curated_model("deepseek-v3.2").unwrap().min_tier));
            assert_eq!(
                tier.allows(find_curated_model("claude-fable-5").unwrap().min_tier),
                *tier == JcodeTier::Flagship
            );
        }
    }

    #[test]
    fn effective_tier_defaults_to_plus_when_unknown() {
        let _guard = crate::storage::lock_test_env();
        crate::env::remove_var(JCODE_TIER_ENV);
        let temp = tempfile::tempdir().expect("temp home");
        crate::env::set_var("JCODE_HOME", temp.path().to_string_lossy().to_string());

        assert_eq!(cached_tier(), None);
        assert_eq!(effective_tier(), JcodeTier::Plus);
        assert!(is_model_allowed_for_current_tier("claude-opus-4-8"));
        assert!(is_model_allowed_for_current_tier("claude-sonnet-4-6"));
        assert!(is_model_allowed_for_current_tier("gpt-5.5"));
        assert!(is_model_allowed_for_current_tier("gpt-5.6-sol"));
        assert!(is_model_allowed_for_current_tier("qwen3-coder-next"));
        assert!(is_model_allowed_for_current_tier("devstral-2-123b"));
        assert!(is_model_allowed_for_current_tier("deepseek-v3.2"));
        assert!(!is_model_allowed_for_current_tier("claude-fable-5"));

        crate::env::set_var(JCODE_TIER_ENV, "mystery");
        assert_eq!(cached_tier(), None);
        assert_eq!(effective_tier(), JcodeTier::Plus);

        for tier in [JcodeTier::Pro, JcodeTier::Max, JcodeTier::Ultra] {
            crate::env::set_var(JCODE_TIER_ENV, tier.as_str());
            assert_eq!(effective_tier(), tier);
            assert!(is_model_allowed_for_current_tier("claude-opus-4-8"));
            assert!(is_model_allowed_for_current_tier("claude-sonnet-4-6"));
            assert!(is_model_allowed_for_current_tier("gpt-5.5"));
            assert!(is_model_allowed_for_current_tier("gpt-5.6-sol"));
            assert!(is_model_allowed_for_current_tier("qwen3-coder-next"));
            assert!(is_model_allowed_for_current_tier("devstral-2-123b"));
            assert!(is_model_allowed_for_current_tier("deepseek-v3.2"));
            assert!(!is_model_allowed_for_current_tier("claude-fable-5"));
        }

        crate::env::remove_var(JCODE_TIER_ENV);
        store_cached_tier(Some(JcodeTier::Flagship)).expect("persist tier");
        assert_eq!(cached_tier(), Some(JcodeTier::Flagship));
        assert!(is_model_allowed_for_current_tier("claude-fable-5"));
        assert!(is_model_allowed_for_current_tier("gpt-5.6-sol"));

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
