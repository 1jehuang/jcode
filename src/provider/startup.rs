use super::*;

impl MultiProvider {
    pub(super) fn spawn_post_auth_model_refresh(
        provider: Arc<dyn Provider>,
        provider_label: &'static str,
    ) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        handle.spawn(async move {
            provider.invalidate_credentials().await;
            match provider.prefetch_models().await {
                Ok(()) => {
                    crate::bus::Bus::global().publish_models_updated();
                }
                Err(err) => {
                    crate::logging::info(&format!(
                        "Failed to refresh {} models after auth change: {}",
                        provider_label, err
                    ));
                }
            }
        });
    }

    pub(super) async fn invalidate_provider_credentials_for_account_switch(
        &self,
        provider: ActiveProvider,
    ) {
        match provider {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.invalidate_credentials().await;
                }
                if let Some(claude) = self.claude_provider() {
                    claude.invalidate_credentials().await;
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    openai.invalidate_credentials().await;
                }
            }
            _ => {}
        }
    }

    pub(super) fn new_with_auth_status(auth_status: auth::AuthStatus) -> Self {
        let provider_init_start = std::time::Instant::now();
        let cfg = crate::config::config();
        let mut default_named_provider_profile: Option<String> = None;
        if std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_none()
            && std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none()
            && let Some(pref) = cfg.provider.default_provider.as_deref()
        {
            if let Some(profile) =
                crate::provider_catalog::resolve_openai_compatible_profile_selection(pref)
            {
                crate::provider_catalog::apply_openai_compatible_profile_env(Some(profile));
            } else if cfg.providers.contains_key(pref) {
                match crate::provider_catalog::apply_named_provider_profile_env_from_config(
                    pref, cfg,
                ) {
                    Ok(profile_name) => {
                        crate::env::set_var("JCODE_PROVIDER_PROFILE_NAME", &profile_name);
                        crate::env::set_var("JCODE_PROVIDER_PROFILE_ACTIVE", "1");
                        default_named_provider_profile = Some(profile_name);
                    }
                    Err(err) => crate::logging::warn(&format!(
                        "Failed to apply default provider profile '{}': {}",
                        pref, err
                    )),
                }
            }
        }

        let has_claude_creds = auth::claude::load_credentials().is_ok();
        let has_openai_creds = auth::codex::load_credentials().is_ok();
        let has_copilot_api = auth_status.copilot_has_api_token;
        let has_antigravity_creds = auth::antigravity::load_tokens().is_ok();
        let has_gemini_creds = auth::gemini::load_tokens().is_ok();
        let has_cursor_creds = matches!(auth_status.cursor, auth::AuthState::Available);
        let has_openrouter_creds = openrouter::OpenRouterProvider::has_credentials();

        let use_claude_cli = std::env::var("JCODE_USE_CLAUDE_CLI")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if use_claude_cli {
            crate::logging::warn(
                "JCODE_USE_CLAUDE_CLI is deprecated and will be removed. Direct Anthropic API transport is the default.",
            );
        }

        let claude = if has_claude_creds && use_claude_cli {
            crate::logging::info(
                "Using deprecated Claude CLI provider (forced by JCODE_USE_CLAUDE_CLI=1)",
            );
            Some(Arc::new(claude::ClaudeProvider::new()))
        } else {
            None
        };

        let anthropic = if has_claude_creds && !use_claude_cli {
            Some(Arc::new(anthropic::AnthropicProvider::new()))
        } else {
            None
        };

        let openai = if has_openai_creds {
            auth::codex::load_credentials()
                .ok()
                .map(openai::OpenAIProvider::new)
                .map(Arc::new)
        } else {
            None
        };

        let copilot_api = if has_copilot_api {
            let copilot_init_start = std::time::Instant::now();
            match copilot::CopilotApiProvider::new() {
                Ok(p) => {
                    crate::logging::info(&format!(
                        "Copilot API provider initialized (direct API) in {}ms",
                        copilot_init_start.elapsed().as_millis()
                    ));
                    let provider = Arc::new(p);
                    if should_eager_detect_copilot_tier() {
                        let p_clone = provider.clone();
                        tokio::spawn(async move {
                            p_clone.detect_tier_and_set_default().await;
                        });
                    } else {
                        crate::logging::info(
                            "Deferring Copilot tier detection during non-interactive startup",
                        );
                        provider.complete_init_without_tier_detection();
                    }
                    Some(provider)
                }
                Err(e) => {
                    crate::logging::info(&format!("Failed to initialize Copilot API: {}", e));
                    None
                }
            }
        } else {
            None
        };

        let antigravity_provider = if has_antigravity_creds {
            Some(Arc::new(antigravity::AntigravityProvider::new()))
        } else {
            None
        };

        let gemini_provider = if has_gemini_creds {
            Some(Arc::new(gemini::GeminiProvider::new()))
        } else {
            None
        };

        let cursor_provider = if has_cursor_creds {
            Some(Arc::new(cursor::CursorCliProvider::new()))
        } else {
            None
        };

        let openrouter = if has_openrouter_creds {
            let named_profile = std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .or_else(|| default_named_provider_profile.clone());
            let provider_result = if let Some(profile_name) = named_profile.as_deref() {
                if let Some(profile) = cfg.providers.get(profile_name) {
                    openrouter::OpenRouterProvider::new_named_openai_compatible(
                        profile_name,
                        profile,
                    )
                } else {
                    openrouter::OpenRouterProvider::new()
                }
            } else {
                openrouter::OpenRouterProvider::new()
            };
            match provider_result {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    crate::logging::info(&format!("Failed to initialize OpenRouter: {}", e));
                    None
                }
            }
        } else {
            None
        };

        let copilot_premium_zero = matches!(
            std::env::var("JCODE_COPILOT_PREMIUM").ok().as_deref(),
            Some("0")
        );
        let availability = ProviderAvailability {
            openai: openai.is_some(),
            claude: claude.is_some() || anthropic.is_some(),
            copilot: copilot_api.is_some(),
            antigravity: antigravity_provider.is_some(),
            gemini: gemini_provider.is_some(),
            cursor: cursor_provider.is_some(),
            openrouter: openrouter.is_some(),
            copilot_premium_zero,
        };
        let mut active = Self::auto_default_provider(availability);

        if copilot_premium_zero && matches!(active, ActiveProvider::Copilot) {
            crate::logging::info(
                "Copilot premium mode is Zero (free requests) - defaulting to Copilot provider",
            );
        }

        let forced_provider = Self::forced_provider_from_env();
        if let Some(forced) = forced_provider {
            active = forced;
            let is_configured = availability.is_configured(forced);
            if is_configured {
                let display = if matches!(forced, ActiveProvider::OpenRouter) {
                    crate::provider_catalog::active_openai_compatible_display_name()
                        .unwrap_or_else(|| Self::provider_key(forced).to_string())
                } else {
                    Self::provider_key(forced).to_string()
                };
                crate::logging::info(&format!(
                    "Using forced provider '{}' from CLI/environment",
                    display
                ));
            } else {
                crate::logging::warn(&format!(
                    "Forced provider '{}' is not configured; requests will fail until credentials are available",
                    Self::provider_key(forced)
                ));
            }
        } else if let Some(ref pref) = cfg.provider.default_provider {
            if let Some(profile) =
                crate::provider_catalog::resolve_openai_compatible_profile_selection(pref)
            {
                let is_configured = availability.is_configured(ActiveProvider::OpenRouter);
                if is_configured {
                    active = ActiveProvider::OpenRouter;
                    let resolved =
                        crate::provider_catalog::resolve_openai_compatible_profile(profile);
                    crate::logging::info(&format!(
                        "Using preferred provider '{}' from config via OpenAI-compatible profile {}",
                        pref, resolved.display_name
                    ));
                } else {
                    crate::logging::warn(&format!(
                        "Preferred provider '{}' is not configured, using auto-detected default",
                        pref
                    ));
                }
            } else if cfg.providers.contains_key(pref) {
                let is_configured = availability.is_configured(ActiveProvider::OpenRouter);
                if is_configured {
                    active = ActiveProvider::OpenRouter;
                    crate::logging::info(&format!(
                        "Using preferred provider profile '{}' from config",
                        pref
                    ));
                } else {
                    crate::logging::warn(&format!(
                        "Preferred provider profile '{}' is not configured, using auto-detected default",
                        pref
                    ));
                }
            } else if let Some(pref_provider) = Self::parse_provider_hint(pref) {
                let is_configured = availability.is_configured(pref_provider);
                if is_configured {
                    active = pref_provider;
                    crate::logging::info(&format!(
                        "Using preferred provider '{}' from config",
                        pref
                    ));
                } else {
                    crate::logging::warn(&format!(
                        "Preferred provider '{}' is not configured, using auto-detected default",
                        pref
                    ));
                }
            } else {
                crate::logging::warn(&format!(
                    "Unknown default_provider '{}' in config (expected: claude|openai|copilot|antigravity|gemini|cursor|openrouter or an OpenAI-compatible profile such as deepseek|comtegra|zai|openai-compatible)",
                    pref
                ));
            }
        }

        let result = Self {
            claude: RwLock::new(claude),
            anthropic: RwLock::new(anthropic),
            openai: RwLock::new(openai),
            copilot_api: RwLock::new(copilot_api),
            antigravity: RwLock::new(antigravity_provider),
            gemini: RwLock::new(gemini_provider),
            cursor: RwLock::new(cursor_provider),
            openrouter: RwLock::new(openrouter),
            active: RwLock::new(active),
            use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider,
        };

        if let Some(ref model) = cfg.provider.default_model {
            if let Err(e) =
                result.set_config_default_model(model, cfg.provider.default_provider.as_deref())
            {
                crate::logging::warn(&format!(
                    "Failed to apply default_model '{}' from config: {}",
                    model, e
                ));
            } else {
                crate::logging::info(&format!("Applied default model '{}' from config", model));
            }
        }

        result.spawn_anthropic_catalog_refresh_if_needed();
        result.spawn_openai_catalog_refresh_if_needed();
        result.auto_select_active_multi_account();
        crate::logging::info(&format!(
            "[TIMING] provider_init: claude={}, anthropic={}, openai={}, copilot={}, antigravity={}, gemini={}, cursor={}, openrouter={}, total={}ms",
            result
                .claude
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .anthropic
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .openai
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .copilot_api
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .antigravity
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .gemini
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .cursor
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            result
                .openrouter
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            provider_init_start.elapsed().as_millis()
        ));
        result
    }

    pub(super) fn spawn_openai_catalog_refresh_if_needed(&self) {
        if self.openai_provider().is_none() {
            return;
        }
        if !begin_openai_model_catalog_refresh() {
            return;
        }

        let creds = auth::codex::load_credentials();
        let token = creds
            .as_ref()
            .ok()
            .map(|c| c.access_token.clone())
            .unwrap_or_default();
        refresh_openai_model_catalog_in_background(token, "multi-provider");
    }

    pub(super) fn spawn_anthropic_catalog_refresh_if_needed(&self) {
        let provider: Arc<dyn Provider> = if let Some(anthropic) = self.anthropic_provider() {
            anthropic
        } else if let Some(claude) = self.claude_provider() {
            claude
        } else {
            return;
        };

        let Some(scope) = begin_anthropic_model_catalog_refresh() else {
            return;
        };

        tokio::spawn(async move {
            if let Err(err) = provider.prefetch_models().await {
                crate::logging::info(&format!(
                    "Failed to refresh Anthropic model catalog from provider bootstrap: {}",
                    err
                ));
            }
            finish_anthropic_model_catalog_refresh_for_scope(&scope);
        });
    }

    /// Create a new MultiProvider, detecting available credentials
    pub fn new() -> Self {
        Self::new_with_auth_status(auth::AuthStatus::check())
    }

    /// Create a startup-optimized MultiProvider that avoids expensive auth probes.
    pub fn new_fast() -> Self {
        Self::new_with_auth_status(auth::AuthStatus::check_fast())
    }

    pub fn from_auth_status(auth_status: auth::AuthStatus) -> Self {
        Self::new_with_auth_status(auth_status)
    }

    /// Create with explicit initial provider preference
    pub fn with_preference(prefer_openai: bool) -> Self {
        let provider = Self::new();
        if provider.forced_provider.is_none()
            && prefer_openai
            && provider.openai_provider().is_some()
        {
            *provider
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::OpenAI;
        }
        provider
    }

    pub fn with_preference_fast(prefer_openai: bool) -> Self {
        let provider = Self::new_fast();
        if provider.forced_provider.is_none()
            && prefer_openai
            && provider.openai_provider().is_some()
        {
            *provider
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::OpenAI;
        }
        provider
    }

    pub(super) fn active_provider(&self) -> ActiveProvider {
        *self
            .active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn auto_select_active_multi_account(&self) {
        self.auto_select_multi_account_for_provider(self.active_provider());
    }

    /// Backward-compatible wrapper for the Anthropic-specific startup rotation entrypoint.
    pub fn auto_select_anthropic_account(&self) {
        self.auto_select_multi_account_for_provider(ActiveProvider::Claude);
    }

    pub fn auto_select_openai_account(&self) {
        self.auto_select_multi_account_for_provider(ActiveProvider::OpenAI);
    }

    pub(super) fn auto_select_multi_account_for_provider(&self, provider: ActiveProvider) {
        if self.active_provider() != provider {
            return;
        }
        if !self.provider_is_configured(provider) {
            return;
        }
        if provider == ActiveProvider::OpenAI {
            return;
        }

        let Some(probe) = account_usage_probe(provider) else {
            return;
        };
        if !probe.has_multiple_accounts() || !probe.current_exhausted() {
            return;
        }

        let provider_name = probe.provider.display_name();
        if let Some(alternative) = probe.best_available_alternative() {
            crate::logging::info(&format!(
                "{} account '{}' is exhausted, switching to '{}' ({})",
                provider_name,
                probe.current_label,
                alternative.label,
                alternative.summary()
            ));

            match provider {
                ActiveProvider::Claude => {
                    crate::auth::claude::set_active_account_override(Some(
                        alternative.label.clone(),
                    ));
                    clear_all_provider_unavailability_for_account();
                    clear_all_model_unavailability_for_account();
                    if let Some(anthropic) = self.anthropic_provider() {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(anthropic.invalidate_credentials())
                        });
                    }
                }
                ActiveProvider::OpenAI => {
                    crate::auth::codex::set_active_account_override(Some(
                        alternative.label.clone(),
                    ));
                    clear_all_provider_unavailability_for_account();
                    clear_all_model_unavailability_for_account();
                    if let Some(openai) = self.openai_provider() {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(openai.invalidate_credentials())
                        });
                    }
                }
                _ => return,
            }

            let notice = format!(
                "⚡ Auto-switched {} account: **{}** -> **{}** (previous account exhausted)",
                provider_name, probe.current_label, alternative.label
            );
            self.startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(notice);
            return;
        }

        if probe.all_accounts_exhausted() {
            crate::logging::info(&format!("All {} accounts are exhausted", provider_name));
            let notice = format!(
                "⚠ All {} accounts exhausted - will fall back to other providers if available",
                provider_name
            );
            self.startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(notice);
        }
    }

    /// Check if Anthropic OAuth usage is exhausted (both 5hr and 7d at 100%)
    pub(super) fn is_claude_usage_exhausted(&self) -> bool {
        if !self.has_claude_runtime() {
            return false;
        }

        let usage = crate::usage::get_sync();
        usage.five_hour >= 0.99 && usage.seven_day >= 0.99
    }
}
