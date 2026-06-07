mod accessors;
mod account_failover;
pub mod activation;
pub mod anthropic;
pub mod antigravity;
pub mod bedrock;
mod catalog_routes;
pub mod claude;
pub mod copilot;
pub mod cursor;
mod dispatch;
mod failover;
mod fingerprint;
pub mod gemini;
pub mod jcode;
pub mod models;
mod multi_provider;
pub mod openai;
pub mod openai_request;
pub mod openrouter;
pub mod pricing;
mod registry;
mod route_builders;
mod routing;
mod selection;
mod startup;
mod state;

use crate::auth;
use crate::message::{Message, ToolDefinition};
use account_failover::{
    account_usage_probe, active_account_label_for_provider, maybe_annotate_limit_summary,
    same_provider_account_candidates, same_provider_account_failover_enabled,
    set_account_override_for_provider,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
#[cfg(test)]
use jcode_provider_core::FailoverDecision;
use registry::ProviderRegistry;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub use catalog_routes::{
    append_simplified_anthropic_model_routes, remote_current_openai_compatible_route_for_model,
    remote_model_is_server_copilot_only, remote_model_routes_fallback,
    remote_model_routes_lightweight_fallback, remote_model_should_offer_copilot_route,
    remote_openai_compatible_route_for_model, simplified_model_routes_for_picker,
};
pub use jcode_provider_core::cli_provider_arg_for_session_key;
pub use jcode_provider_core::{
    ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS, CHEAPNESS_REFERENCE_INPUT_TOKENS,
    CHEAPNESS_REFERENCE_OUTPUT_TOKENS, DEFAULT_CONTEXT_LIMIT, EventStream, JCODE_USER_AGENT,
    ModelCapabilities, ModelCatalogRefreshSummary, ModelRoute, ModelRouteApiMethod,
    NativeCompactionResult, NativeToolResult, NativeToolResultSender, PremiumMode, Provider,
    RouteBillingKind, RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource, RouteSelection,
    RuntimeKey, dedupe_model_routes, explicit_model_provider_prefix, model_name_for_provider,
    normalize_copilot_model_name, provider_from_model_key, shared_http_client,
    summarize_model_catalog_refresh,
};
pub use jcode_provider_core::{ProviderFailoverPrompt, parse_failover_prompt_message};
pub use route_builders::{
    build_anthropic_oauth_route, build_copilot_route, build_openai_api_key_route,
    build_openai_oauth_route, build_openrouter_auto_route, build_openrouter_endpoint_route,
    build_openrouter_fallback_provider_route, is_listable_model_name,
    listable_model_names_from_routes, openrouter_catalog_model_id,
};
pub(crate) use routing::{
    anthropic_api_key_route_availability, anthropic_oauth_route_availability,
    is_transient_transport_error, should_eager_detect_copilot_tier,
};

/// Whether reasoning deltas should be persisted in session history for later
/// provider context reconstruction.
///
/// Display is controlled separately by `display.show_thinking`. Persist only
/// when a provider request builder can safely send the stored block back in
/// the provider-native shape. Anthropic is included only because we preserve
/// its thinking signatures in `ContentBlock::AnthropicThinking`.
pub fn stores_reasoning_content_for_context(provider_name: &str) -> bool {
    if !crate::config::config().provider.preserve_reasoning_context {
        return false;
    }
    matches!(
        provider_name.to_ascii_lowercase().as_str(),
        "openrouter" | "anthropic" | "openai"
    )
}

fn cached_live_models_for_openai_compatible_profile(
    resolved: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Option<Vec<String>> {
    let cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&resolved.id)?;
    let source_api_base = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)?;
    let expected_api_base = crate::provider_catalog::normalize_api_base(&resolved.api_base)?;
    if source_api_base != expected_api_base {
        return None;
    }

    let models = cache
        .models
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

fn direct_openai_compatible_profile_routes(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> Vec<ModelRoute> {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let static_models = crate::provider_catalog::openai_compatible_profile_static_models(profile);
    let (mut models, from_live_catalog) =
        if let Some(models) = cached_live_models_for_openai_compatible_profile(&resolved) {
            (models, true)
        } else {
            crate::provider::openrouter::maybe_schedule_openai_compatible_profile_catalog_refresh(
                profile,
                "inactive direct profile route cache miss",
            );
            let mut models = static_models;
            if models.is_empty()
                && let Some(default_model) = resolved.default_model.as_ref()
                && !default_model.trim().is_empty()
            {
                models.push(default_model.trim().to_string());
            }
            (models, false)
        };

    let provider = resolved.display_name.clone();
    let api_method = format!("openai-compatible:{}", resolved.id);
    let detail = if from_live_catalog {
        resolved.api_base.clone()
    } else if resolved.api_base.trim().is_empty() {
        "fallback: static provider model list".to_string()
    } else {
        format!(
            "{}; fallback: static provider model list",
            resolved.api_base
        )
    };

    let mut routes = Vec::new();
    for model in models.drain(..) {
        if !is_listable_model_name(&model)
            || !crate::provider_catalog::openai_compatible_profile_model_supports_chat(
                &resolved.id,
                &model,
            )
            || routes.iter().any(|route: &ModelRoute| route.model == model)
        {
            continue;
        }

        routes.push(ModelRoute {
            model,
            provider: provider.clone(),
            api_method: api_method.clone(),
            available: true,
            detail: detail.clone(),
            cheapness: None,
        });
    }

    routes
}

fn standard_openrouter_profile_configured() -> bool {
    crate::provider_catalog::load_env_value_from_env_or_config(
        "OPENROUTER_API_KEY",
        "openrouter.env",
    )
    .is_some()
}

fn configured_standard_openrouter_profile_routes() -> Vec<ModelRoute> {
    let Some(cache) = jcode_provider_openrouter::load_disk_cache_entry_for_namespace("openrouter")
    else {
        return Vec::new();
    };

    let source_matches_openrouter = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)
        .map(|base| base.contains("openrouter.ai"))
        .unwrap_or(false);
    if !source_matches_openrouter {
        return Vec::new();
    }

    let available = standard_openrouter_profile_configured();
    cache
        .models
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| is_listable_model_name(model))
        .map(|model| build_openrouter_auto_route(&model, available, String::new()))
        .collect()
}

pub fn set_model_with_auth_refresh(provider: &dyn Provider, model: &str) -> Result<()> {
    match provider.set_model(model) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            let first_message = first_err.to_string();
            crate::logging::auth_event(
                "auth_changed_retry_after_set_model_failure",
                provider.name(),
                &[("reason", first_message.as_str())],
            );
            provider.on_auth_changed();
            provider.set_model(model).map_err(|second_err| {
                anyhow::anyhow!(
                    "{} (retried after reloading auth from disk: {})",
                    first_message,
                    second_err
                )
            })
        }
    }
}

