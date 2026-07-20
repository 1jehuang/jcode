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
        // 1. Auto-promote a brand-new frontier release that is not yet in the
        //    curated list. Model ids in these families encode their version
        //    (`gpt-5.5`, `claude-opus-4-8`), so a strictly-newer *pure* flagship
        //    id (no cheap/specialized suffix) should become the default the day
        //    it ships, without waiting for a code change.
        if let Some(newer) = newest_frontier_release(activation, &matching_routes) {
            return Some(newer);
        }
        // 2. Otherwise pick the best curated model by quality order.
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

/// Pick the strongest available route across every authenticated provider.
///
/// This is intentionally separate from [`provider_model_to_select_after_auth`],
/// which keeps normal re-authentication scoped to the provider that changed.
/// First-run onboarding can use this global selector after importing multiple
/// accounts. Returning the complete route preserves OAuth/API-key/profile
/// identity when the caller applies the selection.
pub fn globally_preferred_default_route(routes: &[ModelRoute]) -> Option<ModelRoute> {
    routes
        .iter()
        .enumerate()
        .filter(|(_, route)| route.available)
        .min_by_key(|(catalog_index, route)| {
            (globally_preferred_model_rank(&route.model), *catalog_index)
        })
        .map(|(_, route)| route.clone())
}

fn globally_preferred_model_rank(model: &str) -> (u8, usize) {
    let normalized = normalize_model_for_preference(model);
    let openai_default = normalize_model_for_preference(jcode_provider_core::DEFAULT_OPENAI_MODEL);
    let claude_default = normalize_model_for_preference(jcode_provider_core::DEFAULT_CLAUDE_MODEL);

    if normalized == openai_default {
        return (0, 0);
    }
    // Some catalogs expose the clean release id instead of jcode's Sol route.
    if normalized == "gpt-5.6" {
        return (1, 0);
    }
    if normalized == claude_default {
        return (2, 0);
    }
    if let Some(position) = crate::provider::ALL_CLAUDE_MODELS
        .iter()
        .position(|candidate| normalize_model_for_preference(candidate) == normalized)
    {
        return (3, position);
    }
    if let Some(position) = crate::provider::ALL_OPENAI_MODELS
        .iter()
        .position(|candidate| normalize_model_for_preference(candidate) == normalized)
    {
        return (4, position);
    }

    // Unknown provider families retain catalog order as the final fallback.
    (5, usize::MAX)
}

/// Curated flagship-first order for Bedrock-hosted models. Bedrock ids carry a
/// vendor prefix (`anthropic.claude-opus-4-...`, `us.anthropic.claude-...`) which
/// `parse_frontier_model`/`normalize_model_for_preference` strip before matching,
/// so the bare canonical ids here line up with the live route ids. Claude Opus
/// is the flagship, then Sonnet, then Nova/Llama/Mistral, then Haiku/cheap.
const ALL_BEDROCK_MODELS: &[&str] = &[
    "claude-opus-4",
    "claude-sonnet-4",
    "claude-3-7-sonnet",
    "claude-3-5-sonnet",
    "amazon.nova-pro",
    "meta.llama3-1-405b-instruct",
    "mistral.mistral-large",
    "claude-3-5-haiku",
    "amazon.nova-lite",
    "amazon.nova-micro",
];

/// Curated flagship-first order for Gemini (Code Assist OAuth + Gemini API).
/// `pro` is Gemini's flagship tier and `flash`/`lite` are the cheaper tiers, so
/// (unlike Claude/OpenAI) `pro` must NOT be treated as a non-flagship marker for
/// this family. Listed newest-and-strongest first.
const ALL_GEMINI_MODELS: &[&str] = &[
    "gemini-3.1-pro",
    "gemini-3-pro",
    "gemini-2.5-pro",
    "gemini-1.5-pro",
    "gemini-3-flash",
    "gemini-2.5-flash",
    "gemini-2.0-flash",
    "gemini-1.5-flash",
];

/// Flagship-first preference tiers used only to break ties when falling back to
/// an arbitrary matching route after a login. Each inner slice is one curated
/// family ordered best-first; earlier families outrank later ones. Returns an
/// empty slice for providers without a curated order (local OpenAI-compatible,
/// raw OpenRouter, ...), which preserves live-catalog order.
///
/// Copilot and Cursor proxy Claude/OpenAI models under their bare canonical ids
/// (`copilot:claude-opus-4-8`), so they share the same "catalog lists the cheap
/// model first" hazard as a direct login and get the combined Claude+OpenAI
/// order. The Claude/OpenAI subscription default bias mirrors jcode's global
/// default model. Bedrock/Azure/Gemini/Antigravity are native hosted catalogs
/// whose route lists are often ordered oldest-first, so they get an explicit
/// curated order too.
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
        Some("bedrock") => &[ALL_BEDROCK_MODELS],
        // Azure hosts the OpenAI family.
        Some("azure-openai") => &[crate::provider::ALL_OPENAI_MODELS],
        // Gemini (Code Assist OAuth) and Antigravity both serve Gemini models.
        Some("gemini") | Some("antigravity") => &[ALL_GEMINI_MODELS],
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
/// `[1m]` long-context suffix, strip a trailing 8-digit `-YYYYMMDD` date so live
/// dated ids (`claude-haiku-4-5-20251001`) match bare canonical ids
/// (`claude-haiku-4-5`), and strip hosted-vendor prefixes/suffixes so Bedrock and
/// proxy ids line up with the curated bare ids.
///
/// Examples:
///   `us.anthropic.claude-opus-4-20250514-v1:0` -> `claude-opus-4`
///   `anthropic.claude-3-5-sonnet-20241022-v2:0` -> `claude-3-5-sonnet`
///   `accounts/fireworks/models/qwen3-coder` -> `qwen3-coder`
///   `models/gemini-3-pro-preview` -> `gemini-3-pro`
fn normalize_model_for_preference(model: &str) -> String {
    let mut id = jcode_provider_core::model_id::canonical(model);

    // Drop a `/`-qualified path prefix (`accounts/x/models/y`, `models/gemini`).
    if let Some(idx) = id.rfind('/') {
        id = id[idx + 1..].to_string();
    }

    // Drop a trailing Bedrock version tag (`-v1:0`, `-v2:0`, `:0`).
    if let Some(idx) = id.find(":0") {
        id = id[..idx].to_string();
    }
    if let Some(stripped) = strip_trailing_bedrock_version(&id) {
        id = stripped;
    }

    // Drop a trailing release-date suffix.
    id = jcode_provider_core::model_id::strip_date_suffix(&id).to_string();

    // Drop a trailing `-preview`/`-exp`/`-latest` marketing suffix so Gemini
    // preview ids match their canonical family entry.
    for suffix in ["-preview", "-exp", "-latest"] {
        if let Some(base) = id.strip_suffix(suffix) {
            id = base.to_string();
        }
    }

    // Drop a leading hosted-vendor segment (`anthropic.`, `us.anthropic.`,
    // `meta.`, `amazon.`, `mistral.`) so `anthropic.claude-opus-4` matches the
    // curated `claude-opus-4`. Keep `amazon.nova`/`meta.llama`/`mistral.` whole
    // because those families are listed with their vendor prefix in
    // `ALL_BEDROCK_MODELS`; only strip the region + the redundant `anthropic.`.
    id = strip_bedrock_region_prefix(&id);
    if let Some(rest) = id.strip_prefix("anthropic.") {
        id = rest.to_string();
    }

    id
}

