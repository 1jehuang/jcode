//! Provider strict end-to-end diagnostic runner.
//!
//! This powers `jcode provider-doctor`: it walks the same strict provider/model
//! checkpoints that the coverage ledger tracks, but as a user-facing diagnostic
//! so anyone can answer "why is my provider/model or model picker broken?".
//!
//! Three tiers trade off safety vs. coverage:
//! - [`DoctorTier::Offline`]: no API key, no network, no spend. Validates jcode's
//!   own wiring (catalog reload, picker rendering, fallback labeling, model-switch
//!   routing, auth-lifecycle transcript) against a synthetic catalog.
//! - [`DoctorTier::Catalog`]: needs a key, ~no spend. Everything in offline plus the
//!   live `GET /models` fetch (validates the key, the endpoint, and that the model
//!   exists in the live catalog).
//! - [`DoctorTier::Full`]: needs a key, spends balance. Everything in catalog plus a
//!   non-streaming completion, a streaming completion, and a tool-call loop.
//!
//! Only the [`DoctorTier::Full`] tier can earn strict coverage; the lighter tiers
//! intentionally record the API-dependent checkpoints as skipped so nothing is
//! over-credited in the ledger.

use crate::live_provider_probes::{
    fetch_live_openai_compatible_models, run_live_antigravity_native_reasoning_smoke,
    run_live_antigravity_native_smoke, run_live_antigravity_native_stream_smoke,
    run_live_antigravity_native_tool_smoke, run_live_claude_native_reasoning_smoke,
    run_live_claude_native_smoke, run_live_claude_native_stream_smoke,
    run_live_claude_native_tool_smoke, run_live_native_provider_reasoning_smoke,
    run_live_native_provider_smoke, run_live_native_provider_stream_smoke,
    run_live_native_provider_tool_smoke, run_live_openai_compatible_smoke,
    run_live_openai_compatible_stream_smoke, run_live_openai_compatible_tool_smoke,
};
use jcode_base::auth::lifecycle::{
    AuthActivationRequest, activate_auth_change, validate_catalog_invariants,
};
use jcode_base::live_tests::{
    self, LiveVerificationAuth, LiveVerificationEvent, LiveVerificationResult,
    LiveVerificationStage, LiveVerificationStageStatus, checkpoints,
};
use jcode_base::protocol::{AuthChanged, CatalogNamespace, RuntimeProviderKey};
use jcode_base::provider::ModelRoute;
use jcode_base::provider_catalog::OpenAiCompatibleProfile;

/// How much of the strict pipeline to exercise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorTier {
    /// No key, no network, no spend. Validates jcode-side wiring only.
    Offline,
    /// Needs a key, negligible spend. Adds the live model catalog fetch.
    Catalog,
    /// Needs a key, spends balance. Adds chat, streaming, and tool-call checkpoints.
    Full,
}

impl DoctorTier {
    pub fn requires_api_key(self) -> bool {
        !matches!(self, DoctorTier::Offline)
    }

    pub fn spends_balance(self) -> bool {
        matches!(self, DoctorTier::Full)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DoctorTier::Offline => "offline",
            DoctorTier::Catalog => "catalog",
            DoctorTier::Full => "full",
        }
    }
}

impl std::str::FromStr for DoctorTier {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "offline" => Ok(DoctorTier::Offline),
            "catalog" => Ok(DoctorTier::Catalog),
            "full" => Ok(DoctorTier::Full),
            other => Err(format!(
                "unknown tier `{other}` (expected offline, catalog, or full)"
            )),
        }
    }
}

/// One checkpoint result in a doctor run.
#[derive(Clone, Debug)]
pub struct DoctorCheck {
    pub checkpoint: &'static str,
    pub label: &'static str,
    pub status: LiveVerificationStageStatus,
    /// Human-readable detail (failure reason, evidence summary, or skip reason).
    pub detail: String,
}

impl DoctorCheck {
    fn passed(checkpoint: &'static str, label: &'static str, detail: impl Into<String>) -> Self {
        Self {
            checkpoint,
            label,
            status: LiveVerificationStageStatus::Passed,
            detail: detail.into(),
        }
    }

    fn failed(checkpoint: &'static str, label: &'static str, detail: impl Into<String>) -> Self {
        Self {
            checkpoint,
            label,
            status: LiveVerificationStageStatus::Failed,
            detail: detail.into(),
        }
    }

    fn skipped(checkpoint: &'static str, label: &'static str, detail: impl Into<String>) -> Self {
        Self {
            checkpoint,
            label,
            status: LiveVerificationStageStatus::Skipped,
            detail: detail.into(),
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(
            self.status,
            LiveVerificationStageStatus::Failed | LiveVerificationStageStatus::Blocked
        )
    }
}

/// The complete result of a doctor run for one provider/model.
#[derive(Clone, Debug)]
pub struct DoctorReport {
    pub provider_id: String,
    pub provider_label: String,
    pub model: String,
    pub tier: DoctorTier,
    pub checks: Vec<DoctorCheck>,
    /// True when every required checkpoint for the chosen tier passed.
    pub tier_passed: bool,
    /// True when every strict checkpoint passed (only possible on the full tier).
    pub strict_passed: bool,
    /// Token/cost spend incurred by this run's billable API calls.
    pub spend: DoctorSpend,
}

/// Tokens and (when the provider reports it) dollar cost spent by a doctor run.
///
/// Aggregated across every billable call in the run (non-streaming chat,
/// streaming chat, and the tool-call round-trip on the `full` tier). Lighter
/// tiers leave this empty/zeroed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DoctorSpend {
    /// Number of billable API calls made.
    pub billable_calls: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// Sum of provider-reported `cost` (USD), when present. `None` if no call
    /// reported a cost field.
    pub reported_cost_usd: Option<f64>,
    /// True when at least one billable call reported a token count.
    pub has_token_data: bool,
}

impl DoctorSpend {
    /// Fold one API response's `usage`/`cost` JSON into the running total.
    fn accumulate(&mut self, usage: Option<&serde_json::Value>, cost: Option<&serde_json::Value>) {
        self.billable_calls += 1;
        if let Some(usage) = usage {
            let prompt = usage
                .get("prompt_tokens")
                .or_else(|| usage.get("input_tokens"))
                .and_then(serde_json::Value::as_u64);
            let completion = usage
                .get("completion_tokens")
                .or_else(|| usage.get("output_tokens"))
                .and_then(serde_json::Value::as_u64);
            let total = usage
                .get("total_tokens")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| match (prompt, completion) {
                    (Some(p), Some(c)) => Some(p + c),
                    _ => None,
                });
            if let Some(prompt) = prompt {
                self.prompt_tokens += prompt;
                self.has_token_data = true;
            }
            if let Some(completion) = completion {
                self.completion_tokens += completion;
                self.has_token_data = true;
            }
            if let Some(total) = total {
                self.total_tokens += total;
                self.has_token_data = true;
            }
        }
        if let Some(cost) = cost.and_then(serde_json::Value::as_f64) {
            *self.reported_cost_usd.get_or_insert(0.0) += cost;
        }
    }

    /// Serialize for persistence into the ledger event metadata.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "billable_calls": self.billable_calls,
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total_tokens,
            "reported_cost_usd": self.reported_cost_usd,
            "has_token_data": self.has_token_data,
        })
    }

    /// One-line, human-readable spend summary for the doctor output.
    pub fn human_summary(&self) -> String {
        if self.billable_calls == 0 {
            return "no billable API calls (no balance spent)".to_string();
        }
        let calls = format!(
            "{} billable API call{}",
            self.billable_calls,
            if self.billable_calls == 1 { "" } else { "s" }
        );
        let tokens = if self.has_token_data {
            format!(
                ", {} tokens ({} in + {} out)",
                self.total_tokens, self.prompt_tokens, self.completion_tokens
            )
        } else {
            ", token usage not reported by provider".to_string()
        };
        let cost = match self.reported_cost_usd {
            Some(cost) => format!(", provider-reported cost ${cost:.6}"),
            None => ", cost not reported by provider".to_string(),
        };
        format!("{calls}{tokens}{cost}")
    }
}

impl DoctorReport {
    pub fn first_failure(&self) -> Option<&DoctorCheck> {
        self.checks.iter().find(|check| check.is_failure())
    }
}

const FULL_PIPELINE_LABELS: &[(&str, &str)] = &[
    (checkpoints::AUTH_CREDENTIAL_LOADED, "Credential loaded"),
    (
        checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
        "Live model catalog endpoint",
    ),
    (
        checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
        "Catalog hot reload in current session",
    ),
    (checkpoints::PICKER_LIVE_MODELS, "Picker shows live models"),
    (
        checkpoints::PICKER_FALLBACK_LABELING,
        "Picker fallback labeling",
    ),
    (checkpoints::MODEL_SWITCH_ROUTE, "Model switch route"),
    (
        checkpoints::NON_STREAMING_CHAT_COMPLETION,
        "Non-streaming chat completion",
    ),
    (
        checkpoints::STREAMING_CHAT_COMPLETION,
        "Streaming chat completion",
    ),
    (checkpoints::TOOL_CALL_PARSE, "Tool-call parse"),
    (checkpoints::TOOL_EXECUTION_LOOP, "Tool execution loop"),
    (checkpoints::TOOL_RESULT_FOLLOWUP, "Tool-result followup"),
    (checkpoints::REAL_JCODE_TOOL_SMOKE, "Real Jcode tool smoke"),
    (checkpoints::REASONING_CAPABILITY, "Reasoning capability"),
];

fn label_for(checkpoint: &str) -> &'static str {
    FULL_PIPELINE_LABELS
        .iter()
        .find(|(id, _)| *id == checkpoint)
        .map(|(_, label)| *label)
        .unwrap_or("Checkpoint")
}

/// Human-readable detail for a passed tool-smoke stage, surfacing whether the
/// multi-call thought-signature replay phase was exercised. The native tool
/// smoke records `multi_tool_replay` as `verified` (a two-`functionCall`
/// history was replayed and accepted, the shape that reproduces the
/// "missing a thought_signature ... position N" 400) or `skipped` (the model
/// declined a second tool call). Surfacing it keeps the coverage observable in
/// the doctor report instead of collapsing to a generic pass string.
fn tool_stage_detail(stage: &jcode_base::live_tests::LiveVerificationStage) -> String {
    let multi = match stage
        .evidence
        .get("multi_tool_replay")
        .and_then(|value| value.as_str())
    {
        Some("verified") => "multi-call signature replay verified",
        Some("skipped") => "multi-call signature replay skipped (no 2nd tool call)",
        _ => "",
    };
    let parallel = match stage
        .evidence
        .get("parallel_tool_calls")
        .and_then(|value| value.as_str())
    {
        Some("verified") => "parallel tool calls verified",
        Some("skipped") => "parallel tool calls skipped (single call)",
        _ => "",
    };
    let mut detail = "tool call parsed and executed".to_string();
    for part in [multi, parallel] {
        if !part.is_empty() {
            detail.push_str("; ");
            detail.push_str(part);
        }
    }
    detail
}