use self::dispatch::CompletionMode;
pub use self::models::{
    AccountModelAvailability, AccountModelAvailabilityState, AnthropicModelCatalog,
    OpenAIModelCatalog, begin_anthropic_model_catalog_refresh, begin_openai_model_catalog_refresh,
    cached_anthropic_model_ids, cached_openai_model_ids,
    clear_all_model_unavailability_for_account, clear_all_provider_unavailability_for_account,
    clear_model_unavailable_for_account, clear_provider_unavailable_for_account,
    context_limit_for_model, context_limit_for_model_with_provider, fetch_anthropic_model_catalog,
    fetch_anthropic_model_catalog_oauth, fetch_openai_api_key_model_catalog,
    fetch_openai_context_limits, fetch_openai_model_catalog,
    finish_anthropic_model_catalog_refresh_for_scope, finish_openai_model_catalog_refresh,
    format_account_model_availability_detail, get_best_available_openai_model,
    is_model_available_for_account, known_anthropic_model_ids, known_openai_model_ids,
    model_availability_for_account, model_unavailability_detail_for_account,
    note_openai_model_catalog_refresh_attempt, persist_anthropic_model_catalog,
    persist_openai_model_catalog, populate_account_models, populate_anthropic_models,
    populate_context_limits, provider_for_model, provider_for_model_with_hint,
    provider_unavailability_detail_for_account, record_model_unavailable_for_account,
    record_provider_unavailable_for_account, refresh_openai_model_catalog_in_background,
    resolve_model_capabilities, should_refresh_anthropic_model_catalog,
    should_refresh_openai_model_catalog,
};
pub use self::selection::DefaultModelSelection;
use self::selection::{ActiveProvider, ProviderAvailability};
use self::state::ProviderState;
pub use self::state::{ProviderModelSelectionSource, ProviderRuntimeState, ProviderStateEvent};

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    /// Claude Code CLI provider
    claude: RwLock<Option<Arc<claude::ClaudeProvider>>>,
    /// Direct Anthropic API provider (no Python dependency)
    anthropic: RwLock<Option<Arc<anthropic::AnthropicProvider>>>,
    openai: RwLock<Option<Arc<openai::OpenAIProvider>>>,
    /// GitHub Copilot API provider (direct API, hot-swappable after login)
    copilot_api: RwLock<Option<Arc<copilot::CopilotApiProvider>>>,
    /// Antigravity provider (direct HTTPS, hot-swappable after login)
    antigravity: RwLock<Option<Arc<antigravity::AntigravityProvider>>>,
    /// Gemini provider (hot-swappable after login)
    gemini: RwLock<Option<Arc<gemini::GeminiProvider>>>,
    /// Cursor provider (native/direct API, hot-swappable after login)
    cursor: RwLock<Option<Arc<cursor::CursorCliProvider>>>,
    /// AWS Bedrock provider (native Converse/ConverseStream, IAM/SigV4)
    bedrock: RwLock<Option<Arc<bedrock::BedrockProvider>>>,
    /// OpenRouter API provider
    openrouter: RwLock<Option<Arc<openrouter::OpenRouterProvider>>>,
    /// Direct OpenAI-compatible runtimes keyed by profile id.
    ///
    /// These use the same wire protocol implementation as OpenRouter, but must
    /// not occupy the real OpenRouter slot. Keeping them separate prevents a
    /// compatible endpoint selection from corrupting later OpenRouter model
    /// switches, catalog display, or auth refresh handling.
    openai_compatible_profiles: RwLock<HashMap<String, Arc<openrouter::OpenRouterProvider>>>,
    active_openai_compatible_profile: RwLock<Option<String>>,
    active: RwLock<ActiveProvider>,
    /// Use Claude CLI instead of direct API (legacy mode)
    use_claude_cli: bool,
    /// Notifications generated during provider/account auto-selection.
    /// The TUI should drain and display these on session start.
    startup_notices: RwLock<Vec<String>>,
    /// Optional explicit provider lock set by CLI `--provider`.
    /// When present, cross-provider fallback is disabled.
    forced_provider: Option<ActiveProvider>,
}

impl MultiProvider {
    #[cfg(test)]
    fn same_provider_account_candidates(provider: ActiveProvider) -> Vec<String> {
        account_failover::same_provider_account_candidates(provider)
    }

