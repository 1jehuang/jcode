use crate::protocol::{AuthChanged, CatalogNamespace, RuntimeProviderKey};
use crate::provider::ModelRoute;
use crate::provider::activation::{ProviderActivation, RuntimeProviderId};
use jcode_provider_core::ActiveProvider;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthActivationRequest {
    pub legacy_provider_hint: Option<String>,
    pub auth: Option<AuthChanged>,
}

impl AuthActivationRequest {
    pub fn new(legacy_provider_hint: Option<String>, auth: Option<AuthChanged>) -> Self {
        Self {
            legacy_provider_hint,
            auth,
        }
    }

    pub fn provider_id(&self) -> Option<String> {
        self.auth
            .as_ref()
            .map(|auth| auth.provider.as_str().to_string())
            .or_else(|| self.legacy_provider_hint.clone())
            .and_then(|provider| {
                normalized_auth_provider_id(Some(provider.as_str())).map(str::to_string)
            })
    }

    pub fn expected_runtime(&self) -> Option<&RuntimeProviderKey> {
        self.auth
            .as_ref()
            .and_then(|auth| auth.expected_runtime.as_ref())
    }

    pub fn expected_catalog_namespace(&self) -> Option<&CatalogNamespace> {
        self.auth
            .as_ref()
            .and_then(|auth| auth.expected_catalog_namespace.as_ref())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthActivationResult {
    pub provider_id: Option<String>,
    pub provider_label: Option<String>,
    pub activated_model: Option<String>,
    pub expected_runtime: Option<String>,
    pub expected_catalog_namespace: Option<String>,
}

impl AuthActivationResult {
    pub fn model_switch_request(&self, current_provider_name: &str, model: &str) -> String {
        model_switch_request_for_provider_id(
            self.provider_id.as_deref(),
            current_provider_name,
            model,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthCatalogInvariantReport {
    pub applicable: bool,
    pub provider_id: Option<String>,
    pub provider_label: Option<String>,
    pub selectable_provider_routes: usize,
    pub selected_model: Option<String>,
    pub selected_model_matches_provider_route: bool,
    pub route_sample: Vec<String>,
}

impl AuthCatalogInvariantReport {
    pub fn ok(&self) -> bool {
        !self.applicable
            || (self.selectable_provider_routes > 0 && self.selected_model_matches_provider_route)
    }

    pub fn warning_message(&self) -> Option<String> {
        if self.ok() {
            return None;
        }

        let provider = self
            .provider_label
            .as_deref()
            .or(self.provider_id.as_deref())
            .unwrap_or("provider");
        let selected = self.selected_model.as_deref().unwrap_or("none");
        let sample = if self.route_sample.is_empty() {
            "none".to_string()
        } else {
            self.route_sample.join(", ")
        };
        Some(format!(
            "\n\n**Auth Model Catalog Warning**\n\nExpected selectable {provider} model routes after auth, but found {} matching route(s). Selected model: `{selected}`. Matching route sample: {sample}.",
            self.selectable_provider_routes
        ))
    }
}

pub fn validate_catalog_invariants(
    activation: &AuthActivationResult,
    selected_model: Option<&str>,
    routes: &[ModelRoute],
) -> AuthCatalogInvariantReport {
    let provider_id = activation.provider_id.clone();
    let provider_label = activation.provider_label.clone();
    let applicable = provider_id.is_some() || provider_label.is_some();
    let selected_model = selected_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string);

    let matching_routes = routes
        .iter()
        .filter(|route| route.available && route_matches_activation(route, activation))
        .collect::<Vec<_>>();
    let selected_model_matches_provider_route = selected_model
        .as_ref()
        .is_some_and(|selected| matching_routes.iter().any(|route| route.model == *selected));
    let route_sample = matching_routes
        .iter()
        .take(5)
        .map(|route| format!("`{}` via {}", route.model, route.api_method))
        .collect::<Vec<_>>();

    AuthCatalogInvariantReport {
        applicable,
        provider_id,
        provider_label,
        selectable_provider_routes: matching_routes.len(),
        selected_model,
        selected_model_matches_provider_route,
        route_sample,
    }
}

pub fn provider_model_to_select_after_auth(
    activation: &AuthActivationResult,
    selected_model: Option<&str>,
    routes: &[ModelRoute],
) -> Option<String> {
    let matching_routes = routes
        .iter()
        .filter(|route| route.available && route_matches_activation(route, activation))
        .collect::<Vec<_>>();
    if matching_routes.is_empty() {
        return None;
    }

    let selected_model = selected_model
        .map(str::trim)
        .filter(|model| !model.is_empty());
    if let Some(selected) = selected_model
        && matching_routes.iter().any(|route| route.model == selected)
    {
        let same_model_wrong_route_exists = routes.iter().any(|route| {
            route.available
                && route.model == selected
                && !route_matches_activation(route, activation)
        });
        if same_model_wrong_route_exists {
            return Some(selected.to_string());
        }
        return None;
    }

    if let Some(activated_model) = activation
        .activated_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        && matching_routes
            .iter()
            .any(|route| route.model == activated_model)
    {
        return Some(activated_model.to_string());
    }

    // No usable current model and no activation-supplied model: fall back to the
    // best available route. Plain catalog order would pick whatever the live
    // catalog happened to list first (e.g. `claude-haiku-4-5-...` ahead of
    // `claude-opus-4-8`), so an Anthropic API-key login would auto-select Haiku
    // instead of the provider's flagship default. When the provider has a
    // curated flagship-first preference order, pick the highest-ranked matching
    // route; ties and unranked providers preserve catalog order.
    let orders = provider_preferred_model_orders(activation);
    if !orders.is_empty() {
        return matching_routes
            .iter()
            .min_by_key(|route| preferred_model_rank(orders, &route.model))
            .map(|route| route.model.clone());
    }

    if let Some(provider_id) = activation.provider_id.as_deref()
        && let Some(newest_model) =
            crate::provider_catalog::newest_released_model_for_openai_compatible_profile(
                provider_id,
            )
        && matching_routes
            .iter()
            .any(|route| route.model == newest_model)
    {
        return Some(newest_model);
    }

    matching_routes.first().map(|route| route.model.clone())
}

/// Flagship-first preference tiers used only to break ties when falling back to
/// an arbitrary matching route after a login. Each inner slice is one curated
/// family ordered best-first; earlier families outrank later ones. Returns an
/// empty slice for providers without a curated order (local OpenAI-compatible,
/// Gemini, Bedrock arns, OpenRouter, ...), which preserves live-catalog order.
///
/// Copilot and Cursor proxy Claude/OpenAI models under their bare canonical ids
/// (`copilot:claude-opus-4-8`), so they share the same "catalog lists the cheap
/// model first" hazard as a direct login and get the combined Claude+OpenAI
/// order. The Claude/OpenAI subscription default bias mirrors jcode's global
/// default model.
fn provider_preferred_model_orders(
    activation: &AuthActivationResult,
) -> &'static [&'static [&'static str]] {
    match activation.provider_id.as_deref() {
        Some("claude") | Some("claude-api") => &[crate::provider::ALL_CLAUDE_MODELS],
        Some("openai") | Some("openai-api") => &[crate::provider::ALL_OPENAI_MODELS],
        Some("copilot") | Some("cursor") => &[
            crate::provider::ALL_CLAUDE_MODELS,
            crate::provider::ALL_OPENAI_MODELS,
        ],
        _ => &[],
    }
}

/// Rank a (possibly date-suffixed) catalog model id against flagship-first
/// preference tiers. Lower is more preferred: an earlier family tier always
/// outranks a later one, and within a tier the curated position decides.
/// Unknown models sort last so they only win when nothing curated matches.
fn preferred_model_rank(orders: &[&[&str]], model: &str) -> usize {
    const TIER_STRIDE: usize = 10_000;
    let normalized = normalize_model_for_preference(model);
    for (tier, order) in orders.iter().enumerate() {
        if let Some(position) = order
            .iter()
            .position(|candidate| normalize_model_for_preference(candidate) == normalized)
        {
            return tier * TIER_STRIDE + position;
        }
    }
    usize::MAX
}

/// Normalize a model id for flagship-preference comparison: lowercase, drop a
/// `[1m]` long-context suffix, and strip a trailing 8-digit `-YYYYMMDD` date so
/// live dated ids (`claude-haiku-4-5-20251001`) match bare canonical ids
/// (`claude-haiku-4-5`).
fn normalize_model_for_preference(model: &str) -> String {
    let lowered = model.trim().to_ascii_lowercase();
    let without_long_context = lowered.strip_suffix("[1m]").unwrap_or(&lowered);
    match without_long_context.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|byte| byte.is_ascii_digit()) => {
            head.to_string()
        }
        _ => without_long_context.to_string(),
    }
}

fn route_matches_activation(route: &ModelRoute, activation: &AuthActivationResult) -> bool {
    let api_method = route.api_method_kind();
    let Some(provider_id) = activation.provider_id.as_deref() else {
        if let Some(label) = activation.provider_label.as_deref()
            && route.provider.eq_ignore_ascii_case(label)
        {
            return true;
        }
        return false;
    };

    if api_method.matches_openai_compatible_profile(provider_id) {
        return true;
    }

    if route.api_method.eq_ignore_ascii_case(provider_id) {
        return true;
    }

    match provider_id {
        "claude" => {
            return matches!(
                api_method,
                crate::provider::ModelRouteApiMethod::ClaudeOAuth
            );
        }
        "claude-api" => {
            return route.provider.eq_ignore_ascii_case("Anthropic")
                && matches!(
                    api_method,
                    crate::provider::ModelRouteApiMethod::AnthropicApiKey
                );
        }
        "openai" => {
            return matches!(
                api_method,
                crate::provider::ModelRouteApiMethod::OpenAIOAuth
            );
        }
        "openai-api" => {
            return matches!(
                api_method,
                crate::provider::ModelRouteApiMethod::OpenAIApiKey
            );
        }
        _ => {}
    }

    // OpenAI-compatible auth has a concrete catalog namespace. Accepting a
    // matching display label or generic `openai-compatible` route as success can
    // hide stale/mixed catalogs, especially when providers share model IDs.
    if activation.expected_runtime.as_deref() == Some("openai-compatible")
        || activation.expected_catalog_namespace.is_some()
    {
        return false;
    }

    if let Some(label) = activation.provider_label.as_deref()
        && route.provider.eq_ignore_ascii_case(label)
    {
        return true;
    }

    false
}

pub fn normalized_auth_provider_id(provider_hint: Option<&str>) -> Option<&'static str> {
    let provider = provider_hint?.trim();
    if provider.eq_ignore_ascii_case("azure")
        || provider.eq_ignore_ascii_case("azure-openai")
        || provider.eq_ignore_ascii_case("azure openai")
    {
        Some("azure-openai")
    } else if let Some(profile) =
        crate::provider_catalog::resolve_openai_compatible_profile_selection(provider)
    {
        Some(profile.id)
    } else if let Some(descriptor) = crate::provider_catalog::resolve_login_provider(provider) {
        normalized_login_provider_id(descriptor.id)
    } else {
        None
    }
}

fn normalized_login_provider_id(provider_id: &str) -> Option<&'static str> {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => Some("claude"),
        "anthropic-api" | "claude-api" | "anthropic-key" | "claude-key" => Some("claude-api"),
        "openai" => Some("openai"),
        "openai-api" | "openai-key" | "openai-apikey" | "openai-platform" | "platform-openai" => {
            Some("openai-api")
        }
        "openrouter" => Some("openrouter"),
        "jcode" | "subscription" | "jcode-subscription" => Some("jcode"),
        "bedrock" | "aws-bedrock" | "aws_bedrock" => Some("bedrock"),
        "cursor" => Some("cursor"),
        "copilot" => Some("copilot"),
        "gemini" => Some("gemini"),
        "antigravity" => Some("antigravity"),
        _ => None,
    }
}