/// Human-readable detail for a passed reasoning-capability stage. The stage
/// records `reasoning_capability` as `streamed` (visible reasoning text),
/// `opaque` (no text but a reasoning signal: thought signature, reasoning item,
/// or reasoning tokens), or `none` (neither). All three are passes; `opaque` and
/// `none` are legitimate because providers like Gemini-3 and OpenAI hide their
/// reasoning. Surfacing the classification keeps the observation visible in the
/// doctor report.
fn reasoning_stage_detail(stage: &jcode_base::live_tests::LiveVerificationStage) -> String {
    match stage
        .evidence
        .get("reasoning_capability")
        .and_then(|value| value.as_str())
    {
        Some("streamed") => "reasoning streamed (visible thinking text)".to_string(),
        Some("opaque") => {
            "reasoning hidden but signaled (opaque: thought signature / reasoning item)".to_string()
        }
        Some("none") => "no reasoning signal observed (model hides or skips reasoning)".to_string(),
        _ => "reasoning turn completed".to_string(),
    }
}

/// Fold a reasoning-capability probe result into a [`DoctorCheck`], honoring the
/// observe-only contract.
///
/// A clean turn records a passed checkpoint carrying the `streamed`/`opaque`/
/// `none` classification (all three are passes; hiding reasoning is legitimate).
/// A probe *error* (network, or a turn that did not complete with a coherent
/// answer) is recorded as **skipped**, never failed: this checkpoint must never
/// flip a provider to "not user-ready", and it is not part of the strict
/// coverage ladder, so an observational miss should not fail the tier. The
/// broader chat/streaming checkpoints already guard turn completion.
fn push_reasoning_check(
    result: anyhow::Result<LiveVerificationStage>,
    checks: &mut Vec<DoctorCheck>,
    spend: &mut DoctorSpend,
) {
    match result {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            let detail = reasoning_stage_detail(&stage);
            checks.push(DoctorCheck::passed(
                checkpoints::REASONING_CAPABILITY,
                label_for(checkpoints::REASONING_CAPABILITY),
                detail,
            ));
        }
        Err(error) => checks.push(DoctorCheck::skipped(
            checkpoints::REASONING_CAPABILITY,
            label_for(checkpoints::REASONING_CAPABILITY),
            format!(
                "observe-only reasoning probe did not complete: {}",
                format_error_chain(&error)
            ),
        )),
    }
}

/// Checkpoints that require a real API response and are therefore skipped on the
/// offline/catalog tiers.
const API_DEPENDENT_CHECKPOINTS: &[&str] = &[
    checkpoints::NON_STREAMING_CHAT_COMPLETION,
    checkpoints::STREAMING_CHAT_COMPLETION,
    checkpoints::TOOL_CALL_PARSE,
    checkpoints::TOOL_EXECUTION_LOOP,
    checkpoints::TOOL_RESULT_FOLLOWUP,
    checkpoints::REAL_JCODE_TOOL_SMOKE,
    checkpoints::REASONING_CAPABILITY,
];

/// Run the strict provider/model diagnostic.
///
/// `api_key` may be `None` only when `tier == DoctorTier::Offline`.
pub async fn run_provider_e2e(
    profile: OpenAiCompatibleProfile,
    api_key: Option<&str>,
    requested_model: Option<&str>,
    tier: DoctorTier,
) -> anyhow::Result<DoctorReport> {
    let resolved = jcode_base::provider_catalog::resolve_openai_compatible_profile(profile);
    let provider_id = profile.id.to_string();
    let provider_label = profile.display_name.to_string();
    let mut checks: Vec<DoctorCheck> = Vec::new();

    if tier.requires_api_key() && api_key.map(str::trim).unwrap_or("").is_empty() {
        anyhow::bail!(
            "tier `{}` requires an API key for provider `{}` but none was supplied",
            tier.as_str(),
            provider_id
        );
    }

    // --- Stage 1: credential loaded ---
    match api_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(_) => checks.push(DoctorCheck::passed(
            checkpoints::AUTH_CREDENTIAL_LOADED,
            label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
            format!("Loaded credential from {}", resolved.api_key_env),
        )),
        None => checks.push(DoctorCheck::skipped(
            checkpoints::AUTH_CREDENTIAL_LOADED,
            label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
            "offline tier: no credential required".to_string(),
        )),
    }

    // --- Stage 2: live model catalog (or synthetic for offline) ---
    let catalog_models: Vec<String> = if tier.requires_api_key() {
        match fetch_live_openai_compatible_models(profile, api_key.unwrap_or_default()).await {
            Ok(models) => {
                checks.push(DoctorCheck::passed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!("{} live model(s) returned", models.len()),
                ));
                models
            }
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format_error_chain(&error),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    compat_auth(api_key, &resolved.api_key_env, &resolved.env_file),
                ));
            }
        }
    } else {
        // Offline tier: synthesize a small catalog so we can still validate wiring.
        checks.push(DoctorCheck::skipped(
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
            "offline tier: using synthetic catalog (no network)".to_string(),
        ));
        let default_model = profile.default_model.unwrap_or("fixture-model");
        vec![
            default_model.to_string(),
            format!("{}-alternate-fixture-model", profile.id),
        ]
    };

    // Pick the model under test.
    let selected = match requested_model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => {
            if tier.requires_api_key() && !catalog_models.iter().any(|m| m == model) {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!(
                        "requested model `{model}` is not in the live catalog ({} model(s): {})",
                        catalog_models.len(),
                        truncate_list(&catalog_models)
                    ),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    model.to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    compat_auth(api_key, &resolved.api_key_env, &resolved.env_file),
                ));
            }
            model.to_string()
        }
        None => profile
            .default_model
            .filter(|default| catalog_models.iter().any(|m| m == default))
            .map(ToString::to_string)
            .or_else(|| catalog_models.first().cloned())
            .unwrap_or_else(|| "fixture-model".to_string()),
    };

    // --- Stage 3: auth-lifecycle wiring (catalog reload, picker, fallback, switch) ---
    run_wiring_checks(profile, &selected, &catalog_models, &mut checks);

    // --- Stage 4: API-dependent checkpoints ---
    let mut spend = DoctorSpend::default();
    if tier == DoctorTier::Full {
        run_full_api_checks(
            profile,
            api_key.unwrap_or_default(),
            &selected,
            &mut checks,
            &mut spend,
        )
        .await;
    } else {
        for checkpoint in API_DEPENDENT_CHECKPOINTS {
            checks.push(DoctorCheck::skipped(
                checkpoint,
                label_for(checkpoint),
                format!(
                    "{} tier: requires --tier full (spends balance)",
                    tier.as_str()
                ),
            ));
        }
    }

    Ok(finish_report(
        provider_id,
        provider_label,
        selected,
        tier,
        checks,
        spend,
        compat_auth(api_key, &resolved.api_key_env, &resolved.env_file),
    ))
}

/// The native-runtime providers this doctor can drive directly (i.e. providers
/// whose live path is not OpenAI-compatible and so cannot be exercised by
/// [`run_provider_e2e`]). Today this is the Claude OAuth/subscription provider,
/// the Antigravity (Google OAuth Cloud Code) provider, and the generic
/// native-runtime providers (OpenAI, Gemini, Cursor, Copilot, Bedrock).
///
/// The predicate itself lives in `jcode_base::auth::doctor` so base-internal
/// code (`live_tests` roster annotation) can call it without depending on this
/// crate; `native_provider_roster_matches_base_predicate` below keeps it in
/// sync with [`NativeProviderKind`].
pub use jcode_base::auth::doctor::native_doctor_supports_provider;

/// The wiring contract for the native Claude (OAuth/subscription) provider.
///
/// `claude` activates the native Claude runtime and routes through the
/// `claude-oauth:` model-switch prefix; its live-catalog routes carry the
/// `claude-oauth` api_method and the `Anthropic` provider label.
fn native_claude_wiring_contract() -> WiringContract {
    WiringContract {
        api_method: "claude-oauth".to_string(),
        route_provider: "Anthropic".to_string(),
        expected_runtime: "claude",
        expected_namespace: None,
        switch_prefix: "claude-oauth:".to_string(),
    }
}

/// Pick the cheapest sensible Claude model from a catalog for a smoke run.
///
/// Prefers Haiku, then Sonnet, then Opus (cheapest to priciest). Variants with
/// extended context windows (e.g. `[1m]`) are skipped in favor of the base id to
/// avoid the long-context surcharge. Returns `None` if no Claude tier matches,
/// letting the caller fall back to the runtime default.
fn cheapest_catalog_model(catalog_models: &[String]) -> Option<String> {
    let base_only = |m: &&String| !m.contains('[');
    for tier in ["haiku", "sonnet", "opus"] {
        if let Some(model) = catalog_models
            .iter()
            .filter(base_only)
            .find(|m| m.to_ascii_lowercase().contains(tier))
        {
            return Some(model.clone());
        }
    }
    None
}