/// Strip a leading Bedrock region routing segment (`us.`, `eu.`, `apac.`,
/// `us-gov.`) from a model id.
fn strip_bedrock_region_prefix(id: &str) -> String {
    for region in ["us-gov.", "us.", "eu.", "apac.", "ap.", "global."] {
        if let Some(rest) = id.strip_prefix(region) {
            return rest.to_string();
        }
    }
    id.to_string()
}

/// Strip a trailing Bedrock version tag like `-v1`, `-v2` (after the `:0` has
/// already been removed). Returns `None` when there is no such tag.
fn strip_trailing_bedrock_version(id: &str) -> Option<String> {
    let (head, tail) = id.rsplit_once('-')?;
    let is_version_tag =
        tail.len() >= 2 && tail.starts_with('v') && tail[1..].chars().all(|c| c.is_ascii_digit());
    is_version_tag.then(|| head.to_string())
}

/// A parsed "frontier flagship" model id: its family prefix (e.g. `claude-opus`
/// or `gpt`) plus an ordered version vector parsed from the trailing numeric
/// components, used to compare releases within a family.
///
/// Only *pure* flagship ids parse successfully: ids carrying a cheaper or
/// specialized tier word (`mini`, `nano`, `haiku`, `flash`, `codex`, `pro`,
/// `chat`, ...) are rejected so a new cheap/specialized model never auto-promotes
/// over the flagship.
struct FrontierModel {
    family: String,
    version: Vec<u64>,
}

/// Tier/specialization words that disqualify an id from frontier auto-promotion
/// for the default (Claude/OpenAI-style) families. These mark cheaper or
/// non-default variants that must never outrank a clean flagship id purely
/// because they share a (possibly higher) version number.
///
/// NOTE: this list is used for families whose flagship id is *bare* (no tier
/// word), e.g. `claude-opus-4-8`, `gpt-5.5`. Families like Gemini, whose flagship
/// id contains a tier word (`gemini-3-pro`), use a family-specific rule instead
/// (see [`frontier_families`]).
const NON_FLAGSHIP_TIER_WORDS: &[&str] = &[
    "mini", "nano", "haiku", "flash", "lite", "small", "tiny", "instant", "codex", "pro", "chat",
    "audio", "realtime", "image", "tts", "embed", "search", "guard", "deep", "thinking",
];

/// One frontier-eligible family: the canonical id prefix to match (after
/// [`normalize_model_for_preference`]) and, optionally, a required flagship tier
/// token that must be present (e.g. Gemini's `pro`). When `flagship_token` is
/// `Some`, version parsing ignores that token and rejects any *other* trailing
/// word; when `None`, the id must be bare (Claude/OpenAI style) and is rejected
/// if it contains any [`NON_FLAGSHIP_TIER_WORDS`].
#[derive(Clone, Copy)]
struct FrontierFamily {
    prefix: &'static str,
    flagship_token: Option<&'static str>,
}

/// Family descriptors eligible for frontier auto-promotion per provider. Only the
/// strongest family per provider is listed (Claude Opus, GPT base, Gemini Pro) so
/// we never auto-promote a cheaper family/tier over the curated flagship.
fn frontier_families(activation: &AuthActivationResult) -> &'static [FrontierFamily] {
    const FABLE: FrontierFamily = FrontierFamily {
        prefix: "claude-fable",
        flagship_token: None,
    };
    const CLAUDE: FrontierFamily = FrontierFamily {
        prefix: "claude-opus",
        flagship_token: None,
    };
    const GPT: FrontierFamily = FrontierFamily {
        prefix: "gpt",
        flagship_token: None,
    };
    const GEMINI: FrontierFamily = FrontierFamily {
        prefix: "gemini",
        flagship_token: Some("pro"),
    };
    match activation.provider_id.as_deref() {
        Some("claude") | Some("claude-api") => &[FABLE, CLAUDE],
        Some("openai") | Some("openai-api") | Some("azure-openai") => &[GPT],
        // Copilot/Cursor proxy both families under canonical ids.
        Some("copilot") | Some("cursor") => &[FABLE, CLAUDE, GPT],
        // Bedrock hosts Claude under `anthropic.claude-opus-...` (prefix stripped
        // by normalize), so the Claude family applies.
        Some("bedrock") => &[CLAUDE],
        Some("gemini") | Some("antigravity") => &[GEMINI],
        _ => &[],
    }
}