pub fn provider_display_label(provider_id: Option<&str>) -> Option<String> {
    let provider = normalized_auth_provider_id(provider_id)?;
    if provider == "azure-openai" {
        return Some("Azure OpenAI".to_string());
    }
    crate::provider_catalog::openai_compatible_profile_by_id(provider)
        .map(|profile| profile.display_name.to_string())
        .or_else(|| {
            crate::provider_catalog::resolve_login_provider(provider)
                .map(|descriptor| descriptor.display_name.to_string())
        })
        .or_else(|| Some(provider.to_string()))
}

pub fn activate_auth_change(request: &AuthActivationRequest) -> AuthActivationResult {
    let provider_id = request.provider_id();
    let provider_label = provider_display_label(provider_id.as_deref());
    let activated_model = apply_auth_provider_runtime(provider_id.as_deref());
    AuthActivationResult {
        provider_id,
        provider_label,
        activated_model,
        expected_runtime: request
            .expected_runtime()
            .map(|runtime| runtime.as_str().to_string()),
        expected_catalog_namespace: request
            .expected_catalog_namespace()
            .map(|namespace| namespace.as_str().to_string()),
    }
}

fn apply_auth_provider_runtime(provider_id: Option<&str>) -> Option<String> {
    match normalized_auth_provider_id(provider_id) {
        Some("azure-openai") => match crate::provider::activation::apply_azure_openai_runtime() {
            Ok(model) => model,
            Err(error) => {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    "azure-openai",
                    &[("reason", message.as_str())],
                );
                None
            }
        },
        Some(profile_id)
            if direct_provider_activation(profile_id).is_none()
                && crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
                    .is_some() =>
        {
            let Some(profile) =
                crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
            else {
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    profile_id,
                    &[(
                        "reason",
                        "openai-compatible profile disappeared during activation",
                    )],
                );
                return None;
            };
            crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(profile));
            let default_model =
                crate::provider_catalog::resolve_openai_compatible_profile(profile).default_model;
            if let Err(error) =
                crate::provider::activation::apply_openai_compatible_runtime(default_model.clone())
            {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    profile_id,
                    &[("reason", message.as_str())],
                );
                None
            } else {
                default_model
            }
        }
        Some(provider_id) => {
            if let Some(activation) = direct_provider_activation(provider_id)
                && let Err(error) = activation.apply_env()
            {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    provider_id,
                    &[("reason", message.as_str())],
                );
            }
            None
        }
        _ => None,
    }
}