/// Run the strict provider/model diagnostic for the **native Claude** provider.
///
/// This is the native-runtime counterpart to [`run_provider_e2e`]: instead of
/// driving an OpenAI-compatible HTTP shim, it exercises the production
/// [`AnthropicProvider`] runtime end to end (OAuth/API-key resolution, the live
/// `GET /v1/models` catalog, the Claude Code OAuth preflight, request shaping,
/// SSE→`StreamEvent` translation, and tool-call round-trips). It records the
/// same 11 strict checkpoints so the coverage ledger can promote `claude` to
/// READY exactly like a doctor-drivable provider.
///
/// `provider_id` is the auth provider id under test (`claude`/`anthropic`).
pub async fn run_claude_native_e2e(
    provider_id: &str,
    requested_model: Option<&str>,
    tier: DoctorTier,
) -> anyhow::Result<DoctorReport> {
    use jcode_base::provider::Provider;
    use jcode_provider_anthropic_runtime::AnthropicProvider;

    let normalized = jcode_base::auth::lifecycle::normalized_auth_provider_id(Some(provider_id))
        .unwrap_or("claude");
    let provider_label = jcode_base::auth::lifecycle::provider_display_label(Some(normalized))
        .unwrap_or_else(|| "Anthropic/Claude".to_string());
    let provider_id = normalized.to_string();
    let mut checks: Vec<DoctorCheck> = Vec::new();

    // Resolve the credential through the production runtime so the doctor sees
    // exactly what the agent would. We never log or surface the token itself.
    //
    // The `claude` login provider is specifically the OAuth/subscription path,
    // so pin OAuth mode before resolving: otherwise a self-dev session with
    // `JCODE_RUNTIME_PROVIDER=claude-api` would silently test the API-key path
    // and mislabel the credential. Pinning also points any provider instances
    // the probes build afterwards at the same OAuth path.
    let provider_runtime = AnthropicProvider::new();
    let want_oauth = true;
    if tier.requires_api_key()
        && let Err(error) = provider_runtime.pin_credential_mode_for_doctor(want_oauth)
    {
        checks.push(DoctorCheck::failed(
            checkpoints::AUTH_CREDENTIAL_LOADED,
            label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
            format!(
                "could not select the Claude OAuth credential path: {error}. \
                 Run `jcode login --provider claude` to mint a fresh OAuth token."
            ),
        ));
        return Ok(finish_report(
            provider_id,
            provider_label,
            requested_model.unwrap_or("").to_string(),
            tier,
            checks,
            DoctorSpend::default(),
            native_claude_auth(want_oauth),
        ));
    }
    let credential_is_oauth = if tier.requires_api_key() {
        match provider_runtime.resolve_access_token_for_doctor().await {
            Ok((token, is_oauth)) if !token.trim().is_empty() => {
                let kind = if is_oauth {
                    "OAuth (subscription)"
                } else {
                    "API key"
                };
                checks.push(DoctorCheck::passed(
                    checkpoints::AUTH_CREDENTIAL_LOADED,
                    label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                    format!("Resolved Claude {kind} credential"),
                ));
                Some(is_oauth)
            }
            Ok(_) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::AUTH_CREDENTIAL_LOADED,
                    label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                    "resolved an empty Claude access token".to_string(),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_claude_auth(false),
                ));
            }
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::AUTH_CREDENTIAL_LOADED,
                    label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                    format!(
                        "could not resolve a Claude credential: {error}. \
                         Run `jcode login --provider claude` to mint a fresh OAuth token."
                    ),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_claude_auth(false),
                ));
            }
        }
    } else {
        checks.push(DoctorCheck::skipped(
            checkpoints::AUTH_CREDENTIAL_LOADED,
            label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
            "offline tier: no credential required".to_string(),
        ));
        None
    };
    let is_oauth = credential_is_oauth.unwrap_or(false);

    // --- Stage 2: live model catalog (or synthetic for offline) ---
    let catalog_models: Vec<String> = if tier.requires_api_key() {
        match provider_runtime.fetch_live_model_ids_for_doctor().await {
            Ok(models) if !models.is_empty() => {
                checks.push(DoctorCheck::passed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!("{} live model(s) returned", models.len()),
                ));
                models
            }
            Ok(_) => {
                // Endpoint worked but returned nothing usable; fall back to the
                // known model ids so wiring checks can still run, and record the
                // catalog endpoint as passed (it answered) but note the fallback.
                let fallback = jcode_base::provider::known_anthropic_model_ids();
                checks.push(DoctorCheck::passed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!(
                        "live catalog empty; using {} known model id(s)",
                        fallback.len()
                    ),
                ));
                fallback
            }
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format_error_chain(&error),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_claude_auth(is_oauth),
                ));
            }
        }
    } else {
        checks.push(DoctorCheck::skipped(
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
            "offline tier: using known Claude model ids (no network)".to_string(),
        ));
        jcode_base::provider::known_anthropic_model_ids()
    };

    // Pick the model under test. When the caller does not request a specific
    // model, prefer the cheapest available Claude tier (Haiku) so the live smoke
    // run spends as little balance as possible; fall back to the runtime default
    // and finally to whatever the catalog offers.
    let default_model = provider_runtime.model();
    let selected = match requested_model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => {
            if tier.requires_api_key() && !catalog_models.iter().any(|m| m == model) {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!(
                        "requested model `{model}` is not in the live catalog ({} model(s): {})",
                        catalog_models.len(),
                        truncate_list(&catalog_models)
                    ),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    model.to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_claude_auth(is_oauth),
                ));
            }
            model.to_string()
        }
        None => cheapest_catalog_model(&catalog_models)
            .or_else(|| {
                catalog_models
                    .iter()
                    .find(|m| **m == default_model)
                    .cloned()
            })
            .or_else(|| catalog_models.first().cloned())
            .unwrap_or(default_model),
    };

    // --- Stage 3: auth-lifecycle wiring (catalog reload, picker, fallback, switch) ---
    run_wiring_checks_for_contract(
        &provider_id,
        &native_claude_wiring_contract(),
        &selected,
        &catalog_models,
        &mut checks,
    );

    // --- Stage 4: API-dependent checkpoints (live native runtime) ---
    let mut spend = DoctorSpend::default();
    if tier == DoctorTier::Full {
        run_native_claude_api_checks(&selected, &mut checks, &mut spend).await;
    } else {
        for checkpoint in API_DEPENDENT_CHECKPOINTS {
            checks.push(DoctorCheck::skipped(
                checkpoint,
                label_for(checkpoint),
                format!(
                    "{} tier: requires --tier full (spends balance)",
                    tier.as_str()
                ),
            ));
        }
    }

    Ok(finish_report(
        provider_id,
        provider_label,
        selected,
        tier,
        checks,
        spend,
        native_claude_auth(is_oauth),
    ))
}

/// Credential descriptor for the native Claude doctor. We never persist the
/// token; this records the credential *source* (OAuth vs API key) for the
/// ledger without a secret fingerprint, since OAuth tokens rotate.
fn native_claude_auth(is_oauth: bool) -> LiveVerificationAuth {
    let source = if is_oauth {
        "Claude OAuth (subscription) via auth.json"
    } else {
        "Claude API key (ANTHROPIC_API_KEY)"
    };
    let env_key = if is_oauth {
        None
    } else {
        Some("ANTHROPIC_API_KEY")
    };
    LiveVerificationAuth::non_secret(source, env_key)
}

/// Drive the three live native-Claude probes and fold their results into the
/// six API-dependent checkpoints, mirroring [`run_full_api_checks`].
async fn run_native_claude_api_checks(
    selected: &str,
    checks: &mut Vec<DoctorCheck>,
    spend: &mut DoctorSpend,
) {
    // Non-streaming completion.
    match run_live_claude_native_smoke(selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::NON_STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
                "received expected completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::NON_STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Streaming completion.
    match run_live_claude_native_stream_smoke(selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::STREAMING_CHAT_COMPLETION),
                "received expected streamed completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Tool call + derived execution/result/smoke checkpoints (one round-trip).
    match run_live_claude_native_tool_smoke(selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::passed(
                    checkpoint,
                    label_for(checkpoint),
                    "tool call parsed and executed".to_string(),
                ));
            }
        }
        Err(error) => {
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::failed(
                    checkpoint,
                    label_for(checkpoint),
                    format_error_chain(&error),
                ));
            }
        }
    }

    // Reasoning capability (observe-only; never gates readiness).
    push_reasoning_check(
        run_live_claude_native_reasoning_smoke(selected).await,
        checks,
        spend,
    );
}

/// The wiring contract for the native Antigravity (Google OAuth Cloud Code)
/// provider.
///
/// `antigravity` activates the `Antigravity` runtime and routes through the
/// `antigravity:` model-switch prefix; its live-catalog routes carry the
/// `https` api_method (parsed as [`ModelRouteApiMethod::AntigravityHttps`]) and
/// the `Antigravity` provider label.
fn native_antigravity_wiring_contract() -> WiringContract {
    WiringContract {
        api_method: "https".to_string(),
        route_provider: "Antigravity".to_string(),
        expected_runtime: "antigravity",
        expected_namespace: None,
        switch_prefix: "antigravity:".to_string(),
    }
}

/// Credential descriptor for the native Antigravity doctor. Antigravity uses
/// Google OAuth tokens minted by `jcode login --provider antigravity`; the
/// tokens rotate and are never persisted here, so we record only the source
/// (and the resolved Google account email when available) without a secret.
fn native_antigravity_auth(account: &str) -> LiveVerificationAuth {
    let source = if account.trim().is_empty() {
        "Antigravity Google OAuth via auth.json".to_string()
    } else {
        format!("Antigravity Google OAuth ({account}) via auth.json")
    };
    LiveVerificationAuth::non_secret(source, None::<String>)
}

/// Pick the cheapest sensible Antigravity model from a catalog for a smoke run.
///
/// Prefers a Gemini Flash tier (cheapest, and the backend's native path that
/// accepts every schema construct jcode emits), then any Gemini model, then any
/// available catalog model. Returns `None` when the catalog is empty, letting
/// the caller fall back to the runtime default.
fn cheapest_antigravity_model(catalog_models: &[String]) -> Option<String> {
    let is_alias = |m: &&String| m.trim().is_empty() || m.trim() == "default";
    if let Some(flash) = catalog_models.iter().filter(|m| !is_alias(m)).find(|m| {
        let lower = m.to_ascii_lowercase();
        lower.starts_with("gemini") && lower.contains("flash")
    }) {
        return Some(flash.clone());
    }
    if let Some(gemini) = catalog_models
        .iter()
        .filter(|m| !is_alias(m))
        .find(|m| m.to_ascii_lowercase().starts_with("gemini"))
    {
        return Some(gemini.clone());
    }
    catalog_models.iter().find(|m| !is_alias(m)).cloned()
}