/// Parse a model id into a [`FrontierModel`] if it is a clean flagship id for one
/// of `families`. Returns `None` for non-matching families, ids with a
/// non-flagship tier word, or ids without a parseable version.
fn parse_frontier_model(model: &str, families: &[FrontierFamily]) -> Option<FrontierModel> {
    let normalized = normalize_model_for_preference(model);
    // Find the family this id belongs to (longest prefix wins so `claude-opus`
    // is preferred over a hypothetical `claude`).
    let family = families
        .iter()
        .filter(|fam| normalized.starts_with(fam.prefix))
        .max_by_key(|fam| fam.prefix.len())?;

    let version = parse_frontier_version_for_family(&normalized, *family, false)?;
    Some(FrontierModel {
        family: family.prefix.to_string(),
        version,
    })
}

/// Parse a model version within one known frontier family. Curated defaults may
/// carry a quality-profile suffix such as `gpt-5.6-sol`; when `allow_suffix` is
/// true, the leading numeric release is still used as the promotion baseline.
/// Live promotion candidates remain strict, so cheap/specialized variants never
/// self-promote merely because they contain a higher version number.
fn parse_frontier_version_for_family(
    normalized_model: &str,
    family: FrontierFamily,
    allow_suffix: bool,
) -> Option<Vec<u64>> {
    if !normalized_model.starts_with(family.prefix) {
        return None;
    }

    match family.flagship_token {
        None => {
            // Bare-flagship family (Claude/OpenAI): reject any tier word, then
            // require a pure-numeric remainder after the prefix.
            if !allow_suffix
                && NON_FLAGSHIP_TIER_WORDS
                    .iter()
                    .any(|word| normalized_model.contains(word))
            {
                return None;
            }
            let remainder = normalized_model[family.prefix.len()..].trim_matches(['-', '.', ' ']);
            if !allow_suffix {
                return parse_version_components(remainder);
            }
            let mut version = Vec::new();
            for part in remainder.split(['.', '-']) {
                if part.is_empty() {
                    continue;
                }
                match part.parse::<u64>() {
                    Ok(number) => version.push(number),
                    Err(_) if !version.is_empty() => break,
                    Err(_) => return None,
                }
            }
            (!version.is_empty()).then_some(version)
        }
        Some(flagship_token) => {
            // Flagship-token family (Gemini): the id must contain the flagship
            // token and nothing else but version numbers + that token. Reject any
            // other word (e.g. `flash`, `lite`).
            let remainder = normalized_model[family.prefix.len()..].trim_matches(['-', '.', ' ']);
            if remainder.is_empty() {
                return None;
            }
            let mut version = Vec::new();
            let mut saw_flagship_token = false;
            for part in remainder.split(['.', '-']) {
                if part.is_empty() {
                    continue;
                }
                if part == flagship_token {
                    saw_flagship_token = true;
                    continue;
                }
                let number: u64 = part.parse().ok()?; // any other word => reject
                version.push(number);
            }
            if !saw_flagship_token || version.is_empty() {
                return None;
            }
            Some(version)
        }
    }
}

/// Parse a dash/dot-separated numeric version remainder (`4-8`, `5.5`) into a
/// component vector. Returns `None` if any component is non-numeric or empty.
fn parse_version_components(remainder: &str) -> Option<Vec<u64>> {
    if remainder.is_empty() {
        return None;
    }
    let mut version = Vec::new();
    for part in remainder.split(['.', '-']) {
        if part.is_empty() {
            continue;
        }
        version.push(part.parse::<u64>().ok()?);
    }
    (!version.is_empty()).then_some(version)
}

/// Compare two version vectors component-wise (semver-like). Missing trailing
/// components are treated as 0 so `[5]` < `[5, 1]`.
fn version_cmp(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Auto-detect a brand-new frontier flagship release among `routes` that is
/// strictly newer than the curated baseline flagship for the same family, and is
/// not already the curated #1. Returns the chosen model id, or `None` when the
/// curated default is still the newest known flagship.
///
/// This is the new-release robustness layer: the day Anthropic ships
/// `claude-opus-4-9` or OpenAI ships `gpt-5.6`, the live catalog will carry it
/// and it auto-promotes to the post-login default without a code change, while
/// cheaper/specialized variants are excluded by [`parse_frontier_model`].
fn newest_frontier_release(
    activation: &AuthActivationResult,
    routes: &[&ModelRoute],
) -> Option<String> {
    let families = frontier_families(activation);
    if families.is_empty() {
        return None;
    }

    // Only auto-promote within the strongest frontier family that is actually
    // present. This prevents a newer lower-priority Opus id from displacing an
    // available Fable default, while still allowing Opus promotion when Fable is
    // absent and GPT promotion when Sol is the active quality profile.
    let active_family = families.iter().copied().find(|family| {
        routes.iter().any(|route| {
            let normalized = normalize_model_for_preference(&route.model);
            parse_frontier_version_for_family(&normalized, *family, true).is_some()
        })
    })?;

    // Baseline: the strongest *available* curated release in the active family.
    // If the preferred profile (for example Sol 5.6) is unavailable, a clean
    // model at that release may still promote over the next available fallback
    // (for example GPT 5.5). When no curated route is available at all, fall back
    // to the shipped catalog baseline so an old unknown id cannot self-promote.
    let curated_baseline = |family: FrontierFamily| -> Option<Vec<u64>> {
        let orders = provider_preferred_model_orders(activation);
        let available_baseline = routes
            .iter()
            .filter(|route| preferred_model_rank(orders, &route.model) != usize::MAX)
            .filter_map(|route| {
                let normalized = normalize_model_for_preference(&route.model);
                parse_frontier_version_for_family(&normalized, family, true)
            })
            .max_by(|a, b| version_cmp(a, b));
        available_baseline.or_else(|| {
            orders
                .iter()
                .flat_map(|order| order.iter())
                .filter_map(|id| {
                    let normalized = normalize_model_for_preference(id);
                    parse_frontier_version_for_family(&normalized, family, true)
                })
                .max_by(|a, b| version_cmp(a, b))
        })
    };

    let mut best: Option<(FrontierModel, String)> = None;
    for route in routes {
        let Some(parsed) = parse_frontier_model(&route.model, families) else {
            continue;
        };
        if parsed.family != active_family.prefix {
            continue;
        }
        // Must strictly beat the curated baseline for its family.
        let Some(baseline) = curated_baseline(active_family) else {
            continue;
        };
        if version_cmp(&parsed.version, &baseline) != std::cmp::Ordering::Greater {
            continue;
        }
        // Among qualifying releases, keep the highest version (preferring the
        // strongest family by the order in `families` on ties).
        let is_better = match &best {
            None => true,
            Some((current, _)) => match version_cmp(&parsed.version, &current.version) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Equal => {
                    // Tie on version: prefer the family listed earlier (stronger).
                    let rank = |fam: &str| {
                        families
                            .iter()
                            .position(|f| f.prefix == fam)
                            .unwrap_or(usize::MAX)
                    };
                    rank(&parsed.family) < rank(&current.family)
                }
                std::cmp::Ordering::Less => false,
            },
        };
        if is_better {
            best = Some((parsed, route.model.clone()));
        }
    }

    best.map(|(_, model)| model)
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
        "gemini" => {
            // Gemini's Code Assist OAuth routes carry the `code-assist-oauth`
            // api_method (not the bare provider id), so match on the parsed kind
            // like the other native credential routes above.
            return matches!(
                api_method,
                crate::provider::ModelRouteApiMethod::CodeAssistOAuth
            );
        }
        "jcode" => {
            // Jcode subscription routes deliberately keep their managed public
            // identity even though the runtime reuses OpenRouter transport code.
            return matches!(
                api_method,
                crate::provider::ModelRouteApiMethod::JcodeSubscription
            );
        }
        "azure-openai" => {
            // Azure OpenAI reuses the OpenRouter transport (configured via Azure
            // env), so its routes carry the `openrouter` api_method while keeping
            // the `azure-openai` runtime identity.
            return matches!(api_method, crate::provider::ModelRouteApiMethod::OpenRouter);
        }
        _ => {}
    }

    // OpenAI-compatible auth has a concrete catalog namespace. Accepting a
    // matching display label or generic `openai-compatible` route as success can
    // hide stale/mixed catalogs, especially when providers share model IDs.
    // Legacy/local TUI auth notifications only carry the provider label, so
    // derive this constraint from the normalized profile id as well as explicit
    // protocol metadata.
    if crate::provider_catalog::openai_compatible_profile_by_id(provider_id).is_some()
        || activation.expected_runtime.as_deref() == Some("openai-compatible")
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