fn direct_provider_activation(provider_id: &str) -> Option<ProviderActivation> {
    match normalized_login_provider_id(provider_id)? {
        "claude" => Some(ProviderActivation::locked(
            RuntimeProviderId::Claude,
            ActiveProvider::Claude,
        )),
        "claude-api" => Some(ProviderActivation::locked(
            RuntimeProviderId::ClaudeApiKey,
            ActiveProvider::Claude,
        )),
        "openai" => Some(ProviderActivation::locked(
            RuntimeProviderId::OpenAi,
            ActiveProvider::OpenAI,
        )),
        "openai-api" => Some(ProviderActivation::locked(
            RuntimeProviderId::OpenAiApiKey,
            ActiveProvider::OpenAI,
        )),
        "openrouter" => Some(ProviderActivation::locked(
            RuntimeProviderId::OpenRouter,
            ActiveProvider::OpenRouter,
        )),
        "jcode" => Some(ProviderActivation::locked(
            RuntimeProviderId::Jcode,
            ActiveProvider::OpenRouter,
        )),
        "bedrock" => Some(ProviderActivation::locked(
            RuntimeProviderId::Bedrock,
            ActiveProvider::Bedrock,
        )),
        "cursor" => Some(ProviderActivation::locked(
            RuntimeProviderId::Cursor,
            ActiveProvider::Cursor,
        )),
        "copilot" => Some(ProviderActivation::locked(
            RuntimeProviderId::Copilot,
            ActiveProvider::Copilot,
        )),
        "gemini" => Some(ProviderActivation::locked(
            RuntimeProviderId::Gemini,
            ActiveProvider::Gemini,
        )),
        "antigravity" => Some(ProviderActivation::locked(
            RuntimeProviderId::Antigravity,
            ActiveProvider::Antigravity,
        )),
        _ => None,
    }
}