/// Run the strict provider/model diagnostic for the **native Antigravity**
/// provider.
///
/// The native-runtime counterpart to [`run_provider_e2e`] for Antigravity:
/// instead of driving an OpenAI-compatible HTTP shim, it exercises the
/// production [`AntigravityProvider`] runtime end to end (Google OAuth token
/// load/refresh, project resolution, the live `fetchAvailableModels` catalog,
/// request shaping, the per-model schema normalization, the Gemini->StreamEvent
/// translation, and Gemini-3 thought-signature tool round-trips). It records the
/// same strict checkpoints so the coverage ledger can promote `antigravity` to
/// READY exactly like a doctor-drivable provider.
pub async fn run_antigravity_native_e2e(
    provider_id: &str,
    requested_model: Option<&str>,
    tier: DoctorTier,
) -> anyhow::Result<DoctorReport> {
    use jcode_base::provider::Provider;
    use jcode_provider_antigravity_runtime::AntigravityProvider;

    // The antigravity login provider has a single fixed id; accept any alias the
    // caller passed (e.g. "antigravity") and normalize to the canonical id.
    let _ = jcode_base::auth::lifecycle::normalized_auth_provider_id(Some(provider_id));
    let provider_label = jcode_base::auth::lifecycle::provider_display_label(Some("antigravity"))
        .unwrap_or_else(|| "Antigravity".to_string());
    let provider_id = "antigravity".to_string();
    let mut checks: Vec<DoctorCheck> = Vec::new();

    let runtime = AntigravityProvider::new();

    // --- Stage 1: credential resolution (Google OAuth tokens) ---
    let mut account = String::new();
    if tier.requires_api_key() {
        match runtime.resolve_account_for_doctor().await {
            Ok(resolved) => {
                account = resolved;
                let detail = if account.trim().is_empty() {
                    "Resolved Antigravity Google OAuth credential".to_string()
                } else {
                    format!("Resolved Antigravity Google OAuth credential ({account})")
                };
                checks.push(DoctorCheck::passed(
                    checkpoints::AUTH_CREDENTIAL_LOADED,
                    label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                    detail,
                ));
            }
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::AUTH_CREDENTIAL_LOADED,
                    label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                    format!(
                        "could not resolve an Antigravity credential: {error}. \
                         Run `jcode login --provider antigravity` to sign in."
                    ),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_antigravity_auth(&account),
                ));
            }
        }
    } else {
        checks.push(DoctorCheck::skipped(
            checkpoints::AUTH_CREDENTIAL_LOADED,
            label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
            "offline tier: no credential required".to_string(),
        ));
    }

    // --- Stage 2: live model catalog (or synthetic for offline) ---
    let catalog_models: Vec<String> = if tier.requires_api_key() {
        match runtime.fetch_live_model_ids_for_doctor().await {
            Ok(models) if !models.is_empty() => {
                checks.push(DoctorCheck::passed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!("{} live model(s) returned", models.len()),
                ));
                models
            }
            Ok(_) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    "live Antigravity catalog returned no available models".to_string(),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_antigravity_auth(&account),
                ));
            }
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format_error_chain(&error),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_antigravity_auth(&account),
                ));
            }
        }
    } else {
        checks.push(DoctorCheck::skipped(
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
            "offline tier: using known Antigravity model ids (no network)".to_string(),
        ));
        runtime
            .available_models()
            .into_iter()
            .map(str::to_string)
            .collect()
    };

    // Pick the model under test. Without an explicit request, prefer the
    // cheapest Gemini Flash tier (cheapest + native path) so the live smoke run
    // spends as little quota as possible.
    let default_model = runtime.model();
    let selected = match requested_model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => {
            if tier.requires_api_key() && !catalog_models.iter().any(|m| m == model) {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!(
                        "requested model `{model}` is not in the live catalog ({} model(s): {})",
                        catalog_models.len(),
                        truncate_list(&catalog_models)
                    ),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    model.to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    native_antigravity_auth(&account),
                ));
            }
            model.to_string()
        }
        None => cheapest_antigravity_model(&catalog_models)
            .or_else(|| {
                catalog_models
                    .iter()
                    .find(|m| **m == default_model)
                    .cloned()
            })
            .or_else(|| catalog_models.first().cloned())
            .unwrap_or(default_model),
    };

    // --- Stage 3: auth-lifecycle wiring (catalog reload, picker, fallback, switch) ---
    run_wiring_checks_for_contract(
        &provider_id,
        &native_antigravity_wiring_contract(),
        &selected,
        &catalog_models,
        &mut checks,
    );

    // --- Stage 4: API-dependent checkpoints (live native runtime) ---
    let mut spend = DoctorSpend::default();
    if tier == DoctorTier::Full {
        run_native_antigravity_api_checks(&selected, &mut checks, &mut spend).await;
    } else {
        for checkpoint in API_DEPENDENT_CHECKPOINTS {
            checks.push(DoctorCheck::skipped(
                checkpoint,
                label_for(checkpoint),
                format!(
                    "{} tier: requires --tier full (spends quota)",
                    tier.as_str()
                ),
            ));
        }
    }

    Ok(finish_report(
        provider_id,
        provider_label,
        selected,
        tier,
        checks,
        spend,
        native_antigravity_auth(&account),
    ))
}

/// Drive the three live native-Antigravity probes and fold their results into
/// the six API-dependent checkpoints, mirroring [`run_native_claude_api_checks`].
async fn run_native_antigravity_api_checks(
    selected: &str,
    checks: &mut Vec<DoctorCheck>,
    spend: &mut DoctorSpend,
) {
    // Non-streaming completion.
    match run_live_antigravity_native_smoke(selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::NON_STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
                "received expected completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::NON_STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Streaming completion.
    match run_live_antigravity_native_stream_smoke(selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::STREAMING_CHAT_COMPLETION),
                "received expected streamed completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Tool call + derived execution/result/smoke checkpoints (one round-trip).
    match run_live_antigravity_native_tool_smoke(selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            let detail = tool_stage_detail(&stage);
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::passed(
                    checkpoint,
                    label_for(checkpoint),
                    detail.clone(),
                ));
            }
        }
        Err(error) => {
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::failed(
                    checkpoint,
                    label_for(checkpoint),
                    format_error_chain(&error),
                ));
            }
        }
    }

    // Reasoning capability (observe-only; never gates readiness).
    push_reasoning_check(
        run_live_antigravity_native_reasoning_smoke(selected).await,
        checks,
        spend,
    );
}

// === Generic native-runtime doctor =========================================
//
// Claude and Antigravity each have a bespoke `run_*_native_e2e` driver above
// because their credential/catalog stories are unusual (OAuth-vs-API-key mode
// pinning for Claude; Google project resolution + thought-signature replay for
// Antigravity). The remaining native-runtime providers (OpenAI OAuth, Gemini
// Code Assist, Cursor, GitHub Copilot, AWS Bedrock) share the same shape:
// resolve a credential, fetch the live catalog through the production runtime,
// then run the shared wiring + API probes. `run_generic_native_e2e` drives all
// of them from a single [`NativeProviderSpec`] so adding a provider is a small,
// declarative change rather than a copy of the ~200-line driver.

/// The native-runtime providers driven by the generic doctor (everything except
/// Claude and Antigravity, which keep their bespoke drivers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeProviderKind {
    OpenAi,
    Gemini,
    Cursor,
    Copilot,
    Bedrock,
    Jcode,
    Azure,
}

impl NativeProviderKind {
    /// Map a normalized auth-provider id to a generic native kind, if any.
    pub fn from_normalized(provider_id: &str) -> Option<Self> {
        match provider_id {
            "openai" => Some(Self::OpenAi),
            "gemini" => Some(Self::Gemini),
            "cursor" => Some(Self::Cursor),
            "copilot" => Some(Self::Copilot),
            "bedrock" => Some(Self::Bedrock),
            "jcode" => Some(Self::Jcode),
            "azure-openai" => Some(Self::Azure),
            _ => None,
        }
    }

    fn spec(self) -> NativeProviderSpec {
        match self {
            Self::OpenAi => NativeProviderSpec {
                provider_id: "openai",
                label: "OpenAI",
                contract: WiringContract {
                    api_method: "openai-oauth".to_string(),
                    route_provider: "OpenAI".to_string(),
                    expected_runtime: "openai",
                    expected_namespace: None,
                    switch_prefix: "openai-oauth:".to_string(),
                },
                auth_source: "OpenAI ChatGPT OAuth / API key via auth.json",
                auth_env_key: None,
                login_hint: "jcode login --provider openai",
            },
            Self::Gemini => NativeProviderSpec {
                provider_id: "gemini",
                label: "Google Gemini",
                contract: WiringContract {
                    api_method: "code-assist-oauth".to_string(),
                    route_provider: "Gemini".to_string(),
                    expected_runtime: "gemini",
                    expected_namespace: None,
                    switch_prefix: "gemini:".to_string(),
                },
                auth_source: "Gemini Code Assist Google OAuth via gemini_oauth.json",
                auth_env_key: None,
                login_hint: "jcode login --provider gemini",
            },
            Self::Cursor => NativeProviderSpec {
                provider_id: "cursor",
                label: "Cursor",
                contract: WiringContract {
                    api_method: "cursor".to_string(),
                    route_provider: "Cursor".to_string(),
                    expected_runtime: "cursor",
                    expected_namespace: None,
                    switch_prefix: "cursor:".to_string(),
                },
                auth_source: "Cursor API key / CLI session via auth.json",
                auth_env_key: Some("CURSOR_API_KEY"),
                login_hint: "jcode login --provider cursor",
            },
            Self::Copilot => NativeProviderSpec {
                provider_id: "copilot",
                label: "GitHub Copilot",
                contract: WiringContract {
                    api_method: "copilot".to_string(),
                    route_provider: "Copilot".to_string(),
                    expected_runtime: "copilot",
                    expected_namespace: None,
                    switch_prefix: "copilot:".to_string(),
                },
                auth_source: "GitHub Copilot device-flow token via hosts.json",
                auth_env_key: None,
                login_hint: "jcode login --provider copilot",
            },
            Self::Bedrock => NativeProviderSpec {
                provider_id: "bedrock",
                label: "AWS Bedrock",
                contract: WiringContract {
                    api_method: "bedrock".to_string(),
                    route_provider: "AWS Bedrock".to_string(),
                    expected_runtime: "bedrock",
                    expected_namespace: None,
                    switch_prefix: "bedrock:".to_string(),
                },
                auth_source: "AWS Bedrock API key / AWS credentials",
                auth_env_key: Some("AWS_BEARER_TOKEN_BEDROCK"),
                login_hint: "jcode login --provider bedrock",
            },
            Self::Jcode => NativeProviderSpec {
                provider_id: "jcode",
                label: "Jcode Subscription",
                // The transport is OpenAI-compatible internally, but the public
                // route identity is the managed Jcode subscription. Model
                // switches use a bare model id so they stay on that runtime.
                contract: WiringContract {
                    api_method: jcode_base::subscription_catalog::JCODE_ROUTE_API_METHOD
                        .to_string(),
                    route_provider: jcode_base::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
                        .to_string(),
                    expected_runtime: "jcode",
                    expected_namespace: None,
                    switch_prefix: String::new(),
                },
                auth_source: "Jcode subscription API key (JCODE_API_KEY)",
                auth_env_key: Some("JCODE_API_KEY"),
                login_hint: "jcode login --provider jcode",
            },
            Self::Azure => NativeProviderSpec {
                provider_id: "azure-openai",
                label: "Azure OpenAI",
                // Azure OpenAI reuses the OpenRouter transport (configured via
                // Azure env), so its routes carry the generic `openrouter`
                // api_method/label and switch with the `openrouter:` prefix while
                // keeping the `azure-openai` runtime identity.
                contract: WiringContract {
                    api_method: "openrouter".to_string(),
                    route_provider: "auto".to_string(),
                    expected_runtime: "azure-openai",
                    expected_namespace: None,
                    switch_prefix: "openrouter:".to_string(),
                },
                auth_source: "Azure OpenAI API key / Entra ID (AZURE_OPENAI_*)",
                auth_env_key: Some("AZURE_OPENAI_API_KEY"),
                login_hint: "jcode login --provider azure",
            },
        }
    }