    async fn complete_with_failover(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();

        let detected_active = self.active_provider();
        let active = if let Some(forced) = self.forced_provider {
            if detected_active != forced {
                crate::logging::warn(&format!(
                    "Provider lock corrected active provider from {} to {} before request",
                    Self::provider_label(detected_active),
                    Self::provider_label(forced),
                ));
                self.set_active_provider(forced);
            }
            forced
        } else {
            detected_active
        };
        let sequence = Self::fallback_sequence_for(active, self.forced_provider);
        let mut notes: Vec<String> = Vec::new();
        let mut failover_reason: Option<String> = None;
        let (estimated_input_chars, estimated_input_tokens) =
            Self::estimate_request_input(messages, tools, mode);

        for candidate in sequence {
            let label = Self::provider_label(candidate);
            let key = Self::provider_key(candidate);

            if candidate != active && failover_reason.is_some() {
                let prompt = self.build_failover_prompt(
                    active,
                    candidate,
                    failover_reason
                        .clone()
                        .unwrap_or_else(|| "provider unavailable".to_string()),
                    estimated_input_chars,
                    estimated_input_tokens,
                );
                return Err(anyhow::anyhow!(prompt.to_error_message()));
            }

            if !self.provider_is_configured(candidate) {
                let note = format!("{}: not configured", label);
                if candidate == active {
                    crate::logging::warn(&format!(
                        "Failover{}: skipping active provider {} (not configured)",
                        mode.log_suffix(),
                        label
                    ));
                }
                notes.push(note);
                continue;
            }

            if let Some(detail) = provider_unavailability_detail_for_account(key) {
                let note = format!("{}: {}", label, detail);
                if candidate == active {
                    crate::logging::warn(&format!(
                        "Failover{}: skipping active provider {} - {}",
                        mode.log_suffix(),
                        label,
                        detail
                    ));
                    failover_reason = Some(detail.clone());
                }
                notes.push(note);
                continue;
            }

            if let Some(reason) = self.provider_precheck_unavailable_reason(candidate) {
                let note = format!("{}: {}", label, reason);
                if candidate == active {
                    crate::logging::warn(&format!(
                        "Failover{}: skipping active provider {} - {}",
                        mode.log_suffix(),
                        label,
                        reason
                    ));
                    failover_reason = Some(reason.clone());
                }
                notes.push(note);
                record_provider_unavailable_for_account(key, &reason);
                continue;
            }

            let attempt = match mode {
                CompletionMode::Unified { system } => {
                    self.complete_on_provider(candidate, messages, tools, system, resume_session_id)
                        .await
                }
                CompletionMode::Split {
                    system_static,
                    system_dynamic,
                } => {
                    self.complete_split_on_provider(
                        candidate,
                        messages,
                        tools,
                        system_static,
                        system_dynamic,
                        resume_session_id,
                    )
                    .await
                }
            };

            match attempt {
                Ok(stream) => {
                    clear_provider_unavailable_for_account(key);
                    if candidate != active {
                        self.set_active_provider(candidate);
                        let from_label = Self::provider_label(active);
                        let to_label = Self::provider_label(candidate);
                        crate::logging::info(&format!(
                            "{}: switched from {} to {}",
                            mode.switch_log_prefix(),
                            from_label,
                            to_label
                        ));
                        self.startup_notices
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(format!(
                                "⚡ Auto-fallback: {} unavailable, switched to {}",
                                from_label, to_label
                            ));
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let summary =
                        maybe_annotate_limit_summary(candidate, Self::summarize_error(&err));
                    let decision = Self::classify_failover_error(&err);
                    crate::logging::info(&format!(
                        "Provider {} failed{}: {} (failover={} decision={})",
                        label,
                        mode.log_suffix(),
                        summary,
                        decision.should_failover(),
                        decision.as_str()
                    ));
                    notes.push(format!("{}: {}", label, summary));
                    if decision.should_failover() {
                        if decision.should_mark_provider_unavailable() {
                            record_provider_unavailable_for_account(key, &summary);
                        }
                        if candidate == active
                            && let Some(stream) = self
                                .try_same_provider_account_failover(
                                    candidate, messages, tools, mode, &summary, &mut notes,
                                )
                                .await?
                        {
                            return Ok(stream);
                        }
                        if candidate == active {
                            failover_reason = Some(summary);
                        }
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Err(self.no_provider_available_error(&notes))
    }

    fn openai_compatible_model_prefix(
        model: &str,
    ) -> Option<(crate::provider_catalog::OpenAiCompatibleProfile, &str)> {
        let (prefix, rest) = model.split_once(':')?;
        if explicit_model_provider_prefix(model).is_some() {
            return None;
        }
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }

        let profile = crate::provider_catalog::openai_compatible_profile_by_id(prefix)?;
        Some((profile, rest))
    }

    fn ensure_provider_lock_allows_model_target(
        &self,
        target: ActiveProvider,
        requested_model: &str,
    ) -> Result<()> {
        let Some(forced) = self.forced_provider else {
            return Ok(());
        };
        if forced == target {
            return Ok(());
        }
        anyhow::bail!(
            "Model '{}' targets {} but --provider is locked to {}. Remove the provider-specific model prefix or use `--provider {}`.",
            requested_model,
            Self::provider_label(target),
            Self::provider_label(forced),
            Self::provider_key(target),
        );
    }

    fn ensure_provider_lock_allows_openai_compatible_profile(
        &self,
        requested_model: &str,
    ) -> Result<()> {
        let Some(forced) = self.forced_provider else {
            return Ok(());
        };
        if forced == ActiveProvider::OpenRouter {
            return Ok(());
        }
        anyhow::bail!(
            "Model '{}' targets an OpenAI-compatible provider but --provider is locked to {}. Remove the provider-specific model prefix or use `--provider openai-compatible`.",
            requested_model,
            Self::provider_label(forced),
        );
    }

    fn set_model_on_provider(&self, provider: ActiveProvider, model: &str) -> Result<()> {
        self.set_model_on_provider_with_credential_modes(provider, model, None, None)
    }

    fn set_model_on_provider_with_credential_modes(
        &self,
        provider: ActiveProvider,
        model: &str,
        openai_credential_mode: Option<openai::OpenAICredentialMode>,
        anthropic_credential_mode: Option<anthropic::AnthropicCredentialMode>,
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        self.reconcile_auth_if_provider_missing(provider);

        match provider {
            ActiveProvider::Claude => {
                let model = model_name_for_provider(provider, model);
                if let Some(anthropic) = self.anthropic_provider() {
                    if let Some(mode) = anthropic_credential_mode {
                        anthropic.set_credential_mode(mode)?;
                    }
                    anthropic.set_model(&model)?;
                } else if let Some(claude) = self.claude_provider() {
                    claude.set_model(&model)?;
                } else {
                    anyhow::bail!(
                        "Claude credentials not available. Run `jcode login --provider claude` first."
                    );
                }
                self.set_active_provider(ActiveProvider::Claude);
                Ok(())
            }
            ActiveProvider::OpenAI => {
                let Some(openai) = self.openai_provider() else {
                    anyhow::bail!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` first."
                    );
                };
                if let Some(mode) = openai_credential_mode {
                    openai.set_credential_mode(mode)?;
                }
                openai.set_model(model)?;
                self.set_active_provider(ActiveProvider::OpenAI);
                Ok(())
            }
            ActiveProvider::Copilot => {
                let Some(copilot) = self.copilot_provider() else {
                    anyhow::bail!(
                        "GitHub Copilot credentials not available. Run `jcode login --provider copilot` first."
                    );
                };
                copilot.set_model(model)?;
                self.set_active_provider(ActiveProvider::Copilot);
                Ok(())
            }
            ActiveProvider::Antigravity => {
                let Some(antigravity) = self.antigravity_provider() else {
                    anyhow::bail!(
                        "Antigravity credentials not available. Run `jcode login --provider antigravity` first."
                    );
                };
                antigravity.set_model(model)?;
                self.set_active_provider(ActiveProvider::Antigravity);
                Ok(())
            }
            ActiveProvider::Gemini => {
                let Some(gemini) = self.gemini_provider() else {
                    anyhow::bail!(
                        "Gemini credentials not available. Run `jcode login --provider gemini` first."
                    );
                };
                gemini.set_model(model)?;
                self.set_active_provider(ActiveProvider::Gemini);
                Ok(())
            }
            ActiveProvider::Cursor => {
                let Some(cursor) = self.cursor_provider() else {
                    anyhow::bail!(
                        "Cursor credentials not available. Run `jcode login --provider cursor` first."
                    );
                };
                cursor.set_model(model)?;
                self.set_active_provider(ActiveProvider::Cursor);
                Ok(())
            }
            ActiveProvider::Bedrock => {
                let Some(bedrock) = self.bedrock_provider() else {
                    anyhow::bail!(
                        "AWS Bedrock credentials not available. Configure AWS credentials and region first."
                    );
                };
                bedrock.set_model(model)?;
                self.set_active_provider(ActiveProvider::Bedrock);
                Ok(())
            }
            ActiveProvider::OpenRouter => {
                self.clear_active_openai_compatible_profile();
                if self
                    .openrouter_provider()
                    .as_deref()
                    .map(|provider| !provider.supports_provider_routing_features())
                    .unwrap_or(true)
                {
                    let provider =
                        Arc::new(openrouter::OpenRouterProvider::new_openrouter_api_key_runtime()?);
                    *self
                        .openrouter
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
                }

                let Some(openrouter) = self.openrouter_provider() else {
                    anyhow::bail!(
                        "OpenRouter/OpenAI-compatible credentials not available. Set the configured API key or run `jcode login --provider openrouter` first."
                    );
                };
                openrouter.set_model(model)?;
                self.set_active_provider(ActiveProvider::OpenRouter);
                Ok(())
            }
        }
    }

    fn set_model_on_openai_compatible_profile(
        &self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
        model: &str,
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        if !crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
            anyhow::bail!(
                "{} credentials not available. Run `jcode login --provider {}` first.",
                resolved.display_name,
                resolved.id,
            );
        }

        let profile_id = resolved.id.clone();
        let registry = ProviderRegistry::new(self);
        let provider = {
            let existing = registry.compatible_profile(&profile_id).filter(|provider| {
                provider
                    .direct_openai_compatible_route_parts()
                    .and_then(|(_provider, api_method, _detail)| {
                        api_method
                            .strip_prefix("openai-compatible:")
                            .map(|profile| profile.trim().to_string())
                    })
                    .as_deref()
                    == Some(profile_id.as_str())
            });
            if let Some(provider) = existing {
                provider
            } else {
                let provider = Arc::new(
                    openrouter::OpenRouterProvider::new_openai_compatible_profile_runtime(profile)?,
                );
                registry.install_compatible_profile(profile_id.clone(), provider.clone());
                provider
            }
        };
        provider.set_model(model)?;
        registry.set_active_compatible_profile(profile_id);
        self.set_active_provider(ActiveProvider::OpenRouter);
        Ok(())
    }

    fn should_replace_openrouter_after_auth_change(
        existing: &openrouter::OpenRouterProvider,
        candidate: &openrouter::OpenRouterProvider,
    ) -> bool {
        if existing.supports_provider_routing_features()
            != candidate.supports_provider_routing_features()
        {
            return false;
        }

        let existing_direct = existing
            .direct_openai_compatible_route_parts()
            .map(|(_provider, api_method, _detail)| api_method);
        let candidate_direct = candidate
            .direct_openai_compatible_route_parts()
            .map(|(_provider, api_method, _detail)| api_method);

        existing_direct == candidate_direct
    }

    fn handle_auth_changed(&self, preserve_existing_openrouter_profile: bool) {
        crate::logging::auth_event("auth_changed_received", "multi-provider", &[]);
        // Auth just changed, so discard any stale full/fast snapshots before
        // using cheap local probes to hot-initialize newly configured providers.
        crate::auth::AuthStatus::invalidate_cache();

        if self.use_claude_cli {
            if self.claude_provider().is_none() && crate::auth::claude::load_credentials().is_ok() {
                crate::logging::info("Hot-initialized Claude CLI provider after auth change");
                *self
                    .claude
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(Arc::new(claude::ClaudeProvider::new()));
            }
        } else if self.anthropic_provider().is_none()
            && (crate::auth::claude::load_credentials().is_ok()
                || crate::provider_catalog::load_api_key_from_env_or_config(
                    "ANTHROPIC_API_KEY",
                    "anthropic.env",
                )
                .is_some())
        {
            crate::logging::info("Hot-initialized Anthropic provider after auth change");
            *self
                .anthropic
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(anthropic::AnthropicProvider::new()));
        }

        if let Some(openai) = self.openai_provider() {
            openai.reload_credentials_now();
        } else if let Ok(credentials) = crate::auth::codex::load_credentials() {
            crate::logging::info("Hot-initialized OpenAI provider after auth change");
            *self
                .openai
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(openai::OpenAIProvider::new(credentials)));
        }

        if openrouter::OpenRouterProvider::has_credentials() {
            match openrouter::OpenRouterProvider::new() {
                Ok(provider) => {
                    let should_install = if preserve_existing_openrouter_profile {
                        self.openrouter_provider()
                            .as_deref()
                            .map(|existing| {
                                Self::should_replace_openrouter_after_auth_change(
                                    existing, &provider,
                                )
                            })
                            .unwrap_or(true)
                    } else {
                        true
                    };
                    if should_install {
                        crate::logging::info(
                            "Hot-initialized OpenRouter/OpenAI-compatible provider after auth change",
                        );
                        *self
                            .openrouter
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(Arc::new(provider));
                    } else {
                        crate::logging::info(
                            "Preserved existing OpenRouter/OpenAI-compatible provider after unrelated auth change",
                        );
                    }
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "Failed to hot-initialize OpenRouter/OpenAI-compatible provider after auth change: {}",
                        e
                    ));
                }
            }
        }

        let already_has = self.copilot_provider().is_some();
        if !already_has {
            let status = crate::auth::AuthStatus::check_fast();
            if status.copilot_has_api_token {
                match copilot::CopilotApiProvider::new() {
                    Ok(p) => {
                        crate::logging::info("Hot-initialized Copilot API provider after login");
                        let provider = Arc::new(p);
                        let p_clone = provider.clone();
                        tokio::spawn(async move {
                            p_clone.detect_tier_and_set_default().await;
                        });
                        *self
                            .copilot_api
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Failed to hot-initialize Copilot API after login: {}",
                            e
                        ));
                    }
                }
            }
        }

        let already_has_antigravity = self.antigravity_provider().is_some();
        if !already_has_antigravity && crate::auth::antigravity::load_tokens().is_ok() {
            crate::logging::info("Hot-initialized Antigravity provider after login");
            *self
                .antigravity
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(antigravity::AntigravityProvider::new()));
        }

        let already_has_gemini = self.gemini_provider().is_some();
        if !already_has_gemini && crate::auth::gemini::load_tokens().is_ok() {
            crate::logging::info("Hot-initialized Gemini provider after login");
            *self
                .gemini
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(gemini::GeminiProvider::new()));
        }

        let already_has_cursor = self.cursor_provider().is_some();
        if !already_has_cursor
            && crate::auth::AuthStatus::check_fast()
                .assessment_for_provider(crate::provider_catalog::CURSOR_LOGIN_PROVIDER)
                .is_available()
        {
            crate::logging::info("Hot-initialized Cursor provider after login");
            *self
                .cursor
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(cursor::CursorCliProvider::new()));
        }

        let already_has_bedrock = self.bedrock_provider().is_some();
        if !already_has_bedrock && bedrock::BedrockProvider::has_credentials() {
            crate::logging::info("Hot-initialized AWS Bedrock provider after login");
            *self
                .bedrock
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(bedrock::BedrockProvider::new()));
        }

        if let Some(anthropic) = self.anthropic_provider() {
            Self::spawn_post_auth_model_refresh(anthropic, "Anthropic");
        }
        if let Some(claude) = self.claude_provider() {
            Self::spawn_post_auth_model_refresh(claude, "Claude");
        }
        if let Some(openai) = self.openai_provider() {
            Self::spawn_post_auth_model_refresh(openai, "OpenAI");
        }
        if let Some(antigravity) = self.antigravity_provider() {
            Self::spawn_post_auth_model_refresh(antigravity, "Antigravity");
        }
        if let Some(gemini) = self.gemini_provider() {
            Self::spawn_post_auth_model_refresh(gemini, "Gemini");
        }
        if let Some(cursor) = self.cursor_provider() {
            Self::spawn_post_auth_model_refresh(cursor, "Cursor");
        }
        if let Some(openrouter) = self.openrouter_provider() {
            Self::spawn_post_auth_model_refresh(openrouter, "OpenRouter");
        }
        if let Some(bedrock) = self.bedrock_provider() {
            Self::spawn_post_auth_model_refresh(bedrock, "AWS Bedrock");
        }
        crate::logging::auth_event("auth_changed_completed", "multi-provider", &[]);
    }

    pub(super) fn set_config_default_model(
        &self,
        model: &str,
        default_provider: Option<&str>,
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        // A configured default_provider is a routing decision, not just a
        // startup hint. Treat default_model as provider-local when the config
        // names a concrete provider/profile so global model-name heuristics
        // cannot undo that decision. This is especially important for
        // OpenAI-compatible gateways whose model IDs often look like built-in
        // OpenAI, Anthropic, or OpenRouter models.
        if let Some(pref) = default_provider.and_then(|pref| {
            let trimmed = pref.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        }) && let Some(selection) =
            Self::resolve_config_provider_selection(pref, crate::config::config())
        {
            return self.set_model_on_provider(selection.active_provider(), model);
        }

        self.set_model(model)
    }

    fn fork_model_switch_request(&self, active: ActiveProvider, current_model: &str) -> String {
        let prefix = match active {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    // OAuth/ApiKey emit their canonical model prefix; Auto keeps
                    // the bare provider key (route without pinning a credential).
                    anthropic
                        .credential_mode_snapshot()
                        .auth_route()
                        .map(|route| route.model_prefix())
                        .unwrap_or("claude")
                } else {
                    "claude"
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    openai
                        .credential_mode_snapshot()
                        .auth_route()
                        .map(|route| route.model_prefix())
                        .unwrap_or("openai")
                } else {
                    "openai"
                }
            }
            ActiveProvider::Copilot => "copilot",
            ActiveProvider::Antigravity => "antigravity",
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Cursor => "cursor",
            ActiveProvider::Bedrock => "bedrock",
            ActiveProvider::OpenRouter => {
                if let Some(openrouter) = self.active_openrouter_execution_provider()
                    && let Some((_provider, api_method, _detail)) =
                        openrouter.direct_openai_compatible_route_parts()
                    && let Some(profile_id) = api_method
                        .strip_prefix("openai-compatible:")
                        .map(str::trim)
                        .filter(|profile_id| !profile_id.is_empty())
                {
                    return format!("{profile_id}:{current_model}");
                }
                if let Some(openrouter) = self.openrouter_provider()
                    && let Some(provider_pin) = openrouter.explicit_provider_pin_for_current_model()
                {
                    return format!("openrouter:{current_model}@{provider_pin}");
                }
                "openrouter"
            }
        };
        format!("{prefix}:{current_model}")
    }
}

impl Default for MultiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MultiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.complete_with_failover(
            messages,
            tools,
            CompletionMode::Unified { system },
            resume_session_id,
        )
        .await
    }

    /// Split system prompt completion - delegates to underlying provider for better caching
    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.complete_with_failover(
            messages,
            tools,
            CompletionMode::Split {
                system_static,
                system_dynamic,
            },
            resume_session_id,
        )
        .await
    }

    fn name(&self) -> &str {
        match self.active_provider() {
            ActiveProvider::Claude => "Claude",
            ActiveProvider::OpenAI => "OpenAI",
            ActiveProvider::Copilot => "Copilot",
            ActiveProvider::Antigravity => "Antigravity",
            ActiveProvider::Gemini => "Gemini",
            ActiveProvider::Cursor => "Cursor",
            ActiveProvider::Bedrock => "Bedrock",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    fn display_name(&self) -> String {
        // The OpenRouter slot multiplexes the public aggregator and every
        // direct OpenAI-compatible profile (NVIDIA NIM, DeepSeek, ...). Ask the
        // active execution runtime for its own label so the UI reflects the
        // profile selected at runtime rather than the fixed "OpenRouter" name.
        if matches!(self.active_provider(), ActiveProvider::OpenRouter)
            && let Some(execution) = self.active_openrouter_execution_provider()
        {
            return execution.runtime_display_name();
        }
        self.name().to_string()
    }

    fn model(&self) -> String {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Prefer anthropic if available
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.model()
                } else if let Some(claude) = self.claude_provider() {
                    claude.model()
                } else {
                    "claude-opus-4-5-20251101".to_string()
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "gpt-5.5".to_string()),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "claude-sonnet-4".to_string()),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "default".to_string()),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "gemini-2.5-pro".to_string()),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "composer-2.5".to_string()),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string()),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string()),
        }
    }

    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        use jcode_provider_core::ResolvedCredential;
        match self.active_provider() {
            ActiveProvider::Claude => {
                let anthropic = self.anthropic_provider()?;
                Some(match anthropic.credential_mode_snapshot() {
                    anthropic::AnthropicCredentialMode::OAuth => ResolvedCredential::Oauth,
                    anthropic::AnthropicCredentialMode::ApiKey => ResolvedCredential::ApiKey,
                    // Auto prefers OAuth (Claude subscription) when available,
                    // otherwise falls back to the API key. Mirror that exactly.
                    anthropic::AnthropicCredentialMode::Auto => {
                        if crate::auth::claude::load_credentials().is_ok() {
                            ResolvedCredential::Oauth
                        } else {
                            ResolvedCredential::ApiKey
                        }
                    }
                })
            }
            ActiveProvider::OpenAI => {
                let openai = self.openai_provider()?;
                Some(match openai.credential_mode_snapshot() {
                    openai::OpenAICredentialMode::OAuth => ResolvedCredential::Oauth,
                    openai::OpenAICredentialMode::ApiKey => ResolvedCredential::ApiKey,
                    // Auto resolves to OAuth first when available, otherwise API key.
                    openai::OpenAICredentialMode::Auto => {
                        if crate::auth::codex::load_oauth_credentials().is_ok() {
                            ResolvedCredential::Oauth
                        } else {
                            ResolvedCredential::ApiKey
                        }
                    }
                })
            }
            _ => None,
        }
    }

    fn supports_image_input(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => self
                .anthropic_provider()
                .map(|provider| provider.supports_image_input())
                .or_else(|| {
                    self.claude_provider()
                        .map(|provider| provider.supports_image_input())
                })
                .unwrap_or(false),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();

        let requested_model = model.trim();
        if requested_model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        if let Some((profile, target_model)) = Self::openai_compatible_model_prefix(requested_model)
        {
            self.ensure_provider_lock_allows_openai_compatible_profile(requested_model)?;
            return self.set_model_on_openai_compatible_profile(profile, target_model);
        }

        // Provider-prefixed model names are explicit routing directives. They
        // must never silently fall through to another provider when the target
        // is unavailable or when --provider locks a different backend.
        if let Some((target, prefix, target_model)) =
            explicit_model_provider_prefix(requested_model)
        {
            self.ensure_provider_lock_allows_model_target(target, requested_model)?;
            // The single canonical parser decides whether this prefix pins a
            // dual-auth credential (and which provider/mode). Bare `claude:` /
            // `openai:` prefixes route without pinning a credential.
            let pinned = jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(prefix);
            let openai_credential_mode = pinned.and_then(|route| {
                matches!(
                    route.provider,
                    jcode_provider_core::DualAuthProvider::OpenAI
                )
                .then(|| match route.mode {
                    jcode_provider_core::AuthMode::ApiKey => openai::OpenAICredentialMode::ApiKey,
                    jcode_provider_core::AuthMode::Oauth => openai::OpenAICredentialMode::OAuth,
                })
            });
            let anthropic_credential_mode = pinned.and_then(|route| {
                matches!(
                    route.provider,
                    jcode_provider_core::DualAuthProvider::Anthropic
                )
                .then(|| match route.mode {
                    jcode_provider_core::AuthMode::ApiKey => {
                        anthropic::AnthropicCredentialMode::ApiKey
                    }
                    jcode_provider_core::AuthMode::Oauth => {
                        anthropic::AnthropicCredentialMode::OAuth
                    }
                })
            });
            if openai_credential_mode.is_some() || anthropic_credential_mode.is_some() {
                return self.set_model_on_provider_with_credential_modes(
                    target,
                    target_model,
                    openai_credential_mode,
                    anthropic_credential_mode,
                );
            }
            return self.set_model_on_provider(target, target_model);
        }

        // A CLI --provider lock means the model string is provider-local. Do
        // not apply global Claude/OpenAI/OpenRouter heuristics here: custom
        // OpenAI-compatible endpoints often use model IDs that look like other
        // providers' IDs, and GitHub Copilot uses Claude-looking dotted names.
        if let Some(forced) = self.forced_provider {
            return self.set_model_on_provider(forced, requested_model);
        }

        // Normalize Copilot-style model names (dots -> hyphens) to canonical form.
        // e.g. "claude-opus-4.6" -> "claude-opus-4-6" so Anthropic accepts it.
        let model = if let Some(canonical) = normalize_copilot_model_name(requested_model) {
            canonical
        } else {
            requested_model
        };

        if let Some((base_model, provider_pin)) = model.rsplit_once('@')
            && !provider_pin.trim().is_empty()
            && let Some(openrouter_model) = openrouter_catalog_model_id(base_model)
        {
            return self.set_model_on_provider(
                ActiveProvider::OpenRouter,
                &format!("{}@{}", openrouter_model, provider_pin),
            );
        }

        // Detect which provider this model belongs to when no explicit
        // --provider lock was requested.
        let target_provider = provider_for_model(model);
        if let Some(target_provider) = target_provider
            && let Some(target) = provider_from_model_key(target_provider)
        {
            self.set_model_on_provider(target, model)
        } else {
            // Unknown model - try current provider.
            self.set_model_on_provider(self.active_provider(), model)
        }
    }

    fn set_route_selection(&self, selection: &RouteSelection) -> Result<()> {
        let model = selection.model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        let routed_model = match &selection.runtime_key {
            RuntimeKey::ClaudeOAuth => format!("claude-oauth:{model}"),
            RuntimeKey::AnthropicApiKey => format!("claude-api:{model}"),
            RuntimeKey::OpenAIOAuth => format!("openai-oauth:{model}"),
            RuntimeKey::OpenAIApiKey => format!("openai-api:{model}"),
            RuntimeKey::OpenAiCompatible {
                profile_id: Some(profile_id),
            } => format!("{}:{model}", profile_id.trim()),
            RuntimeKey::OpenAiCompatible { profile_id: None } => model.to_string(),
            RuntimeKey::OpenRouter => {
                let provider = selection.provider_label.trim();
                if provider.is_empty()
                    || provider.eq_ignore_ascii_case("auto")
                    || model.contains('@')
                {
                    openrouter_catalog_model_id(model).unwrap_or_else(|| model.to_string())
                } else {
                    format!(
                        "{}@{}",
                        openrouter_catalog_model_id(model).unwrap_or_else(|| model.to_string()),
                        provider
                    )
                }
            }
            RuntimeKey::Copilot => format!("copilot:{model}"),
            RuntimeKey::Cursor => format!("cursor:{model}"),
            RuntimeKey::Bedrock => format!("bedrock:{model}"),
            RuntimeKey::Antigravity => format!("antigravity:{model}"),
            RuntimeKey::Gemini
            | RuntimeKey::CodeAssistOAuth
            | RuntimeKey::RemoteCatalog
            | RuntimeKey::Current
            | RuntimeKey::Other(_) => model.to_string(),
        };

        self.set_model(&routed_model)
    }

    fn available_models(&self) -> Vec<&'static str> {
        let mut models = Vec::new();
        models.extend_from_slice(ALL_CLAUDE_MODELS);
        models.extend_from_slice(ALL_OPENAI_MODELS);
        models
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.available_models_for_switching()
                } else if let Some(claude) = self.claude_provider() {
                    claude.available_models_for_switching()
                } else {
                    Vec::new()
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|openai| openai.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|copilot| copilot.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|antigravity| antigravity.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|gemini| gemini.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|cursor| cursor.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|bedrock| bedrock.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|openrouter| openrouter.available_models_for_switching())
                .unwrap_or_default(),
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        listable_model_names_from_routes(&self.model_routes())
    }

    fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        if let Some(model) = openrouter_catalog_model_id(model)
            && let Some(openrouter) = self.openrouter_provider()
        {
            return openrouter.available_providers_for_model(&model);
        }
        Vec::new()
    }

    fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        if let Some(model) = openrouter_catalog_model_id(model)
            && let Some(openrouter) = self.openrouter_provider()
        {
            return openrouter.provider_details_for_model(&model);
        }
        Vec::new()
    }

    fn preferred_provider(&self) -> Option<String> {
        if let Some(openrouter) = self.openrouter_provider()
            && matches!(
                *self
                    .active
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                ActiveProvider::OpenRouter
            )
        {
            return openrouter.preferred_provider();
        }
        None
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        catalog_routes::multiprovider_model_routes(self)
    }

    async fn prefetch_models(&self) -> Result<()> {
        let anthropic = self.anthropic_provider();
        let claude = self.claude_provider();
        let openai = self.openai_provider();
        let openrouter = self.openrouter_provider();
        let copilot = self
            .copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let antigravity = self.antigravity_provider();
        let gemini = self.gemini_provider();
        let cursor = self.cursor_provider();
        let bedrock = self.bedrock_provider();

        let (
            anthropic_result,
            claude_result,
            openai_result,
            openrouter_result,
            copilot_result,
            antigravity_result,
            gemini_result,
            cursor_result,
            bedrock_result,
        ) = tokio::join!(
            async {
                match anthropic {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match claude {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match openai {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match openrouter {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match copilot {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match antigravity {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match gemini {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match cursor {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match bedrock {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
        );

        let mut errors = Vec::new();
        let mut optional_errors = Vec::new();
        for (provider_name, result) in [
            ("anthropic", anthropic_result),
            ("claude", claude_result),
            ("openai", openai_result),
            ("openrouter", openrouter_result),
            ("copilot", copilot_result),
            ("antigravity", antigravity_result),
            ("gemini", gemini_result),
            ("cursor", cursor_result),
            ("bedrock", bedrock_result),
        ] {
            if let Err(err) = result {
                if matches!(provider_name, "bedrock") {
                    optional_errors.push(format!("{provider_name}: {err}"));
                } else {
                    errors.push(format!("{provider_name}: {err}"));
                }
            }
        }

        if !optional_errors.is_empty() {
            crate::logging::warn(&format!(
                "Optional model catalog refresh failed: {}",
                optional_errors.join("; ")
            ));
        }

        if !errors.is_empty() {
            return Err(anyhow!("{}", errors.join("; ")));
        }

        Ok(())
    }

    fn on_auth_changed(&self) {
        self.handle_auth_changed(false);
    }

    fn on_auth_changed_preserve_current_provider(&self) {
        self.handle_auth_changed(true);
    }

    async fn invalidate_credentials(&self) {
        if let Some(anthropic) = self.anthropic_provider() {
            anthropic.invalidate_credentials().await;
        }
        if let Some(openai) = self.openai_provider() {
            openai.invalidate_credentials().await;
        }
    }

    fn handles_tools_internally(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Direct API does NOT handle tools internally - jcode executes them
                if self.anthropic_provider().is_some() {
                    false
                } else {
                    self.claude_provider()
                        .map(|c| c.handles_tools_internally())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::Antigravity => false,
            ActiveProvider::Gemini => false,
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::Bedrock => false, // jcode executes Bedrock tool calls
            ActiveProvider::OpenRouter => false, // jcode executes tools
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if self.use_claude_cli {
                    None
                } else {
                    self.anthropic_provider()
                        .and_then(|provider| provider.reasoning_effort())
                }
            }
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.reasoning_effort()),
            ActiveProvider::Copilot => None,
            ActiveProvider::Antigravity => None,
            ActiveProvider::Gemini => None,
            ActiveProvider::Cursor => None,
            ActiveProvider::Bedrock => None,
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .and_then(|o| o.reasoning_effort()),
        }
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .ok_or_else(|| anyhow::anyhow!("Anthropic provider not available"))?
                .set_reasoning_effort(effort),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_reasoning_effort(effort),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI-compatible provider not available"))?
                .set_reasoning_effort(effort),
            _ => Err(anyhow::anyhow!(
                "Reasoning effort is only supported for OpenAI, Anthropic, and compatible reasoning models"
            )),
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .map(|provider| provider.available_efforts())
                .unwrap_or_default(),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.available_efforts())
                .unwrap_or_default(),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.available_efforts())
                .unwrap_or_default(),
            ActiveProvider::Copilot => vec![],
            ActiveProvider::Antigravity => vec![],
            ActiveProvider::Gemini => vec![],
            ActiveProvider::Cursor => vec![],
            _ => vec![],
        }
    }

    fn service_tier(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => {
                self.anthropic_provider().and_then(|a| a.service_tier())
            }
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.service_tier()),
            _ => None,
        }
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .ok_or_else(|| anyhow::anyhow!("Anthropic provider not available"))?
                .set_service_tier(service_tier),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_service_tier(service_tier),
            _ => Err(anyhow::anyhow!(
                "Service tier switching is only supported for OpenAI models and Claude Opus 4.8"
            )),
        }
    }

    fn available_service_tiers(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .map(|a| a.available_service_tiers())
                .unwrap_or_default(),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.available_service_tiers())
                .unwrap_or_default(),
            _ => vec![],
        }
    }

    fn native_compaction_mode(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
                .and_then(|o| o.native_compaction_mode()),
            _ => None,
        }
    }

    fn native_compaction_threshold_tokens(&self) -> Option<usize> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
                .and_then(|o| o.native_compaction_threshold_tokens()),
            _ => None,
        }
    }

    fn transport(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.transport()),
            _ => None,
        }
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_transport(transport),
            _ => Err(anyhow::anyhow!(
                "Transport switching is only supported for OpenAI models"
            )),
        }
    }

    fn available_transports(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.available_transports())
                .unwrap_or_default(),
            ActiveProvider::Gemini => vec![],
            ActiveProvider::Cursor => vec![],
            _ => vec![],
        }
    }

    fn supports_compaction(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if self.anthropic_provider().is_some() {
                    true
                } else {
                    self.claude_provider()
                        .map(|c| c.supports_compaction())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
        }
    }

    fn uses_jcode_compaction(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if self.anthropic_provider().is_some() {
                    true
                } else {
                    self.claude_provider()
                        .map(|c| c.uses_jcode_compaction())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Bedrock => false,
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
        }
    }

    async fn native_compact(
        &self,
        messages: &[Message],
        existing_summary_text: Option<&str>,
        existing_openai_encrypted_content: Option<&str>,
    ) -> Result<NativeCompactionResult> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else if let Some(claude) = self.claude_provider() {
                    claude
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Claude provider unavailable"))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    openai
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenAI provider unavailable"))
                }
            }
            ActiveProvider::Copilot => {
                let provider = self.copilot_provider();
                if let Some(copilot) = provider {
                    copilot
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Copilot provider unavailable"))
                }
            }
            ActiveProvider::Antigravity => Err(anyhow::anyhow!(
                "Antigravity does not support native compaction"
            )),
            ActiveProvider::Gemini => {
                let provider = self.gemini_provider();
                if let Some(gemini) = provider {
                    gemini
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Gemini provider unavailable"))
                }
            }
            ActiveProvider::Cursor => {
                let provider = self.cursor_provider();
                if let Some(cursor) = provider {
                    cursor
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Cursor provider unavailable"))
                }
            }
            ActiveProvider::Bedrock => Err(anyhow::anyhow!(
                "AWS Bedrock does not support native compaction"
            )),
            ActiveProvider::OpenRouter => {
                let provider = self.openrouter_provider();
                if let Some(openrouter) = provider {
                    openrouter
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenRouter provider unavailable"))
                }
            }
        }
    }

    fn set_premium_mode(&self, mode: PremiumMode) {
        if let Some(copilot) = self.copilot_provider() {
            copilot.set_premium_mode(mode);
        }
    }

    fn premium_mode(&self) -> PremiumMode {
        if let Some(copilot) = self.copilot_provider() {
            copilot.get_premium_mode()
        } else {
            PremiumMode::Normal
        }
    }

    fn drain_startup_notices(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn context_window(&self) -> usize {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.context_window()
                } else if let Some(claude) = self.claude_provider() {
                    claude.context_window()
                } else {
                    DEFAULT_CONTEXT_LIMIT
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let current_model = self.model();
        let active = self.active_provider();

        let claude = if matches!(active, ActiveProvider::Claude) && self.claude_provider().is_some()
        {
            Some(Arc::new(claude::ClaudeProvider::new()))
        } else {
            None
        };
        let anthropic = if self.anthropic_provider().is_some() {
            Some(Arc::new(anthropic::AnthropicProvider::new()))
        } else {
            None
        };
        let openai = if self.openai_provider().is_some() {
            auth::codex::load_credentials()
                .ok()
                .map(openai::OpenAIProvider::new)
                .map(Arc::new)
        } else {
            None
        };
        let copilot_api = self
            .copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let antigravity_provider = self
            .antigravity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let gemini_provider = self
            .gemini
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let cursor_provider = if self
            .cursor
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            Some(Arc::new(cursor::CursorCliProvider::new()))
        } else {
            None
        };
        let bedrock_provider = if self.bedrock_provider().is_some() {
            Some(Arc::new(bedrock::BedrockProvider::new()))
        } else {
            None
        };
        let openrouter = if self
            .openrouter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            openrouter::OpenRouterProvider::new().ok().map(Arc::new)
        } else {
            None
        };

        let provider = Self {
            claude: RwLock::new(claude),
            anthropic: RwLock::new(anthropic),
            openai: RwLock::new(openai),
            copilot_api: RwLock::new(copilot_api),
            antigravity: RwLock::new(antigravity_provider),
            gemini: RwLock::new(gemini_provider),
            cursor: RwLock::new(cursor_provider),
            bedrock: RwLock::new(bedrock_provider),
            openrouter: RwLock::new(openrouter),
            openai_compatible_profiles: RwLock::new(HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(active),
            use_claude_cli: self.use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: self.forced_provider,
        };

        provider.spawn_anthropic_catalog_refresh_if_needed();
        provider.spawn_openai_catalog_refresh_if_needed();
        let switch_request = self.fork_model_switch_request(active, &current_model);
        let _ = provider.set_model(&switch_request);
        Arc::new(provider)
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        match self.active_provider() {
            // Direct API doesn't use native result sender
            ActiveProvider::Claude => {
                if self.anthropic_provider().is_some() {
                    None
                } else {
                    self.claude_provider()
                        .and_then(|c| c.native_result_sender())
                }
            }
            ActiveProvider::OpenAI => None,
            ActiveProvider::Copilot => None,
            ActiveProvider::Antigravity => None,
            ActiveProvider::Gemini => None,
            ActiveProvider::Cursor => None,
            ActiveProvider::Bedrock => None,
            ActiveProvider::OpenRouter => None,
        }
    }

    fn switch_active_provider_to(&self, provider: &str) -> Result<()> {
        let target = Self::parse_provider_hint(provider)
            .ok_or_else(|| anyhow::anyhow!("Unknown provider `{}`", provider))?;
        if !self.provider_is_configured(target) {
            anyhow::bail!(
                "Provider `{}` is not configured in this session",
                Self::provider_key(target)
            );
        }
        self.set_active_provider(target);
        self.auto_select_multi_account_for_provider(target);
        Ok(())
    }
}

/// Get the prompt cache TTL in seconds for a given provider name.
/// Returns None if the provider doesn't support prompt caching or TTL is unknown.
pub fn cache_ttl_for_provider(provider: &str) -> Option<u64> {
    cache_ttl_for_provider_model(provider, None)
}

/// Get the prompt cache TTL in seconds for a given provider/model pair.
///
/// This is provider cache-retention policy: it depends only on provider
/// families (anthropic/openai/...) and their model capabilities, so it lives
/// in `provider` rather than the UI layer.
pub fn cache_ttl_for_provider_model(provider: &str, model: Option<&str>) -> Option<u64> {
    match provider.to_lowercase().as_str() {
        "anthropic" | "claude" => Some(if anthropic::is_cache_ttl_1h() {
            60 * 60
        } else {
            300
        }),
        "openai" => {
            if model
                .map(openai::OpenAIProvider::supports_extended_prompt_cache_retention)
                .unwrap_or(false)
            {
                Some(24 * 60 * 60)
            } else {
                Some(300)
            }
        }
        "openrouter" => Some(300),
        "jcode subscription" => Some(300),
        "gemini" => Some(300),
        "copilot" => None,
        "cursor" => None,
        "antigravity" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests;