pub fn model_switch_request_for_provider_id(
    provider_id: Option<&str>,
    _provider_name: &str,
    model: &str,
) -> String {
    match normalized_auth_provider_id(provider_id) {
        Some("azure-openai") => format!("openrouter:{}", model),
        Some(profile_id)
            if profile_id != "azure-openai"
                && crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
                    .is_some() =>
        {
            format!("{}:{}", profile_id, model)
        }
        Some("claude") => format!("claude-oauth:{}", model),
        Some("claude-api") => format!("claude-api:{}", model),
        Some("openai") => format!("openai-oauth:{}", model),
        Some("openai-api") => format!("openai-api:{}", model),
        Some("openrouter") | Some("jcode") => format!("openrouter:{}", model),
        Some("bedrock") => format!("bedrock:{}", model),
        Some("cursor") => format!("cursor:{}", model),
        Some("copilot") => format!("copilot:{}", model),
        Some("gemini") => format!("gemini:{}", model),
        Some("antigravity") => format!("antigravity:{}", model),
        _ => model.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let lock = crate::storage::lock_test_env();
            let saved = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            for key in keys {
                crate::env::remove_var(key);
            }
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }

    fn route(model: &str, provider: &str, api_method: &str, available: bool) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            provider: provider.to_string(),
            api_method: api_method.to_string(),
            available,
            detail: String::new(),
            cheapness: None,
        }
    }

    #[test]
    fn direct_auth_catalog_matching_preserves_oauth_vs_api_key_route_identity() {
        for (provider_id, provider_label, matching_provider, matching_method, stale_method) in [
            (
                "claude",
                "Anthropic/Claude",
                "Anthropic",
                "claude-oauth",
                "claude-api",
            ),
            (
                "claude-api",
                "Anthropic API",
                "Anthropic",
                "claude-api",
                "claude-oauth",
            ),
            (
                "openai",
                "OpenAI",
                "OpenAI",
                "openai-oauth",
                "openai-api-key",
            ),
            (
                "openai-api",
                "OpenAI API",
                "OpenAI",
                "openai-api-key",
                "openai-oauth",
            ),
        ] {
            let activation = AuthActivationResult {
                provider_id: Some(provider_id.to_string()),
                provider_label: Some(provider_label.to_string()),
                activated_model: Some("shared-model".to_string()),
                expected_runtime: None,
                expected_catalog_namespace: None,
            };
            let routes = vec![
                route("shared-model", matching_provider, stale_method, true),
                route("shared-model", matching_provider, matching_method, true),
            ];

            let report = validate_catalog_invariants(&activation, Some("shared-model"), &routes);
            assert!(
                report.ok(),
                "{provider_id} should match {matching_method}: {report:?}"
            );
            assert_eq!(report.selectable_provider_routes, 1);
            assert_eq!(
                report.route_sample,
                vec![format!("`shared-model` via {matching_method}")]
            );
            assert_eq!(
                provider_model_to_select_after_auth(&activation, Some("shared-model"), &routes),
                Some("shared-model".to_string()),
                "duplicate model IDs must force a provider-explicit model switch for {provider_id}"
            );
        }
    }

    #[test]
    fn typed_auth_request_provider_id_wins_over_legacy_hint() {
        let request = AuthActivationRequest::new(
            Some("openai".to_string()),
            Some(AuthChanged::new("cerebras")),
        );

        assert_eq!(request.provider_id().as_deref(), Some("cerebras"));
        assert_eq!(
            provider_display_label(request.provider_id().as_deref()).as_deref(),
            Some("Cerebras")
        );
    }

    #[test]
    fn direct_login_provider_ids_are_normalized_with_display_labels() {
        for (hint, normalized, label) in [
            ("claude", "claude", "Anthropic/Claude"),
            ("anthropic", "claude", "Anthropic/Claude"),
            ("anthropic-api", "claude-api", "Anthropic API"),
            ("claude-api", "claude-api", "Anthropic API"),
            ("openai", "openai", "OpenAI"),
            ("openai-key", "openai-api", "OpenAI API"),
            ("openrouter", "openrouter", "OpenRouter"),
            ("subscription", "jcode", "Jcode Subscription"),
            ("bedrock", "bedrock", "AWS Bedrock"),
            ("cursor", "cursor", "Cursor"),
            ("copilot", "copilot", "GitHub Copilot"),
            ("gemini", "gemini", "Google Gemini"),
            ("antigravity", "antigravity", "Antigravity"),
        ] {
            assert_eq!(normalized_auth_provider_id(Some(hint)), Some(normalized));
            assert_eq!(provider_display_label(Some(hint)).as_deref(), Some(label));
        }
    }

    #[test]
    fn every_model_login_provider_has_explicit_lifecycle_normalization() {
        let mut missing = Vec::new();
        for provider in crate::provider_catalog::login_providers() {
            let is_non_model_auth_surface = matches!(
                provider.target,
                crate::provider_catalog::LoginProviderTarget::AutoImport
                    | crate::provider_catalog::LoginProviderTarget::Google
            );
            let normalized = normalized_auth_provider_id(Some(provider.id));
            if is_non_model_auth_surface {
                assert!(
                    normalized.is_none(),
                    "non-model auth provider {} should stay out of model lifecycle normalization",
                    provider.id
                );
            } else if normalized.is_none() {
                missing.push(provider.id);
            }
        }

        assert!(
            missing.is_empty(),
            "model login providers missing lifecycle normalization: {:?}",
            missing
        );
    }

    #[test]
    fn direct_login_provider_activation_sets_runtime_identity_and_active_provider() {
        let _guard = EnvGuard::new(&[
            "JCODE_RUNTIME_PROVIDER",
            "JCODE_ACTIVE_PROVIDER",
            "JCODE_FORCE_PROVIDER",
            "JCODE_OPENROUTER_MODEL",
        ]);

        for (provider, runtime, active) in [
            ("claude", "claude", "claude"),
            ("claude-api", "claude-api", "claude"),
            ("openai", "openai", "openai"),
            ("openai-api", "openai-api", "openai"),
            ("openrouter", "openrouter", "openrouter"),
            ("jcode", "jcode", "openrouter"),
            ("bedrock", "bedrock", "bedrock"),
            ("cursor", "cursor", "cursor"),
            ("copilot", "copilot", "copilot"),
            ("gemini", "gemini", "gemini"),
            ("antigravity", "antigravity", "antigravity"),
        ] {
            crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
            crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
            crate::env::remove_var("JCODE_FORCE_PROVIDER");

            let activation = activate_auth_change(&AuthActivationRequest::new(
                None,
                Some(AuthChanged::new(provider)),
            ));

            assert_eq!(activation.provider_id.as_deref(), Some(provider));
            assert_eq!(
                std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
                Ok(runtime)
            );
            assert_eq!(
                std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
                Ok(active)
            );
            assert_eq!(std::env::var("JCODE_FORCE_PROVIDER").as_deref(), Ok("1"));
        }
    }

    #[test]
    fn direct_login_provider_descriptor_matrix_has_full_lifecycle_parity() {
        let _guard = EnvGuard::new(&[
            "JCODE_RUNTIME_PROVIDER",
            "JCODE_ACTIVE_PROVIDER",
            "JCODE_FORCE_PROVIDER",
            "JCODE_OPENROUTER_MODEL",
        ]);

        let mut covered = Vec::new();
        for provider in crate::provider_catalog::login_providers() {
            let Some((normalized, runtime, active, switch_prefix)) = (match provider.target {
                crate::provider_catalog::LoginProviderTarget::Jcode => {
                    Some(("jcode", "jcode", "openrouter", "openrouter"))
                }
                crate::provider_catalog::LoginProviderTarget::Claude => {
                    Some(("claude", "claude", "claude", "claude-oauth"))
                }
                crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                    Some(("claude-api", "claude-api", "claude", "claude-api"))
                }
                crate::provider_catalog::LoginProviderTarget::OpenAi => {
                    Some(("openai", "openai", "openai", "openai-oauth"))
                }
                crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                    Some(("openai-api", "openai-api", "openai", "openai-api"))
                }
                crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                    Some(("openrouter", "openrouter", "openrouter", "openrouter"))
                }
                crate::provider_catalog::LoginProviderTarget::Bedrock => {
                    Some(("bedrock", "bedrock", "bedrock", "bedrock"))
                }
                crate::provider_catalog::LoginProviderTarget::Cursor => {
                    Some(("cursor", "cursor", "cursor", "cursor"))
                }
                crate::provider_catalog::LoginProviderTarget::Copilot => {
                    Some(("copilot", "copilot", "copilot", "copilot"))
                }
                crate::provider_catalog::LoginProviderTarget::Gemini => {
                    Some(("gemini", "gemini", "gemini", "gemini"))
                }
                crate::provider_catalog::LoginProviderTarget::Antigravity => {
                    Some(("antigravity", "antigravity", "antigravity", "antigravity"))
                }
                _ => None,
            }) else {
                continue;
            };

            covered.push(provider.id);
            assert_eq!(
                normalized_auth_provider_id(Some(provider.id)),
                Some(normalized),
                "{} descriptor id must normalize into the auth lifecycle",
                provider.id
            );
            for alias in provider.aliases {
                assert_eq!(
                    normalized_auth_provider_id(Some(alias)),
                    Some(normalized),
                    "{} alias `{}` must normalize into the same auth lifecycle provider",
                    provider.id,
                    alias
                );
            }
            assert_eq!(
                provider_display_label(Some(provider.id)).as_deref(),
                Some(provider.display_name),
                "{} descriptor display label must be user-visible auth label",
                provider.id
            );

            crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
            crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
            crate::env::remove_var("JCODE_FORCE_PROVIDER");

            let activation = activate_auth_change(&AuthActivationRequest::new(
                None,
                Some(AuthChanged::new(provider.id)),
            ));
            assert_eq!(activation.provider_id.as_deref(), Some(normalized));
            assert_eq!(
                activation.provider_label.as_deref(),
                Some(provider.display_name)
            );
            assert_eq!(
                std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
                Ok(runtime)
            );
            assert_eq!(
                std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
                Ok(active)
            );
            assert_eq!(std::env::var("JCODE_FORCE_PROVIDER").as_deref(), Ok("1"));
            assert_eq!(
                activation.model_switch_request("ignored-runtime", "shared-model"),
                format!("{switch_prefix}:shared-model"),
                "{} direct auth model switch must stay provider-explicit",
                provider.id
            );
        }

        for expected in [
            "claude",
            "anthropic-api",
            "openai",
            "openai-api",
            "openrouter",
            "jcode",
            "bedrock",
            "cursor",
            "copilot",
            "gemini",
            "antigravity",
        ] {
            assert!(
                covered.contains(&expected),
                "direct provider parity matrix did not cover {expected}: {covered:?}"
            );
        }
    }

    #[test]
    fn model_switch_request_prefixes_openai_compatible_profiles_with_profile_id() {
        assert_eq!(
            model_switch_request_for_provider_id(Some("cerebras"), "mock-auth", "llama3.1-8b"),
            "cerebras:llama3.1-8b"
        );
        assert_eq!(
            model_switch_request_for_provider_id(Some("cerebras"), "openrouter", "llama3.1-8b"),
            "cerebras:llama3.1-8b"
        );
    }

    #[test]
    fn model_switch_request_is_provider_explicit_for_all_auth_providers() {
        for (provider, expected) in [
            ("claude", "claude-oauth:shared-model"),
            ("anthropic", "claude-oauth:shared-model"),
            ("anthropic-api", "claude-api:shared-model"),
            ("openai", "openai-oauth:shared-model"),
            ("openai-api", "openai-api:shared-model"),
            ("openrouter", "openrouter:shared-model"),
            ("jcode", "openrouter:shared-model"),
            ("azure-openai", "openrouter:shared-model"),
            ("bedrock", "bedrock:shared-model"),
            ("cursor", "cursor:shared-model"),
            ("copilot", "copilot:shared-model"),
            ("gemini", "gemini:shared-model"),
            ("antigravity", "antigravity:shared-model"),
            ("cerebras", "cerebras:shared-model"),
        ] {
            assert_eq!(
                model_switch_request_for_provider_id(Some(provider), "mock-auth", "shared-model"),
                expected,
                "{provider} auth switch request must route explicitly so duplicate model IDs cannot select the wrong provider"
            );
        }
    }

    #[test]
    fn post_auth_model_selection_reselects_duplicate_model_name_from_matching_provider_route() {
        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: Some("llama3.1-8b".to_string()),
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![
            route(
                "llama3.1-8b",
                "Other Gateway",
                "openai-compatible:other",
                true,
            ),
            route(
                "llama3.1-8b",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, Some("llama3.1-8b"), &routes),
            Some("llama3.1-8b".to_string()),
            "duplicate model IDs must force an explicit provider-profile model switch"
        );
    }

    #[test]
    fn catalog_invariants_pass_when_selected_model_matches_provider_route() {
        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: Some("llama3.1-8b".to_string()),
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai", true),
            route(
                "llama3.1-8b",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
        ];

        let report = validate_catalog_invariants(&activation, Some("llama3.1-8b"), &routes);

        assert!(
            report.ok(),
            "unexpected warning: {:?}",
            report.warning_message()
        );
        assert_eq!(report.selectable_provider_routes, 1);
    }

    #[test]
    fn catalog_invariants_reject_generic_openai_compatible_route_for_namespaced_auth() {
        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: Some("llama3.1-8b".to_string()),
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![route("llama3.1-8b", "Cerebras", "openai-compatible", true)];

        let report = validate_catalog_invariants(&activation, Some("llama3.1-8b"), &routes);

        assert!(
            !report.ok(),
            "generic openai-compatible route should not satisfy namespaced auth: {report:?}"
        );
        assert_eq!(report.selectable_provider_routes, 0);
        assert!(
            report
                .warning_message()
                .expect("warning")
                .contains("Expected selectable Cerebras model routes")
        );
    }

    #[test]
    fn catalog_invariants_warn_when_selected_model_is_from_stale_provider() {
        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: Some("llama3.1-8b".to_string()),
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![route("gpt-5.5", "OpenAI", "openai", true)];

        let report = validate_catalog_invariants(&activation, Some("gpt-5.5"), &routes);

        assert!(!report.ok());
        let warning = report.warning_message().expect("warning expected");
        assert!(warning.contains("Expected selectable Cerebras model routes"));
        assert!(warning.contains("Selected model: `gpt-5.5`"));
    }

    #[test]
    fn post_auth_model_selection_prefers_matching_provider_route_over_stale_model() {
        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: Some("qwen-3-235b-a22b-instruct-2507".to_string()),
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai", true),
            route(
                "qwen-3-235b-a22b-instruct-2507",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
            route(
                "llama3.1-8b",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, Some("gpt-5.5"), &routes).as_deref(),
            Some("qwen-3-235b-a22b-instruct-2507")
        );
        assert_eq!(
            provider_model_to_select_after_auth(
                &activation,
                Some("qwen-3-235b-a22b-instruct-2507"),
                &routes
            ),
            None
        );
    }

    #[test]
    fn post_auth_model_selection_prefers_anthropic_flagship_over_catalog_order() {
        // Live Anthropic catalogs list `claude-haiku-4-5-...` before
        // `claude-opus-4-8`, and an API-key login supplies no activated model.
        // Plain catalog order would auto-select Haiku; the flagship-first
        // fallback must land on Opus instead.
        let activation = AuthActivationResult {
            provider_id: Some("claude-api".to_string()),
            provider_label: Some("Anthropic".to_string()),
            activated_model: None,
            expected_runtime: None,
            expected_catalog_namespace: None,
        };
        let routes = vec![
            route("claude-haiku-4-5-20251001", "Anthropic", "claude-api", true),
            route("claude-opus-4-6", "Anthropic", "claude-api", true),
            route("claude-opus-4-8", "Anthropic", "claude-api", true),
            route("claude-sonnet-4-6", "Anthropic", "claude-api", true),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-opus-4-8"),
            "API-key login should auto-select the Anthropic flagship, not the first catalog route"
        );
    }

    #[test]
    fn post_auth_model_selection_prefers_claude_oauth_flagship() {
        let activation = AuthActivationResult {
            provider_id: Some("claude".to_string()),
            provider_label: Some("Anthropic".to_string()),
            activated_model: None,
            expected_runtime: None,
            expected_catalog_namespace: None,
        };
        let routes = vec![
            route("claude-haiku-4-5", "Anthropic", "claude-oauth", true),
            route("claude-opus-4-8", "Anthropic", "claude-oauth", true),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn post_auth_model_selection_prefers_openai_flagship_over_catalog_order() {
        let activation = AuthActivationResult {
            provider_id: Some("openai-api".to_string()),
            provider_label: Some("OpenAI".to_string()),
            activated_model: None,
            expected_runtime: None,
            expected_catalog_namespace: None,
        };
        let routes = vec![
            route("gpt-5.1", "OpenAI", "openai-api", true),
            route("gpt-5.5", "OpenAI", "openai-api", true),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn post_auth_model_selection_keeps_catalog_order_for_unranked_providers() {
        // OpenAI-compatible / namespaced providers have no curated flagship
        // order; the fallback must preserve live-catalog order for them.
        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: None,
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![
            route(
                "llama3.1-8b",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
            route(
                "qwen-3-235b-a22b-instruct-2507",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("llama3.1-8b"),
            "providers without a curated flagship order keep live-catalog order"
        );
    }

    #[test]
    fn post_auth_model_selection_prefers_newest_live_release_for_unranked_provider() {
        let _env = EnvGuard::new(&["JCODE_HOME"]);
        let temp = tempfile::tempdir().expect("tempdir");
        crate::env::set_var("JCODE_HOME", temp.path());
        jcode_provider_openrouter::save_disk_cache_with_source_for_namespace(
            "cerebras",
            &[
                jcode_provider_openrouter::ModelInfo {
                    id: "llama3.1-8b".to_string(),
                    name: String::new(),
                    context_length: None,
                    pricing: Default::default(),
                    created: Some(1_700_000_000),
                },
                jcode_provider_openrouter::ModelInfo {
                    id: "qwen-3-235b-a22b-instruct-2507".to_string(),
                    name: String::new(),
                    context_length: None,
                    pricing: Default::default(),
                    created: Some(1_800_000_000),
                },
            ],
            Some("https://api.cerebras.ai/v1"),
        );

        let activation = AuthActivationResult {
            provider_id: Some("cerebras".to_string()),
            provider_label: Some("Cerebras".to_string()),
            activated_model: None,
            expected_runtime: Some("openai-compatible".to_string()),
            expected_catalog_namespace: Some("cerebras".to_string()),
        };
        let routes = vec![
            route(
                "llama3.1-8b",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
            route(
                "qwen-3-235b-a22b-instruct-2507",
                "Cerebras",
                "openai-compatible:cerebras",
                true,
            ),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("qwen-3-235b-a22b-instruct-2507"),
            "unranked providers should prefer the newest live release when the catalog includes release timestamps"
        );
    }

    /// The set of canonical provider ids whose post-login fallback must apply a
    /// curated flagship-first order. These are the providers that expose
    /// Claude/OpenAI models under their bare canonical ids and report no
    /// `activated_model`, so a "cheap model first" catalog would otherwise
    /// auto-select the wrong default. Kept here as the single source of truth
    /// the exhaustive walk asserts against.
    const RANKED_PROVIDER_IDS: &[&str] = &[
        "claude",
        "claude-api",
        "openai",
        "openai-api",
        "copilot",
        "cursor",
    ];

    fn activation_for_provider_id(provider_id: &str) -> AuthActivationResult {
        AuthActivationResult {
            provider_id: Some(provider_id.to_string()),
            provider_label: provider_display_label(Some(provider_id)),
            activated_model: None,
            expected_runtime: None,
            expected_catalog_namespace: None,
        }
    }

    /// Exhaustive walk: every login provider descriptor is classified as ranked
    /// (curated flagship order) or unranked (catalog order), and the
    /// classification must exactly match RANKED_PROVIDER_IDS. This is the guard
    /// that catches a newly added provider that proxies Claude/OpenAI models but
    /// forgets to opt into the flagship-first fallback.
    #[test]
    fn post_auth_model_selection_classifies_every_login_provider() {
        let mut ranked_seen: std::collections::BTreeSet<String> = Default::default();
        for descriptor in crate::provider_catalog::login_providers() {
            let Some(provider_id) = normalized_auth_provider_id(Some(descriptor.id)) else {
                // AutoImport / non-runtime descriptors have no activation id.
                continue;
            };
            let activation = activation_for_provider_id(provider_id);
            let ranked = !provider_preferred_model_orders(&activation).is_empty();
            let expected = RANKED_PROVIDER_IDS.contains(&provider_id);
            assert_eq!(
                ranked, expected,
                "login provider `{}` (id `{}`) classified ranked={ranked}, expected {expected}; \
                 if this is a new Claude/OpenAI-proxying provider add it to \
                 provider_preferred_model_orders + RANKED_PROVIDER_IDS, otherwise leave it unranked",
                descriptor.id, provider_id
            );
            if ranked {
                ranked_seen.insert(provider_id.to_string());
            }
        }
        let expected_ranked: std::collections::BTreeSet<String> = RANKED_PROVIDER_IDS
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(
            ranked_seen, expected_ranked,
            "the ranked providers reachable from the login catalog drifted from RANKED_PROVIDER_IDS"
        );
    }

    /// Exhaustive walk: for every ranked provider, an adversarial catalog that
    /// lists the cheapest model first must still auto-select the curated
    /// flagship after login. This is the direct regression for the live
    /// Anthropic API-key login that auto-selected Haiku instead of Opus.
    #[test]
    fn post_auth_model_selection_picks_flagship_for_every_ranked_provider() {
        // (provider_id, api_method, provider_display, cheap_first_routes, expected flagship)
        let cases: &[(&str, &str, &str, &[&str], &str)] = &[
            (
                "claude",
                "claude-oauth",
                "Anthropic",
                &["claude-haiku-4-5", "claude-sonnet-4-6", "claude-opus-4-8"],
                "claude-opus-4-8",
            ),
            (
                "claude-api",
                "claude-api",
                "Anthropic",
                &[
                    "claude-haiku-4-5-20251001",
                    "claude-sonnet-4-6",
                    "claude-opus-4-8",
                ],
                "claude-opus-4-8",
            ),
            (
                "openai",
                "openai-oauth",
                "OpenAI",
                &["gpt-5-nano", "gpt-5.1", "gpt-5.5"],
                "gpt-5.5",
            ),
            (
                "openai-api",
                "openai-api-key",
                "OpenAI",
                &["gpt-5-mini", "gpt-5.1", "gpt-5.5"],
                "gpt-5.5",
            ),
            (
                // Copilot proxies Claude under canonical ids: Opus must beat Haiku.
                "copilot",
                "copilot",
                "Copilot",
                &["claude-haiku-4-5", "gpt-5.5", "claude-opus-4-8"],
                "claude-opus-4-8",
            ),
            (
                // Cursor likewise: an all-OpenAI catalog still picks the flagship.
                "cursor",
                "cursor",
                "Cursor",
                &["gpt-5-nano", "gpt-5.1", "gpt-5.5"],
                "gpt-5.5",
            ),
        ];

        // Guard: the hand-written cases must cover every ranked provider, or the
        // "for_every_ranked_provider" claim silently rots when a new ranked
        // provider is added without a matching case.
        let covered: std::collections::BTreeSet<&str> =
            cases.iter().map(|(provider_id, ..)| *provider_id).collect();
        let expected_covered: std::collections::BTreeSet<&str> =
            RANKED_PROVIDER_IDS.iter().copied().collect();
        assert_eq!(
            covered, expected_covered,
            "flagship cases drifted from RANKED_PROVIDER_IDS; add a cheap-first case for any \
             newly ranked provider so its flagship selection is actually exercised"
        );

        for (provider_id, api_method, provider_display, models, expected) in cases {
            let activation = activation_for_provider_id(provider_id);
            let routes: Vec<ModelRoute> = models
                .iter()
                .map(|model| route(model, provider_display, api_method, true))
                .collect();
            assert_eq!(
                provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
                Some(*expected),
                "provider `{provider_id}` should auto-select flagship `{expected}` from a \
                 cheap-first catalog, not the first route `{}`",
                models[0]
            );
        }
    }

    /// Copilot proxies both families; the cross-family tie-break must prefer the
    /// Claude flagship over the OpenAI flagship to mirror jcode's default model.
    #[test]
    fn post_auth_model_selection_copilot_prefers_claude_family_over_openai() {
        let activation = activation_for_provider_id("copilot");
        let routes = vec![
            route("gpt-5.5", "Copilot", "copilot", true),
            route("claude-opus-4-8", "Copilot", "copilot", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-opus-4-8"),
            "copilot tie-break should prefer the Claude flagship family first"
        );
    }
}