/// Pick the preferred first-run provider when the machine already has working
/// OpenAI and/or Anthropic credentials. Keep this aligned with
/// `jcode_provider_core::auto_default_provider`: Claude wins when both families
/// are available, and OAuth wins over an API key within the same family.
///
/// Returning the auth-specific provider id (`openai` vs `openai-api`, `claude`
/// vs `claude-api`) matters because post-login activation uses it to select the
/// matching route and its strongest available model.
pub fn preferred_frontier_auth_provider(status: &crate::auth::AuthStatus) -> Option<&'static str> {
    use crate::auth::AuthState;

    if status.anthropic.state == AuthState::Available {
        if status.anthropic.has_oauth && status.anthropic.oauth_state == AuthState::Available {
            return Some("claude");
        }
        if status.anthropic.has_api_key {
            return Some("claude-api");
        }
        return Some("claude");
    }

    if status.openai == AuthState::Available {
        if status.openai_has_oauth && status.openai_oauth_state == AuthState::Available {
            return Some("openai");
        }
        if status.openai_has_api_key {
            return Some("openai-api");
        }
        // Preserve compatibility with older/partial status snapshots that only
        // populated the aggregate state.
        return Some("openai");
    }

    None
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
    sync_process_env_from_saved_credentials(request, provider_id.as_deref());
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

/// Env keys and env-file names that persist API-key credentials for a
/// normalized auth provider id. Empty for OAuth/CLI providers whose
/// credentials live in token stores, not env vars.
fn api_key_env_bindings_for_provider(provider_id: &str) -> Vec<(String, String)> {
    match provider_id {
        "claude-api" => vec![("ANTHROPIC_API_KEY".to_string(), "anthropic.env".to_string())],
        "openai-api" => vec![("OPENAI_API_KEY".to_string(), "openai.env".to_string())],
        "openrouter" => vec![(
            "OPENROUTER_API_KEY".to_string(),
            "openrouter.env".to_string(),
        )],
        "jcode" => vec![
            (
                crate::subscription_catalog::JCODE_API_KEY_ENV.to_string(),
                crate::subscription_catalog::JCODE_ENV_FILE.to_string(),
            ),
            (
                crate::subscription_catalog::JCODE_API_BASE_ENV.to_string(),
                crate::subscription_catalog::JCODE_ENV_FILE.to_string(),
            ),
        ],
        "bedrock" => vec![
            (
                crate::provider::bedrock::API_KEY_ENV.to_string(),
                crate::provider::bedrock::ENV_FILE.to_string(),
            ),
            (
                crate::provider::bedrock::REGION_ENV.to_string(),
                crate::provider::bedrock::ENV_FILE.to_string(),
            ),
        ],
        "cursor" => vec![("CURSOR_API_KEY".to_string(), "cursor.env".to_string())],
        "gemini" => super::gemini::GEMINI_API_KEY_ENV_VARS
            .iter()
            .map(|env_key| {
                (
                    env_key.to_string(),
                    super::gemini::GEMINI_API_KEY_ENV_FILE.to_string(),
                )
            })
            .collect(),
        "azure-openai" => vec![
            (
                super::azure::API_KEY_ENV.to_string(),
                super::azure::ENV_FILE.to_string(),
            ),
            (
                super::azure::ENDPOINT_ENV.to_string(),
                super::azure::ENV_FILE.to_string(),
            ),
            (
                super::azure::MODEL_ENV.to_string(),
                super::azure::ENV_FILE.to_string(),
            ),
        ],
        other => crate::provider_catalog::openai_compatible_profile_by_id(other)
            .map(|profile| {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                vec![(resolved.api_key_env, resolved.env_file)]
            })
            .unwrap_or_default(),
    }
}