    /// Build the production runtime for this provider, pinned to no model yet.
    /// Returns an error only when the runtime cannot be constructed at all (e.g.
    /// Copilot with no credential file); model selection happens later.
    fn build_runtime(self) -> anyhow::Result<std::sync::Arc<dyn jcode_base::provider::Provider>> {
        use anyhow::Context as _;
        use jcode_base::provider::Provider;
        let runtime: std::sync::Arc<dyn Provider> = match self {
            Self::OpenAi => {
                let credentials =
                    jcode_base::auth::codex::load_credentials().unwrap_or_else(|_| {
                        jcode_base::auth::codex::CodexCredentials {
                            access_token: String::new(),
                            refresh_token: String::new(),
                            id_token: None,
                            account_id: None,
                            expires_at: None,
                        }
                    });
                std::sync::Arc::new(jcode_provider_openai_runtime::OpenAIProvider::new(
                    credentials,
                ))
            }
            Self::Gemini => {
                std::sync::Arc::new(jcode_provider_gemini_runtime::GeminiProvider::new())
            }
            Self::Cursor => {
                std::sync::Arc::new(jcode_provider_cursor_runtime::CursorCliProvider::new())
            }
            Self::Copilot => {
                // `new()` requires a loadable GitHub token; fall back to an empty
                // token so the offline tier can still construct the runtime for
                // its static catalog. Live tiers resolve the real credential
                // separately and fail with a clear message if it is missing.
                //
                // Disable the startup prefetch grace window: the runtime's
                // `complete` blocks on `wait_for_init`, which is only released by
                // `detect_tier_and_set_default` (run from `prefetch_models`). With
                // the default grace window the doctor's immediate prefetch returns
                // early without marking init done, so the live probes would hang.
                jcode_base::env::set_var("JCODE_COPILOT_PREFETCH_STARTUP_GRACE_MS", "0");
                let runtime = match jcode_provider_copilot_runtime::CopilotApiProvider::new() {
                    Ok(runtime) => runtime,
                    Err(_) => jcode_provider_copilot_runtime::CopilotApiProvider::new_with_token(
                        String::new(),
                    ),
                };
                std::sync::Arc::new(runtime)
            }
            Self::Bedrock => {
                std::sync::Arc::new(jcode_base::provider::bedrock::BedrockProvider::new())
            }
            Self::Jcode => std::sync::Arc::new(jcode_base::provider::jcode::JcodeProvider::new()),
            Self::Azure => {
                // Azure OpenAI is the OpenRouter transport configured via Azure
                // env; apply that env (endpoint/key/header wiring) before building
                // so the runtime points at the user's Azure deployment.
                jcode_base::auth::azure::apply_runtime_env()
                    .context("apply Azure OpenAI runtime env")?;
                let runtime = jcode_provider_openrouter_runtime::OpenRouterProvider::new()
                    .context("construct Azure OpenAI (OpenRouter transport) runtime")?;
                // Azure exposes a single user-configured deployment rather than a
                // live catalog; pin the runtime to it so the catalog/picker
                // checks have a model to assert.
                if let Some(model) = jcode_base::auth::azure::load_model() {
                    let _ = runtime.set_model(&model);
                }
                std::sync::Arc::new(runtime)
            }
        };
        Ok(runtime)
    }

    /// Resolve (best-effort) the live credential the runtime would use, returning
    /// a short non-secret descriptor for the credential check detail. Errors are
    /// surfaced to fail the `auth_credential_loaded` checkpoint.
    async fn resolve_credential(self) -> anyhow::Result<String> {
        use anyhow::Context as _;
        match self {
            Self::OpenAi => {
                let credentials = jcode_base::auth::codex::load_credentials()
                    .context("load OpenAI credentials (run `jcode login --provider openai`)")?;
                if credentials.access_token.trim().is_empty() {
                    anyhow::bail!("resolved an empty OpenAI access token");
                }
                Ok("OpenAI credential resolved".to_string())
            }
            Self::Gemini => {
                let tokens = jcode_base::auth::gemini::load_or_refresh_tokens()
                    .await
                    .context("load Gemini OAuth tokens")?;
                if tokens.access_token.trim().is_empty() {
                    anyhow::bail!("resolved an empty Gemini access token");
                }
                Ok("Gemini Code Assist OAuth credential resolved".to_string())
            }
            Self::Cursor => {
                let key = jcode_base::auth::cursor::load_api_key()
                    .context("load Cursor credential (run `jcode login --provider cursor`)")?;
                if key.trim().is_empty() {
                    anyhow::bail!("resolved an empty Cursor credential");
                }
                Ok("Cursor credential resolved".to_string())
            }
            Self::Copilot => {
                let token = jcode_base::auth::copilot::load_github_token()
                    .context("load GitHub Copilot token (run `jcode login --provider copilot`)")?;
                if token.trim().is_empty() {
                    anyhow::bail!("resolved an empty GitHub Copilot token");
                }
                Ok("GitHub Copilot token resolved".to_string())
            }
            Self::Bedrock => {
                if !jcode_base::provider::bedrock::BedrockProvider::has_credentials() {
                    anyhow::bail!(
                        "no AWS Bedrock credentials found (set AWS_BEARER_TOKEN_BEDROCK, AWS_PROFILE, \
                         or AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY)"
                    );
                }
                Ok("AWS Bedrock credential resolved".to_string())
            }
            Self::Jcode => {
                if !jcode_base::subscription_catalog::has_credentials() {
                    anyhow::bail!(
                        "no Jcode subscription credential found (set JCODE_API_KEY or run \
                         `jcode login --provider jcode`)"
                    );
                }
                Ok("Jcode subscription credential resolved".to_string())
            }
            Self::Azure => {
                if !jcode_base::auth::azure::has_configuration() {
                    anyhow::bail!(
                        "Azure OpenAI is not fully configured (need AZURE_OPENAI_ENDPOINT plus an \
                         API key or Entra ID); run `jcode login --provider azure`"
                    );
                }
                Ok(format!(
                    "Azure OpenAI configured ({})",
                    jcode_base::auth::azure::method_detail()
                ))
            }
        }
    }

    /// Pick the cheapest sensible model from a catalog for a smoke run, by
    /// provider-specific heuristics. Returns `None` to let the caller fall back
    /// to the runtime default.
    fn cheapest_model(self, catalog: &[String]) -> Option<String> {
        let usable = |m: &&String| {
            let t = m.trim();
            !t.is_empty() && t != "default" && !t.contains("[1m]")
        };
        // Prefer cheaper "mini"/"flash"/"haiku"/"fast" tiers when present.
        let cheap_markers: &[&str] = match self {
            Self::OpenAi => &["mini", "nano"],
            Self::Gemini => &["flash"],
            Self::Cursor => &["composer", "fast", "mini"],
            Self::Copilot => &["mini", "haiku", "flash", "fast"],
            Self::Bedrock => &["haiku", "micro", "lite", "mini", "flash"],
            Self::Jcode => &["mini", "flash", "haiku", "lite", "nano"],
            Self::Azure => &["mini", "nano", "flash", "haiku"],
        };
        for marker in cheap_markers {
            if let Some(model) = catalog
                .iter()
                .filter(usable)
                .find(|m| m.to_ascii_lowercase().contains(marker))
            {
                return Some(model.clone());
            }
        }
        catalog.iter().find(usable).cloned()
    }
}

/// Declarative description of a generic native provider's doctor wiring.
struct NativeProviderSpec {
    /// Canonical, normalized provider id (also the ledger/report id).
    provider_id: &'static str,
    /// Human display label for messages and the report.
    label: &'static str,
    /// jcode-side routing/activation contract for the wiring checkpoints.
    contract: WiringContract,
    /// Non-secret description of the credential source for the ledger.
    auth_source: &'static str,
    /// Env var to associate with the credential (for `non_secret`), if any.
    auth_env_key: Option<&'static str>,
    /// `jcode login` hint surfaced when the credential cannot be resolved.
    login_hint: &'static str,
}