/// Make freshly saved credentials win over stale env vars inherited by this
/// process (issue #453).
///
/// `/login` persists API keys to the per-provider env file under the jcode
/// config dir, but credential resolution
/// ([`crate::provider_catalog::load_api_key_from_env_or_config`]) prefers the
/// process env var. A long-lived server that inherited a stale
/// `ANTHROPIC_API_KEY` (or similar) would therefore keep 401-ing forever even
/// though the login succeeded and the file holds a valid key. On an explicit
/// auth change, overwrite this process's env var with the env-file value when
/// the two diverge, so the just-saved credential is actually used.
fn sync_process_env_from_saved_credentials(
    request: &AuthActivationRequest,
    provider_id: Option<&str>,
) {
    let Some(provider_id) = provider_id else {
        return;
    };
    // Only do this for auth changes that plausibly wrote an env file. OAuth
    // logins do not touch API-key env files, and process-env-preseeded auth
    // means the env var itself is the intended source of truth.
    let explicit_env_file_login = match request.auth.as_ref() {
        Some(auth) => {
            matches!(
                auth.credential_source,
                Some(crate::protocol::AuthCredentialSource::ApiKeyFile) | None
            ) && !matches!(
                auth.auth_method,
                Some(crate::protocol::AuthMethod::ProcessEnvPreseeded)
                    | Some(crate::protocol::AuthMethod::OAuthBrowser)
                    | Some(crate::protocol::AuthMethod::DeviceCode)
            )
        }
        // Legacy hint-only notifications carry no source metadata; syncing is
        // still the safe default because it only runs when the file has a
        // value that differs from the env var.
        None => true,
    };
    if !explicit_env_file_login {
        return;
    }

    for (env_key, env_file) in api_key_env_bindings_for_provider(provider_id) {
        let Some(file_value) =
            crate::provider_catalog::load_env_value_from_config_file(&env_key, &env_file)
        else {
            continue;
        };
        let env_value = std::env::var(&env_key).ok();
        if env_value.as_deref() == Some(file_value.as_str()) {
            continue;
        }
        let had_stale_env = env_value.is_some();
        crate::env::set_var(&env_key, &file_value);
        crate::logging::auth_event(
            "auth_changed_env_synced_from_file",
            provider_id,
            &[
                ("env_key", env_key.as_str()),
                ("env_file", env_file.as_str()),
                (
                    "replaced",
                    if had_stale_env {
                        "stale_process_env"
                    } else {
                        "unset_process_env"
                    },
                ),
            ],
        );
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
        Some("openrouter") => format!("openrouter:{}", model),
        Some("jcode") => model.to_string(),
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
    fn api_key_login_replaces_stale_process_env_with_saved_file_key() {
        // Issue #453: a server process that inherited a stale ANTHROPIC_API_KEY
        // must start using the key that /login just wrote to anthropic.env.
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        crate::env::set_var("ANTHROPIC_API_KEY", "stale-inherited-key");
        sandbox
            .write_env_file("anthropic.env", "ANTHROPIC_API_KEY", "fresh-login-key")
            .expect("write env file");

        let mut auth = AuthChanged::new("claude-api");
        auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
        auth.auth_method = Some(crate::protocol::AuthMethod::TuiPasteApiKey);
        let _ = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));

        assert_eq!(
            std::env::var("ANTHROPIC_API_KEY").as_deref(),
            Ok("fresh-login-key")
        );
        assert_eq!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                "ANTHROPIC_API_KEY",
                "anthropic.env"
            )
            .as_deref(),
            Some("fresh-login-key"),
            "credential resolution must use the freshly saved key"
        );
    }

    #[test]
    fn legacy_hint_only_auth_change_still_syncs_saved_file_key() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        crate::env::set_var("ANTHROPIC_API_KEY", "stale-inherited-key");
        sandbox
            .write_env_file("anthropic.env", "ANTHROPIC_API_KEY", "fresh-login-key")
            .expect("write env file");

        let _ = activate_auth_change(&AuthActivationRequest::new(
            Some("anthropic-api".to_string()),
            None,
        ));

        assert_eq!(
            std::env::var("ANTHROPIC_API_KEY").as_deref(),
            Ok("fresh-login-key")
        );
    }

    #[test]
    fn oauth_auth_change_does_not_touch_api_key_process_env() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        crate::env::set_var("ANTHROPIC_API_KEY", "env-key-left-alone");
        sandbox
            .write_env_file("anthropic.env", "ANTHROPIC_API_KEY", "file-key")
            .expect("write env file");

        let mut auth = AuthChanged::new("claude-api");
        auth.auth_method = Some(crate::protocol::AuthMethod::OAuthBrowser);
        auth.credential_source = Some(crate::protocol::AuthCredentialSource::OAuthTokenStore);
        let _ = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));

        assert_eq!(
            std::env::var("ANTHROPIC_API_KEY").as_deref(),
            Ok("env-key-left-alone")
        );
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
        // Sandbox JCODE_HOME so activation's env-file credential sync (#453)
        // cannot read the developer's real ~/.config/jcode/*.env files and
        // leak keys into this process during the matrix run.
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");

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
        // Sandbox JCODE_HOME for the same reason as the activation matrix
        // above: keep the #453 credential sync away from real env files.
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");

        let mut covered = Vec::new();
        for provider in crate::provider_catalog::login_providers() {
            let Some((normalized, runtime, active, switch_prefix)) = (match provider.target {
                crate::provider_catalog::LoginProviderTarget::Jcode => {
                    Some(("jcode", "jcode", "openrouter", ""))
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
            let expected_switch = if switch_prefix.is_empty() {
                "shared-model".to_string()
            } else {
                format!("{switch_prefix}:shared-model")
            };
            assert_eq!(
                activation.model_switch_request("ignored-runtime", "shared-model"),
                expected_switch,
                "{} direct auth model switch must preserve its canonical route identity",
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
            ("jcode", "shared-model"),
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
    fn jcode_auth_lifecycle_matches_only_managed_subscription_routes() {
        let activation = AuthActivationResult {
            provider_id: Some("jcode".to_string()),
            provider_label: Some("Jcode Subscription".to_string()),
            activated_model: Some("gpt-5.5".to_string()),
            expected_runtime: Some("jcode-subscription".to_string()),
            expected_catalog_namespace: Some("jcode-subscription".to_string()),
        };
        let routes = vec![
            route("gpt-5.5", "OpenRouter", "openrouter", true),
            route(
                "gpt-5.5",
                "Jcode Subscription",
                crate::subscription_catalog::JCODE_ROUTE_API_METHOD,
                true,
            ),
        ];

        let report = validate_catalog_invariants(&activation, Some("gpt-5.5"), &routes);
        assert!(
            report.ok(),
            "canonical Jcode route should match: {report:?}"
        );
        assert_eq!(report.selectable_provider_routes, 1);
        assert_eq!(
            report.route_sample,
            vec![format!(
                "`gpt-5.5` via {}",
                crate::subscription_catalog::JCODE_ROUTE_API_METHOD
            )]
        );
        assert_eq!(
            activation.model_switch_request("Jcode Subscription", "gpt-5.5"),
            "gpt-5.5"
        );
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
        // Live Anthropic catalogs list `claude-haiku-4-5-...` before the
        // flagship, and an API-key login supplies no activated model. Plain
        // catalog order would auto-select Haiku; the flagship-first fallback
        // must land on the curated quality-first default (Fable 5) instead.
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
            route("claude-fable-5", "Anthropic", "claude-api", true),
            route("claude-sonnet-4-6", "Anthropic", "claude-api", true),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-fable-5"),
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
            route("claude-fable-5", "Anthropic", "claude-oauth", true),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-fable-5")
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
            route("gpt-5.6-sol", "OpenAI", "openai-api", true),
        ];

        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("gpt-5.6-sol")
        );
    }

    #[test]
    fn global_default_route_prefers_gpt_5_6_over_fable_and_preserves_route() {
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai-api-key", true),
            route("claude-fable-5", "Anthropic", "anthropic-api-key", true),
            route("gpt-5.6-sol", "OpenAI", "openai-oauth", true),
        ];

        let selected = globally_preferred_default_route(&routes).expect("strongest route");
        assert_eq!(selected.model, "gpt-5.6-sol");
        assert_eq!(selected.provider, "OpenAI");
        assert_eq!(selected.api_method, "openai-oauth");
    }

    #[test]
    fn global_default_route_uses_clean_gpt_5_6_then_fable_before_weaker_models() {
        let clean_release = vec![
            route("claude-fable-5", "Anthropic", "claude-oauth", true),
            route("gpt-5.6", "OpenAI", "openai-api-key", true),
        ];
        assert_eq!(
            globally_preferred_default_route(&clean_release)
                .as_ref()
                .map(|route| route.model.as_str()),
            Some("gpt-5.6")
        );

        let unavailable_gpt = vec![
            route("gpt-5.6-sol", "OpenAI", "openai-api-key", false),
            route("gpt-5.5", "OpenAI", "openai-api-key", true),
            route("claude-fable-5", "Anthropic", "claude-oauth", true),
        ];
        assert_eq!(
            globally_preferred_default_route(&unavailable_gpt)
                .as_ref()
                .map(|route| route.model.as_str()),
            Some("claude-fable-5")
        );
    }

    #[test]
    fn global_default_route_ignores_unavailable_routes_and_preserves_unknown_order() {
        let routes = vec![
            route("provider-a-frontier", "Provider A", "provider-a", true),
            route("gpt-5.6-sol", "OpenAI", "openai-api-key", false),
            route("provider-b-frontier", "Provider B", "provider-b", true),
        ];

        let selected = globally_preferred_default_route(&routes).expect("fallback route");
        assert_eq!(selected.model, "provider-a-frontier");
        assert_eq!(selected.api_method, "provider-a");
    }

    #[test]
    fn post_auth_model_selection_falls_back_when_quality_first_model_is_unavailable() {
        let claude = activation_for_provider_id("claude-api");
        let claude_routes = vec![
            route("claude-fable-5", "Anthropic", "claude-api", false),
            route("claude-opus-4-8", "Anthropic", "claude-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&claude, None, &claude_routes).as_deref(),
            Some("claude-opus-4-8")
        );

        let openai = activation_for_provider_id("openai-api");
        let openai_routes = vec![
            route("gpt-5.6-sol", "OpenAI", "openai-api", false),
            route("gpt-5.5", "OpenAI", "openai-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&openai, None, &openai_routes).as_deref(),
            Some("gpt-5.5")
        );

        let openai_routes_with_clean_release = vec![
            route("gpt-5.6-sol", "OpenAI", "openai-api", false),
            route("gpt-5.5", "OpenAI", "openai-api", true),
            route("gpt-5.6", "OpenAI", "openai-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&openai, None, &openai_routes_with_clean_release)
                .as_deref(),
            Some("gpt-5.6"),
            "a clean same-generation release should beat GPT 5.5 when Sol is unavailable"
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

    #[test]
    fn post_auth_auto_promotes_newer_frontier_release_not_yet_in_curated_list() {
        // The day Anthropic ships a stronger Opus than the curated flagship, the
        // live catalog carries it and it must auto-promote to the post-login
        // default without a code change. Here `claude-opus-4-9` beats the curated
        // baseline `claude-opus-4-8`.
        let activation = activation_for_provider_id("claude-api");
        let routes = vec![
            route("claude-haiku-4-5", "Anthropic", "claude-api", true),
            route("claude-opus-4-8", "Anthropic", "claude-api", true),
            route("claude-opus-4-9", "Anthropic", "claude-api", true),
            route("claude-sonnet-4-6", "Anthropic", "claude-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-opus-4-9"),
            "a newer pure Opus flagship in the live catalog should auto-promote"
        );

        // Same for OpenAI: a future `gpt-5.7` beats the curated Sol 5.6 baseline.
        let activation = activation_for_provider_id("openai");
        let routes = vec![
            route("gpt-5-mini", "OpenAI", "openai", true),
            route("gpt-5.6-sol", "OpenAI", "openai", true),
            route("gpt-5.7", "OpenAI", "openai", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("gpt-5.7")
        );
    }

    #[test]
    fn quality_first_defaults_are_not_displaced_by_lower_family_or_equal_release() {
        let claude = activation_for_provider_id("claude-api");
        let claude_routes = vec![
            route("claude-opus-4-9", "Anthropic", "claude-api", true),
            route("claude-fable-5", "Anthropic", "claude-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&claude, None, &claude_routes).as_deref(),
            Some("claude-fable-5"),
            "a newer lower-priority Opus release must not displace available Fable"
        );

        let openai = activation_for_provider_id("openai");
        let openai_routes = vec![
            route("gpt-5.6", "OpenAI", "openai", true),
            route("gpt-5.6-sol", "OpenAI", "openai", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&openai, None, &openai_routes).as_deref(),
            Some("gpt-5.6-sol"),
            "the base model at the same release must not displace the Sol quality profile"
        );
    }

    #[test]
    fn post_auth_frontier_promotion_ignores_cheaper_and_specialized_variants() {
        // A newer *cheaper/specialized* variant must NOT auto-promote over the
        // curated flagship: only clean flagship ids qualify. Even though
        // `claude-haiku-5` and `gpt-6-mini`/`gpt-6-codex` have higher version
        // numbers, they carry non-flagship tier words and must be rejected, so
        // selection stays on the curated flagship.
        let activation = activation_for_provider_id("claude-api");
        let routes = vec![
            route("claude-haiku-5", "Anthropic", "claude-api", true),
            route("claude-opus-4-8", "Anthropic", "claude-api", true),
            route("claude-sonnet-5", "Anthropic", "claude-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-opus-4-8"),
            "cheaper/other-family models must not auto-promote over the curated Opus flagship"
        );

        let activation = activation_for_provider_id("openai");
        let routes = vec![
            route("gpt-6-mini", "OpenAI", "openai", true),
            route("gpt-6-codex", "OpenAI", "openai", true),
            route("gpt-5.5", "OpenAI", "openai", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("gpt-5.5"),
            "mini/codex variants must not auto-promote over the clean gpt flagship"
        );
    }

    #[test]
    fn post_auth_frontier_promotion_no_op_when_curated_is_still_newest() {
        // When the live catalog contains nothing newer than the curated flagship,
        // the curated quality order decides and frontier promotion is a no-op.
        let activation = activation_for_provider_id("claude-api");
        let routes = vec![
            route("claude-haiku-4-5-20251001", "Anthropic", "claude-api", true),
            route("claude-opus-4-6", "Anthropic", "claude-api", true),
            route("claude-opus-4-8", "Anthropic", "claude-api", true),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("claude-opus-4-8")
        );
    }

    #[test]
    fn post_auth_frontier_promotion_covers_bedrock_and_gemini() {
        // Bedrock: a newer Opus 5 (vendor-prefixed + dated) auto-promotes over the
        // curated Opus 4 baseline, and never falls back to the year-old 3.5.
        let activation = activation_for_provider_id("bedrock");
        let routes = vec![
            route(
                "anthropic.claude-3-5-sonnet-20241022-v2:0",
                "AWS Bedrock",
                "bedrock",
                true,
            ),
            route(
                "anthropic.claude-opus-4-20250514-v1:0",
                "AWS Bedrock",
                "bedrock",
                true,
            ),
            route(
                "anthropic.claude-opus-5-20260101-v1:0",
                "AWS Bedrock",
                "bedrock",
                true,
            ),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("anthropic.claude-opus-5-20260101-v1:0"),
            "a newer Bedrock Opus must auto-promote over the curated Opus 4"
        );

        // Gemini: a newer pro auto-promotes; a newer flash never displaces it.
        let activation = activation_for_provider_id("gemini");
        let routes = vec![
            route(
                "gemini-2.5-flash",
                "Google Gemini",
                "code-assist-oauth",
                true,
            ),
            route(
                "gemini-3-pro-preview",
                "Google Gemini",
                "code-assist-oauth",
                true,
            ),
            route(
                "gemini-4-pro-preview",
                "Google Gemini",
                "code-assist-oauth",
                true,
            ),
            route(
                "gemini-9-flash-preview",
                "Google Gemini",
                "code-assist-oauth",
                true,
            ),
        ];
        assert_eq!(
            provider_model_to_select_after_auth(&activation, None, &routes).as_deref(),
            Some("gemini-4-pro-preview"),
            "the newest Gemini *pro* must win; a higher-numbered flash must not"
        );
    }

    #[test]
    fn frontier_version_parsing_and_compare() {
        let fams = &[
            FrontierFamily {
                prefix: "claude-opus",
                flagship_token: None,
            },
            FrontierFamily {
                prefix: "gpt",
                flagship_token: None,
            },
        ];
        // Clean flagship ids parse with a version vector.
        let opus = parse_frontier_model("claude-opus-4-8", fams).expect("opus parses");
        assert_eq!(opus.family, "claude-opus");
        assert_eq!(opus.version, vec![4, 8]);
        let gpt = parse_frontier_model("gpt-5.5", fams).expect("gpt parses");
        assert_eq!(gpt.family, "gpt");
        assert_eq!(gpt.version, vec![5, 5]);
        // Dated id parses on the canonical base.
        assert_eq!(
            parse_frontier_model("claude-opus-4-9-20260101", fams)
                .expect("dated opus parses")
                .version,
            vec![4, 9]
        );
        // Specialized/cheap tiers and other families are rejected.
        assert!(parse_frontier_model("claude-haiku-5", fams).is_none());
        assert!(parse_frontier_model("gpt-6-mini", fams).is_none());
        assert!(parse_frontier_model("gpt-5-codex", fams).is_none());
        assert!(parse_frontier_model("claude-sonnet-5", fams).is_none());
        // Version comparison is component-wise with zero-padding.
        assert_eq!(version_cmp(&[4, 8], &[4, 9]), std::cmp::Ordering::Less);
        assert_eq!(version_cmp(&[5], &[5, 1]), std::cmp::Ordering::Less);
        assert_eq!(version_cmp(&[6], &[5, 9]), std::cmp::Ordering::Greater);
        assert_eq!(version_cmp(&[5, 5], &[5, 5]), std::cmp::Ordering::Equal);

        // Bedrock vendor-prefixed/versioned ids normalize to the bare Claude
        // family and parse as flagship.
        let bedrock = parse_frontier_model(
            "us.anthropic.claude-opus-4-20250514-v1:0",
            &[FrontierFamily {
                prefix: "claude-opus",
                flagship_token: None,
            }],
        )
        .expect("bedrock opus parses");
        assert_eq!(bedrock.version, vec![4]);

        // Gemini flagship token: `pro` is required and `flash`/`lite` are rejected.
        let gem_fams = &[FrontierFamily {
            prefix: "gemini",
            flagship_token: Some("pro"),
        }];
        let gpro = parse_frontier_model("gemini-3-pro-preview", gem_fams).expect("gemini pro");
        assert_eq!(gpro.family, "gemini");
        assert_eq!(gpro.version, vec![3]);
        assert_eq!(
            parse_frontier_model("gemini-3.1-pro", gem_fams)
                .expect("gemini 3.1 pro")
                .version,
            vec![3, 1]
        );
        assert!(
            parse_frontier_model("gemini-3-flash", gem_fams).is_none(),
            "gemini flash is the cheap tier and must not be a frontier flagship"
        );
        assert!(
            parse_frontier_model("gemini-2.5-flash-lite", gem_fams).is_none(),
            "gemini flash-lite must be rejected"
        );
    }

    #[test]
    fn normalize_model_for_preference_strips_hosted_prefixes_and_suffixes() {
        assert_eq!(
            normalize_model_for_preference("us.anthropic.claude-opus-4-20250514-v1:0"),
            "claude-opus-4"
        );
        assert_eq!(
            normalize_model_for_preference("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            "claude-3-5-sonnet"
        );
        assert_eq!(
            normalize_model_for_preference("models/gemini-3-pro-preview"),
            "gemini-3-pro"
        );
        assert_eq!(
            normalize_model_for_preference("accounts/fireworks/models/qwen3-coder"),
            "qwen3-coder"
        );
        // Non-hosted ids are unchanged apart from canonicalization.
        assert_eq!(
            normalize_model_for_preference("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(normalize_model_for_preference("gpt-5.5"), "gpt-5.5");
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
        "bedrock",
        "azure-openai",
        "gemini",
        "antigravity",
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
            (
                // Bedrock lists year-old Claude first; the curated order must
                // still pick Opus 4 over claude-3-5-sonnet. Bedrock ids carry the
                // vendor prefix + version tag, normalized away before ranking.
                "bedrock",
                "bedrock",
                "AWS Bedrock",
                &[
                    "anthropic.claude-3-5-sonnet-20241022-v2:0",
                    "anthropic.claude-3-5-haiku-20241022-v1:0",
                    "anthropic.claude-sonnet-4-20250514-v1:0",
                    "anthropic.claude-opus-4-20250514-v1:0",
                ],
                "anthropic.claude-opus-4-20250514-v1:0",
            ),
            (
                // Azure hosts the OpenAI family over the OpenRouter transport.
                "azure-openai",
                "openrouter",
                "Azure OpenAI",
                &["gpt-5-mini", "gpt-5.1", "gpt-5.5"],
                "gpt-5.5",
            ),
            (
                // Gemini's flagship tier is `pro`; a flash-first catalog must
                // still pick the strongest pro model.
                "gemini",
                "code-assist-oauth",
                "Google Gemini",
                &["gemini-2.5-flash", "gemini-2.5-pro", "gemini-3-pro-preview"],
                "gemini-3-pro-preview",
            ),
            (
                // Antigravity also serves Gemini models (https transport).
                "antigravity",
                "https",
                "Antigravity",
                &["gemini-2.5-flash", "gemini-2.5-pro", "gemini-3-pro-preview"],
                "gemini-3-pro-preview",
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

    #[test]
    fn onboarding_frontier_provider_preference_matrix() {
        use crate::auth::{AuthState, AuthStatus, ProviderAuth};

        let none = AuthStatus::default();
        assert_eq!(preferred_frontier_auth_provider(&none), None);

        let openai_api = AuthStatus {
            openai: AuthState::Available,
            openai_has_api_key: true,
            ..AuthStatus::default()
        };
        assert_eq!(
            preferred_frontier_auth_provider(&openai_api),
            Some("openai-api")
        );

        let anthropic_api = AuthStatus {
            anthropic: ProviderAuth {
                state: AuthState::Available,
                has_api_key: true,
                ..ProviderAuth::default()
            },
            ..AuthStatus::default()
        };
        assert_eq!(
            preferred_frontier_auth_provider(&anthropic_api),
            Some("claude-api")
        );

        let both_oauth = AuthStatus {
            openai: AuthState::Available,
            openai_has_oauth: true,
            openai_oauth_state: AuthState::Available,
            anthropic: ProviderAuth {
                state: AuthState::Available,
                has_oauth: true,
                oauth_state: AuthState::Available,
                ..ProviderAuth::default()
            },
            ..AuthStatus::default()
        };
        assert_eq!(
            preferred_frontier_auth_provider(&both_oauth),
            Some("claude"),
            "Claude is the quality-first default when both frontier providers work"
        );

        let openai_api_and_oauth = AuthStatus {
            openai: AuthState::Available,
            openai_has_oauth: true,
            openai_oauth_state: AuthState::Available,
            openai_has_api_key: true,
            ..AuthStatus::default()
        };
        assert_eq!(
            preferred_frontier_auth_provider(&openai_api_and_oauth),
            Some("openai"),
            "OAuth is preferred over an API key within one provider family"
        );
    }
}