/// Run the strict provider/model diagnostic for a generic native-runtime
/// provider (OpenAI, Gemini, Cursor, Copilot, Bedrock).
///
/// Drives the production runtime end to end: credential resolution, the live
/// model catalog (via the runtime's own `prefetch_models`), the shared
/// auth-lifecycle wiring checks, and (on the full tier) live non-streaming,
/// streaming, and tool-call probes through the exact code path a real session
/// uses. Records the same strict checkpoints so the coverage ledger can promote
/// the provider to READY.
pub async fn run_generic_native_e2e(
    kind: NativeProviderKind,
    requested_model: Option<&str>,
    tier: DoctorTier,
) -> anyhow::Result<DoctorReport> {
    let spec = kind.spec();
    let provider_id = spec.provider_id.to_string();
    let provider_label = spec.label.to_string();
    let auth = || LiveVerificationAuth::non_secret(spec.auth_source, spec.auth_env_key);
    let mut checks: Vec<DoctorCheck> = Vec::new();

    // --- Stage 1: credential resolution ---
    if tier.requires_api_key() {
        match kind.resolve_credential().await {
            Ok(detail) => checks.push(DoctorCheck::passed(
                checkpoints::AUTH_CREDENTIAL_LOADED,
                label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                detail,
            )),
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::AUTH_CREDENTIAL_LOADED,
                    label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                    format!("{error}. Run `{}` to sign in.", spec.login_hint),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    auth(),
                ));
            }
        }
    } else {
        checks.push(DoctorCheck::skipped(
            checkpoints::AUTH_CREDENTIAL_LOADED,
            label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
            "offline tier: no credential required".to_string(),
        ));
    }

    // Build the production runtime (cheap; no blocking network in any ctor).
    let runtime = match kind.build_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            checks.push(DoctorCheck::failed(
                checkpoints::AUTH_CREDENTIAL_LOADED,
                label_for(checkpoints::AUTH_CREDENTIAL_LOADED),
                format!("could not construct the {} runtime: {error}", spec.label),
            ));
            return Ok(finish_report(
                provider_id,
                provider_label,
                requested_model.unwrap_or("").to_string(),
                tier,
                checks,
                DoctorSpend::default(),
                auth(),
            ));
        }
    };

    // --- Stage 2: live model catalog (or static for offline) ---
    let catalog_models: Vec<String> = if tier.requires_api_key() {
        match runtime.prefetch_models().await {
            Ok(()) => {
                let models = runtime.available_models_display();
                if models.is_empty() {
                    checks.push(DoctorCheck::failed(
                        checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                        label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                        format!("live {} catalog returned no models", spec.label),
                    ));
                    return Ok(finish_report(
                        provider_id,
                        provider_label,
                        requested_model.unwrap_or("").to_string(),
                        tier,
                        checks,
                        DoctorSpend::default(),
                        auth(),
                    ));
                }
                checks.push(DoctorCheck::passed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!("{} live model(s) available", models.len()),
                ));
                models
            }
            Err(error) => {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format_error_chain(&error),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    requested_model.unwrap_or("").to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    auth(),
                ));
            }
        }
    } else {
        checks.push(DoctorCheck::skipped(
            checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
            format!(
                "offline tier: using known {} model ids (no network)",
                spec.label
            ),
        ));
        runtime.available_models_display()
    };

    // Pick the model under test.
    let default_model = runtime.model();
    let selected = match requested_model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => {
            if tier.requires_api_key() && !catalog_models.iter().any(|m| m == model) {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    label_for(checkpoints::MODEL_CATALOG_LIVE_ENDPOINT),
                    format!(
                        "requested model `{model}` is not in the live catalog ({} model(s): {})",
                        catalog_models.len(),
                        truncate_list(&catalog_models)
                    ),
                ));
                return Ok(finish_report(
                    provider_id,
                    provider_label,
                    model.to_string(),
                    tier,
                    checks,
                    DoctorSpend::default(),
                    auth(),
                ));
            }
            model.to_string()
        }
        None => kind
            .cheapest_model(&catalog_models)
            .or_else(|| {
                catalog_models
                    .iter()
                    .find(|m| **m == default_model)
                    .cloned()
            })
            .or_else(|| catalog_models.first().cloned())
            .unwrap_or(default_model),
    };

    // --- Stage 3: auth-lifecycle wiring (catalog reload, picker, fallback, switch) ---
    run_wiring_checks_for_contract(
        spec.provider_id,
        &spec.contract,
        &selected,
        &catalog_models,
        &mut checks,
    );

    // --- Stage 4: API-dependent checkpoints (live native runtime) ---
    let mut spend = DoctorSpend::default();
    if tier == DoctorTier::Full {
        // Pin the model before running the live probes so the runtime uses it.
        if let Err(error) = runtime.set_model(&selected) {
            for checkpoint in API_DEPENDENT_CHECKPOINTS {
                checks.push(DoctorCheck::failed(
                    checkpoint,
                    label_for(checkpoint),
                    format!("could not select model `{selected}`: {error}"),
                ));
            }
        } else {
            run_generic_native_api_checks(
                runtime.as_ref(),
                &selected,
                spec.label,
                &mut checks,
                &mut spend,
            )
            .await;
        }
    } else {
        for checkpoint in API_DEPENDENT_CHECKPOINTS {
            checks.push(DoctorCheck::skipped(
                checkpoint,
                label_for(checkpoint),
                format!(
                    "{} tier: requires --tier full (spends balance)",
                    tier.as_str()
                ),
            ));
        }
    }

    Ok(finish_report(
        provider_id,
        provider_label,
        selected,
        tier,
        checks,
        spend,
        auth(),
    ))
}

/// Drive the three generic live native probes and fold their results into the
/// six API-dependent checkpoints, mirroring [`run_native_claude_api_checks`].
async fn run_generic_native_api_checks(
    provider: &dyn jcode_base::provider::Provider,
    selected: &str,
    label: &str,
    checks: &mut Vec<DoctorCheck>,
    spend: &mut DoctorSpend,
) {
    // Non-streaming completion.
    match run_live_native_provider_smoke(provider, selected, label).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::NON_STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
                "received expected completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::NON_STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Streaming completion.
    match run_live_native_provider_stream_smoke(provider, selected, label).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::STREAMING_CHAT_COMPLETION),
                "received expected streamed completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Tool call + derived execution/result/smoke checkpoints (one round-trip).
    match run_live_native_provider_tool_smoke(provider, selected, label).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            let detail = tool_stage_detail(&stage);
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::passed(
                    checkpoint,
                    label_for(checkpoint),
                    detail.clone(),
                ));
            }
        }
        Err(error) => {
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::failed(
                    checkpoint,
                    label_for(checkpoint),
                    format_error_chain(&error),
                ));
            }
        }
    }

    // Reasoning capability (observe-only; never gates readiness).
    push_reasoning_check(
        run_live_native_provider_reasoning_smoke(provider, selected, label).await,
        checks,
        spend,
    );
}

/// The jcode-side wiring a given compat profile is expected to activate.
///
/// Most OpenAI-compatible profiles route through the generic
/// `openai-compatible` runtime with a per-profile catalog namespace and an
/// `openai-compatible:<id>` api_method. A few profile ids deliberately collide
/// with native login providers (`anthropic-api`→Anthropic, `openai-api`→OpenAI)
/// and jcode remaps them to their native runtimes. The doctor must assert the
/// *native* wiring for those, not the generic compat contract, or the routing
/// checkpoints fail even though the live API works.
struct WiringContract {
    /// The api_method string the live-catalog routes should carry.
    api_method: String,
    /// The provider display name to stamp on synthesized routes.
    route_provider: String,
    /// `expected_runtime` for the AuthChanged activation.
    expected_runtime: &'static str,
    /// `expected_catalog_namespace` for the AuthChanged activation, if any.
    expected_namespace: Option<String>,
    /// The `provider:` prefix a model-switch request must produce. An empty
    /// prefix means the runtime deliberately expects the bare model id.
    switch_prefix: String,
}

fn wiring_contract(profile: OpenAiCompatibleProfile) -> WiringContract {
    match jcode_base::auth::lifecycle::normalized_auth_provider_id(Some(profile.id)) {
        Some("claude-api") => WiringContract {
            api_method: "claude-api".to_string(),
            route_provider: "Anthropic".to_string(),
            expected_runtime: "claude-api",
            expected_namespace: None,
            switch_prefix: "claude-api:".to_string(),
        },
        Some("openai-api") => WiringContract {
            api_method: "openai-api".to_string(),
            route_provider: "OpenAI".to_string(),
            expected_runtime: "openai-api",
            expected_namespace: None,
            switch_prefix: "openai-api:".to_string(),
        },
        _ => WiringContract {
            api_method: format!("openai-compatible:{}", profile.id),
            route_provider: profile.display_name.to_string(),
            expected_runtime: "openai-compatible",
            expected_namespace: Some(profile.id.to_string()),
            switch_prefix: format!("{}:", profile.id),
        },
    }
}

fn run_wiring_checks(
    profile: OpenAiCompatibleProfile,
    selected: &str,
    catalog_models: &[String],
    checks: &mut Vec<DoctorCheck>,
) {
    run_wiring_checks_for_contract(
        profile.id,
        &wiring_contract(profile),
        selected,
        catalog_models,
        checks,
    );
}

/// Shared wiring-checkpoint driver used by both the OpenAI-compatible doctor and
/// the native Claude doctor. Builds the live-catalog routes a provider would
/// surface after auth, then exercises the production auth-activation +
/// catalog-invariant + model-switch logic against them.
fn run_wiring_checks_for_contract(
    auth_provider_id: &str,
    contract: &WiringContract,
    selected: &str,
    catalog_models: &[String],
    checks: &mut Vec<DoctorCheck>,
) {
    let api_method = contract.api_method.clone();
    let catalog_routes: Vec<ModelRoute> = catalog_models
        .iter()
        .map(|model| ModelRoute {
            model: model.clone(),
            provider: contract.route_provider.clone(),
            api_method: api_method.clone(),
            available: true,
            detail: "live-catalog route".to_string(),
            cheapness: None,
        })
        .collect();

    let auth = AuthChanged {
        provider: jcode_base::protocol::AuthProviderId::new(auth_provider_id),
        credential_source: None,
        auth_method: None,
        expected_runtime: Some(RuntimeProviderKey::new(contract.expected_runtime)),
        expected_catalog_namespace: contract
            .expected_namespace
            .as_deref()
            .map(CatalogNamespace::new),
    };
    let activation = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));

    // Provider-matched, available routes are what the picker would surface.
    let provider_entries: Vec<String> = catalog_routes
        .iter()
        .filter(|route| {
            route.available
                && (route.api_method.eq_ignore_ascii_case(&api_method)
                    || route.api_method.eq_ignore_ascii_case(auth_provider_id))
        })
        .map(|route| route.model.clone())
        .collect();

    let catalog_report = validate_catalog_invariants(&activation, Some(selected), &catalog_routes);

    // Catalog hot reload.
    if catalog_report.ok() {
        checks.push(DoctorCheck::passed(
            checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
            label_for(checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION),
            format!("{} catalog route(s) reloaded", catalog_routes.len()),
        ));
    } else {
        checks.push(DoctorCheck::failed(
            checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
            label_for(checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION),
            catalog_report
                .warning_message()
                .unwrap_or_else(|| "catalog hot-reload invariant failed".to_string()),
        ));
    }

    // Picker shows live models.
    if provider_entries.is_empty() {
        checks.push(DoctorCheck::failed(
            checkpoints::PICKER_LIVE_MODELS,
            label_for(checkpoints::PICKER_LIVE_MODELS),
            "picker had no provider entries after auth".to_string(),
        ));
    } else if provider_entries.iter().any(|entry| entry == selected) {
        checks.push(DoctorCheck::passed(
            checkpoints::PICKER_LIVE_MODELS,
            label_for(checkpoints::PICKER_LIVE_MODELS),
            format!(
                "{} model(s) in picker, selected `{selected}`",
                provider_entries.len()
            ),
        ));
    } else {
        checks.push(DoctorCheck::failed(
            checkpoints::PICKER_LIVE_MODELS,
            label_for(checkpoints::PICKER_LIVE_MODELS),
            format!("selected model `{selected}` not present in picker entries"),
        ));
    }

    // Picker fallback labeling: every provider-matched route must be live-catalog
    // backed, never a static fallback.
    let matching_routes: Vec<&ModelRoute> = catalog_routes
        .iter()
        .filter(|route| route.available && route.provider == contract.route_provider)
        .collect();
    let from_live_catalog = matching_routes
        .iter()
        .all(|route| route.detail.contains("live-catalog"));
    let has_static_fallback = matching_routes.iter().any(|route| {
        route
            .detail
            .to_ascii_lowercase()
            .contains("static fallback")
    });
    if matching_routes.is_empty() {
        checks.push(DoctorCheck::failed(
            checkpoints::PICKER_FALLBACK_LABELING,
            label_for(checkpoints::PICKER_FALLBACK_LABELING),
            "no provider-matched catalog routes to label".to_string(),
        ));
    } else if from_live_catalog && !has_static_fallback {
        checks.push(DoctorCheck::passed(
            checkpoints::PICKER_FALLBACK_LABELING,
            label_for(checkpoints::PICKER_FALLBACK_LABELING),
            "all routes backed by live catalog (no static fallback)".to_string(),
        ));
    } else {
        checks.push(DoctorCheck::failed(
            checkpoints::PICKER_FALLBACK_LABELING,
            label_for(checkpoints::PICKER_FALLBACK_LABELING),
            "found static-fallback routes where live-catalog routes were expected".to_string(),
        ));
    }

    // Model switch route: switching to another model must produce a provider-explicit
    // request routed through this provider's api method.
    let switch_target = provider_entries
        .iter()
        .find(|model| model.as_str() != selected)
        .or_else(|| provider_entries.first());
    match switch_target {
        Some(target) => {
            let request = activation.model_switch_request("mock-auth", target);
            let request_ok = if contract.switch_prefix.is_empty() {
                request == target.as_str()
            } else {
                request.starts_with(&contract.switch_prefix)
            };
            if request_ok {
                checks.push(DoctorCheck::passed(
                    checkpoints::MODEL_SWITCH_ROUTE,
                    label_for(checkpoints::MODEL_SWITCH_ROUTE),
                    format!("switch request `{request}` routed via `{api_method}`"),
                ));
            } else {
                checks.push(DoctorCheck::failed(
                    checkpoints::MODEL_SWITCH_ROUTE,
                    label_for(checkpoints::MODEL_SWITCH_ROUTE),
                    format!(
                        "model switch produced unexpected request `{request}` (expected {}model id)",
                        if contract.switch_prefix.is_empty() {
                            "bare "
                        } else {
                            contract.switch_prefix.as_str()
                        }
                    ),
                ));
            }
        }
        None => checks.push(DoctorCheck::failed(
            checkpoints::MODEL_SWITCH_ROUTE,
            label_for(checkpoints::MODEL_SWITCH_ROUTE),
            "no switch target available from picker entries".to_string(),
        )),
    }
}

async fn run_full_api_checks(
    profile: OpenAiCompatibleProfile,
    api_key: &str,
    selected: &str,
    checks: &mut Vec<DoctorCheck>,
    spend: &mut DoctorSpend,
) {
    // Non-streaming completion.
    match run_live_openai_compatible_smoke(profile, api_key, selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::NON_STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
                "received expected completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::NON_STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::NON_STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Streaming completion.
    match run_live_openai_compatible_stream_smoke(profile, api_key, selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            checks.push(DoctorCheck::passed(
                checkpoints::STREAMING_CHAT_COMPLETION,
                label_for(checkpoints::STREAMING_CHAT_COMPLETION),
                "received expected streamed completion".to_string(),
            ));
        }
        Err(error) => checks.push(DoctorCheck::failed(
            checkpoints::STREAMING_CHAT_COMPLETION,
            label_for(checkpoints::STREAMING_CHAT_COMPLETION),
            format_error_chain(&error),
        )),
    }

    // Tool call + derived execution/result/smoke checkpoints (one round-trip).
    match run_live_openai_compatible_tool_smoke(profile, api_key, selected).await {
        Ok(stage) => {
            spend.accumulate(stage.evidence.get("usage"), stage.evidence.get("cost"));
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::passed(
                    checkpoint,
                    label_for(checkpoint),
                    "tool call parsed and executed".to_string(),
                ));
            }
        }
        Err(error) => {
            for checkpoint in [
                checkpoints::TOOL_CALL_PARSE,
                checkpoints::TOOL_EXECUTION_LOOP,
                checkpoints::TOOL_RESULT_FOLLOWUP,
                checkpoints::REAL_JCODE_TOOL_SMOKE,
            ] {
                checks.push(DoctorCheck::failed(
                    checkpoint,
                    label_for(checkpoint),
                    format_error_chain(&error),
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_report(
    provider_id: String,
    provider_label: String,
    model: String,
    tier: DoctorTier,
    checks: Vec<DoctorCheck>,
    spend: DoctorSpend,
    auth: LiveVerificationAuth,
) -> DoctorReport {
    // A tier passes when none of its non-skipped checks failed.
    let tier_passed = !checks.iter().any(|check| check.is_failure());
    // Strict passes only on the full tier with every strict checkpoint passed.
    let strict_passed = tier == DoctorTier::Full
        && live_tests::strict_provider_model_coverage_checkpoint_ids().all(|checkpoint| {
            checks.iter().any(|check| {
                check.checkpoint == checkpoint
                    && check.status == LiveVerificationStageStatus::Passed
            })
        });

    record_event(
        &provider_id,
        &provider_label,
        &model,
        tier,
        &checks,
        &spend,
        auth,
        strict_passed || tier_passed,
    );

    DoctorReport {
        provider_id,
        provider_label,
        model,
        tier,
        checks,
        tier_passed,
        strict_passed,
        spend,
    }
}

/// Build the [`LiveVerificationAuth`] for an OpenAI-compatible doctor run from a
/// resolved env-var key (or mark it offline when no key is present).
fn compat_auth(api_key: Option<&str>, api_key_env: &str, env_file: &str) -> LiveVerificationAuth {
    match api_key {
        Some(key) if !key.trim().is_empty() => LiveVerificationAuth::from_secret(
            format!("{api_key_env} via {env_file}"),
            Some(api_key_env),
            key,
        ),
        _ => LiveVerificationAuth::non_secret("provider-doctor (offline)", Some(api_key_env)),
    }
}

#[allow(clippy::too_many_arguments)]
fn record_event(
    provider_id: &str,
    provider_label: &str,
    model: &str,
    tier: DoctorTier,
    checks: &[DoctorCheck],
    spend: &DoctorSpend,
    auth: LiveVerificationAuth,
    overall_passed: bool,
) {
    let mut stages: Vec<LiveVerificationStage> = Vec::new();
    let mut expected: Vec<&'static str> = Vec::new();
    let mut capabilities: Vec<&'static str> = Vec::new();
    for check in checks {
        expected.push(check.checkpoint);
        let stage = match check.status {
            LiveVerificationStageStatus::Passed => {
                capabilities.push(check.checkpoint);
                LiveVerificationStage::passed(check.checkpoint)
                    .with_evidence("detail", serde_json::json!(check.detail))
            }
            LiveVerificationStageStatus::Failed => {
                LiveVerificationStage::failed(check.checkpoint, check.detail.clone())
            }
            LiveVerificationStageStatus::Skipped => {
                LiveVerificationStage::skipped(check.checkpoint, check.detail.clone())
            }
            LiveVerificationStageStatus::Blocked => {
                LiveVerificationStage::blocked(check.checkpoint, check.detail.clone())
            }
            LiveVerificationStageStatus::NotRun => {
                LiveVerificationStage::not_run(check.checkpoint, check.detail.clone())
            }
        };
        stages.push(stage);
    }

    let result = if overall_passed {
        LiveVerificationResult::Passed
    } else {
        LiveVerificationResult::Failed
    };

    let mut event = LiveVerificationEvent::new(
        "provider_doctor_strict_e2e",
        provider_id,
        provider_label,
        auth,
        result,
    )
    .with_expected_checkpoints(expected)
    .with_capabilities(capabilities)
    .with_stages(stages)
    .with_metadata("doctor_tier", serde_json::json!(tier.as_str()))
    .with_metadata(
        "checkpoint_taxonomy_version",
        serde_json::json!(live_tests::CHECKPOINT_TAXONOMY_VERSION),
    )
    .with_metadata("spend", spend.to_json());
    if !model.trim().is_empty() {
        event = event.with_model(model.to_string());
    }
    if let Err(error) = live_tests::append_event(&event) {
        eprintln!("provider-doctor: failed to record live verification event: {error}");
    }
}

fn truncate_list(models: &[String]) -> String {
    let shown: Vec<&str> = models.iter().take(8).map(String::as_str).collect();
    let mut out = shown.join(", ");
    if models.len() > shown.len() {
        out.push_str(&format!(", +{} more", models.len() - shown.len()));
    }
    out
}

/// Render an `anyhow::Error` as its full cause chain (`outer: cause: root`).
///
/// The doctor wraps probe failures with high-level context (e.g. "open native
/// provider stream"), so `format_error_chain(&error)` alone shows only that outer label
/// and hides the actionable root cause (the live HTTP/JSON error). Joining the
/// chain surfaces the real reason a checkpoint failed in the report and ledger.
fn format_error_chain(error: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in error.chain() {
        let message = cause.to_string();
        // Skip empty or exact-duplicate adjacent frames so the chain stays tight.
        if message.trim().is_empty() {
            continue;
        }
        if parts.last().map(String::as_str) == Some(message.as_str()) {
            continue;
        }
        parts.push(message);
    }
    if parts.is_empty() {
        error.to_string()
    } else {
        parts.join(": ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_error_chain_joins_context_with_root_cause() {
        use anyhow::Context as _;
        let root: anyhow::Result<()> = Err(anyhow::anyhow!("HTTP 429 usage_limit_reached"));
        let wrapped = root
            .context("native provider stream event error")
            .context("open native provider stream")
            .unwrap_err();
        let rendered = format_error_chain(&wrapped);
        // Outer-most context first, root cause last, all joined.
        assert_eq!(
            rendered,
            "open native provider stream: native provider stream event error: HTTP 429 usage_limit_reached"
        );
    }

    #[test]
    fn format_error_chain_dedupes_and_handles_single_error() {
        let single = anyhow::anyhow!("standalone failure");
        assert_eq!(format_error_chain(&single), "standalone failure");
    }

    #[test]
    fn native_provider_kind_maps_every_generic_id() {
        for (id, expected) in [
            ("openai", NativeProviderKind::OpenAi),
            ("gemini", NativeProviderKind::Gemini),
            ("cursor", NativeProviderKind::Cursor),
            ("copilot", NativeProviderKind::Copilot),
            ("bedrock", NativeProviderKind::Bedrock),
            ("jcode", NativeProviderKind::Jcode),
            ("azure-openai", NativeProviderKind::Azure),
        ] {
            assert_eq!(NativeProviderKind::from_normalized(id), Some(expected));
        }
        // Native providers with bespoke drivers are intentionally not generic.
        assert_eq!(NativeProviderKind::from_normalized("claude"), None);
        assert_eq!(NativeProviderKind::from_normalized("antigravity"), None);
        assert_eq!(NativeProviderKind::from_normalized("openrouter"), None);
    }

    #[test]
    fn native_provider_specs_are_self_consistent() {
        // Every generic kind's spec must carry a switch_prefix that the wiring
        // contract's api_method-derived routes will satisfy, and a stable id.
        for kind in [
            NativeProviderKind::OpenAi,
            NativeProviderKind::Gemini,
            NativeProviderKind::Cursor,
            NativeProviderKind::Copilot,
            NativeProviderKind::Bedrock,
            NativeProviderKind::Jcode,
            NativeProviderKind::Azure,
        ] {
            let spec = kind.spec();
            assert!(!spec.provider_id.is_empty(), "{kind:?} has empty id");
            assert!(!spec.label.is_empty(), "{kind:?} has empty label");
            if kind == NativeProviderKind::Jcode {
                assert!(
                    spec.contract.switch_prefix.is_empty(),
                    "Jcode switches must use bare managed model ids"
                );
            } else {
                assert!(
                    spec.contract.switch_prefix.ends_with(':'),
                    "{kind:?} switch_prefix must end with ':'"
                );
            }
            // Round-trips through the id map.
            assert_eq!(
                NativeProviderKind::from_normalized(spec.provider_id),
                Some(kind),
                "{kind:?} id does not round-trip"
            );
        }
    }

    #[test]
    fn spend_accumulates_openai_style_usage_and_cost() {
        let mut spend = DoctorSpend::default();
        spend.accumulate(
            Some(&serde_json::json!({
                "prompt_tokens": 100,
                "completion_tokens": 40,
                "total_tokens": 140
            })),
            Some(&serde_json::json!(0.0012)),
        );
        spend.accumulate(
            Some(&serde_json::json!({
                "prompt_tokens": 10,
                "completion_tokens": 5
            })),
            None,
        );
        assert_eq!(spend.billable_calls, 2);
        assert_eq!(spend.prompt_tokens, 110);
        assert_eq!(spend.completion_tokens, 45);
        // Second call has no total_tokens, so it is derived as prompt+completion.
        assert_eq!(spend.total_tokens, 155);
        assert!(spend.has_token_data);
        assert_eq!(spend.reported_cost_usd, Some(0.0012));
        assert!(spend.human_summary().contains("155 tokens"));
        assert!(spend.human_summary().contains("$0.001200"));
    }

    #[test]
    fn spend_handles_missing_usage_and_anthropic_style_keys() {
        let mut spend = DoctorSpend::default();
        // No usage at all (e.g. provider that omits it).
        spend.accumulate(None, None);
        assert_eq!(spend.billable_calls, 1);
        assert!(!spend.has_token_data);
        assert!(
            spend
                .human_summary()
                .contains("token usage not reported by provider")
        );

        // Anthropic-style input_tokens/output_tokens.
        spend.accumulate(
            Some(&serde_json::json!({"input_tokens": 7, "output_tokens": 3})),
            None,
        );
        assert_eq!(spend.prompt_tokens, 7);
        assert_eq!(spend.completion_tokens, 3);
        assert_eq!(spend.total_tokens, 10);
        assert!(spend.has_token_data);
    }

    #[test]
    fn native_doctor_supports_claude_and_antigravity() {
        assert!(native_doctor_supports_provider("claude"));
        assert!(native_doctor_supports_provider("anthropic"));
        assert!(native_doctor_supports_provider("antigravity"));
        // OpenAI-compatible profiles are driven by the generic doctor, not the
        // native path.
        assert!(!native_doctor_supports_provider("openrouter"));
        assert!(!native_doctor_supports_provider(
            "definitely-not-a-provider"
        ));
    }

    /// `native_doctor_supports_provider` lives in `jcode_base::auth::doctor`
    /// (so base's `live_tests` roster can call it) while the drivers live
    /// here. Keep the base predicate in sync with the driver roster: every
    /// generic `NativeProviderKind` id plus the bespoke claude/antigravity
    /// drivers must be accepted, and nothing else native-flavored.
    #[test]
    fn native_provider_roster_matches_base_predicate() {
        for kind in [
            NativeProviderKind::OpenAi,
            NativeProviderKind::Gemini,
            NativeProviderKind::Cursor,
            NativeProviderKind::Copilot,
            NativeProviderKind::Bedrock,
            NativeProviderKind::Jcode,
            NativeProviderKind::Azure,
        ] {
            let id = kind.spec().provider_id;
            assert!(
                native_doctor_supports_provider(id),
                "base predicate rejects generic native driver id {id:?}"
            );
        }
        for id in ["claude", "antigravity"] {
            assert!(
                native_doctor_supports_provider(id),
                "base predicate rejects bespoke native driver id {id:?}"
            );
        }
    }

    #[test]
    fn native_antigravity_contract_routes_via_https_prefix() {
        let contract = native_antigravity_wiring_contract();
        assert_eq!(contract.api_method, "https");
        assert_eq!(contract.route_provider, "Antigravity");
        assert_eq!(contract.expected_runtime, "antigravity");
        assert!(contract.expected_namespace.is_none());
        assert_eq!(contract.switch_prefix, "antigravity:");
    }

    #[test]
    fn native_jcode_contract_uses_managed_subscription_identity() {
        let contract = NativeProviderKind::Jcode.spec().contract;
        assert_eq!(
            contract.api_method,
            jcode_base::subscription_catalog::JCODE_ROUTE_API_METHOD
        );
        assert_eq!(
            contract.route_provider,
            jcode_base::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
        );
        assert_eq!(contract.expected_runtime, "jcode");
        assert!(contract.expected_namespace.is_none());
        assert!(contract.switch_prefix.is_empty());
    }

    #[test]
    fn cheapest_antigravity_model_prefers_gemini_flash() {
        let catalog = vec![
            "claude-opus-4-6-thinking".to_string(),
            "gemini-3.1-pro-high".to_string(),
            "gemini-3-flash".to_string(),
            "gpt-oss-120b-medium".to_string(),
        ];
        assert_eq!(
            cheapest_antigravity_model(&catalog).as_deref(),
            Some("gemini-3-flash")
        );
    }

    #[test]
    fn cheapest_antigravity_model_falls_back_to_any_gemini_then_any_model() {
        // No flash tier: any Gemini wins.
        let gemini_only = vec![
            "claude-sonnet-4-6".to_string(),
            "gemini-3.1-pro-low".to_string(),
        ];
        assert_eq!(
            cheapest_antigravity_model(&gemini_only).as_deref(),
            Some("gemini-3.1-pro-low")
        );
        // No Gemini at all: first non-alias model wins.
        let no_gemini = vec!["default".to_string(), "claude-sonnet-4-6".to_string()];
        assert_eq!(
            cheapest_antigravity_model(&no_gemini).as_deref(),
            Some("claude-sonnet-4-6")
        );
        // Only the alias: nothing usable.
        let alias_only = vec!["default".to_string()];
        assert!(cheapest_antigravity_model(&alias_only).is_none());
    }

    #[test]
    fn native_antigravity_auth_is_secret_free() {
        let with_account = native_antigravity_auth("user@example.com");
        // The source mentions the account but never carries a secret fingerprint.
        assert!(with_account.source.contains("user@example.com"));
        let anonymous = native_antigravity_auth("");
        assert!(anonymous.source.contains("Antigravity Google OAuth"));
    }

    #[test]
    fn tool_stage_detail_surfaces_multi_and_parallel_phases() {
        let verified = LiveVerificationStage::passed(checkpoints::TOOL_CALL_PARSE)
            .with_evidence("multi_tool_replay", serde_json::json!("verified"))
            .with_evidence("parallel_tool_calls", serde_json::json!("verified"));
        let detail = tool_stage_detail(&verified);
        assert!(detail.contains("tool call parsed and executed"));
        assert!(detail.contains("multi-call signature replay verified"));
        assert!(detail.contains("parallel tool calls verified"));

        let skipped = LiveVerificationStage::passed(checkpoints::TOOL_CALL_PARSE)
            .with_evidence("multi_tool_replay", serde_json::json!("skipped"))
            .with_evidence("parallel_tool_calls", serde_json::json!("skipped"));
        let detail = tool_stage_detail(&skipped);
        assert!(detail.contains("multi-call signature replay skipped"));
        assert!(detail.contains("parallel tool calls skipped"));

        // With no evidence the base string is unchanged (back-compat).
        let bare = LiveVerificationStage::passed(checkpoints::TOOL_CALL_PARSE);
        assert_eq!(tool_stage_detail(&bare), "tool call parsed and executed");
    }

    #[test]
    fn reasoning_stage_detail_describes_each_classification() {
        for (value, needle) in [
            ("streamed", "reasoning streamed"),
            ("opaque", "reasoning hidden but signaled"),
            ("none", "no reasoning signal observed"),
        ] {
            let stage = LiveVerificationStage::passed(checkpoints::REASONING_CAPABILITY)
                .with_evidence("reasoning_capability", serde_json::json!(value));
            assert!(
                reasoning_stage_detail(&stage).contains(needle),
                "classification {value} should mention {needle}"
            );
        }
    }

    #[test]
    fn push_reasoning_check_records_pass_for_clean_turn() {
        let mut checks = Vec::new();
        let mut spend = DoctorSpend::default();
        let stage = LiveVerificationStage::passed(checkpoints::REASONING_CAPABILITY)
            .with_evidence("reasoning_capability", serde_json::json!("opaque"));
        push_reasoning_check(Ok(stage), &mut checks, &mut spend);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].checkpoint, checkpoints::REASONING_CAPABILITY);
        assert_eq!(checks[0].status, LiveVerificationStageStatus::Passed);
        assert!(!checks[0].is_failure());
    }

    #[test]
    fn push_reasoning_check_skips_never_fails_on_probe_error() {
        // The observe-only reasoning checkpoint must never produce a failure that
        // could flip the tier to not-ready; a probe error is recorded as skipped.
        let mut checks = Vec::new();
        let mut spend = DoctorSpend::default();
        push_reasoning_check(
            Err(anyhow::anyhow!("network blip")),
            &mut checks,
            &mut spend,
        );
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, LiveVerificationStageStatus::Skipped);
        assert!(!checks[0].is_failure());
    }
}
