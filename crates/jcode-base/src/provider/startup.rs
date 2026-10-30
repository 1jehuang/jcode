use super::*;

/// Shared fresh-session route choice; see `select_fresh_session_pool_route`.
#[derive(Clone)]
struct FreshRouteMemo {
    key: (String, u64, u64, crate::auth::account_pool::AccountPool),
    at: std::time::Instant,
    spec: Option<String>,
}

impl MultiProvider {
    pub(super) fn spawn_post_auth_model_refresh(
        &self,
        provider: Arc<dyn Provider>,
        provider_label: &'static str,
    ) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            crate::logging::auth_event(
                "post_auth_model_refresh_skipped",
                provider_label,
                &[("reason", "no_tokio_runtime")],
            );
            return;
        };

        self.post_auth_refreshes_pending
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let pending = Arc::clone(&self.post_auth_refreshes_pending);

        handle.spawn(async move {
            struct PendingGuard(Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for PendingGuard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            let _pending_guard = PendingGuard(pending);
            let refresh_started = std::time::Instant::now();
            crate::logging::auth_event("post_auth_model_refresh_started", provider_label, &[]);
            provider.invalidate_credentials().await;
            match provider.prefetch_models().await {
                Ok(()) => {
                    let duration_ms = refresh_started.elapsed().as_millis().to_string();
                    crate::logging::auth_event(
                        "post_auth_model_refresh_completed",
                        provider_label,
                        &[("duration_ms", duration_ms.as_str())],
                    );
                    crate::bus::Bus::global().publish_models_updated();
                }
                Err(err) => {
                    let reason = err.to_string();
                    let duration_ms = refresh_started.elapsed().as_millis().to_string();
                    crate::logging::auth_event(
                        "post_auth_model_refresh_failed",
                        provider_label,
                        &[
                            ("reason", reason.as_str()),
                            ("duration_ms", duration_ms.as_str()),
                        ],
                    );
                    crate::logging::info(&format!(
                        "Failed to refresh {} models after auth change: {}",
                        provider_label, err
                    ));
                }
            }
        });
    }

    pub(super) fn new_with_auth_status(auth_status: auth::AuthStatus) -> Self {
        let provider_init_start = std::time::Instant::now();
        let cfg = crate::config::config();
        let provider_state = ProviderState::from_parts(cfg, &auth_status);
        let initial_provider = Self::initial_provider_from_env();
        let mut default_named_provider_profile: Option<String> = None;
        if initial_provider.is_none()
            && std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_none()
            && std::env::var_os("JCODE_NAMED_PROVIDER_PROFILE").is_none()
            && let Some(pref) = provider_state.default_provider_key()
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

        let creds_start = std::time::Instant::now();
        let has_claude_creds =
            auth::claude::load_credentials().is_ok() || anthropic::has_anthropic_api_key();
        let has_openai_creds = auth::codex::load_credentials().is_ok();
        let has_copilot_api = provider_state.auth_status().copilot_has_api_token;
        let has_antigravity_creds = auth::antigravity::load_tokens().is_ok();
        let has_gemini_creds = auth::gemini::load_tokens().is_ok() || auth::gemini::has_api_key();
        let has_cursor_creds = provider_state
            .auth_status()
            .assessment_for_provider(crate::provider_catalog::CURSOR_LOGIN_PROVIDER)
            .is_available();
        let has_bedrock_creds = bedrock::BedrockProvider::has_credentials();
        let has_openrouter_creds = openrouter::has_credentials();
        let creds_ms = creds_start.elapsed().as_millis();
        let runtimes_start = std::time::Instant::now();

        if std::env::var_os("JCODE_USE_CLAUDE_CLI").is_some() {
            crate::logging::warn(
                "JCODE_USE_CLAUDE_CLI has been removed and is ignored. Claude uses the direct Anthropic transport.",
            );
        }

        let anthropic = if has_claude_creds {
            let provider =
                external::instantiate_expected_external_provider(external::ANTHROPIC_RUNTIME);
            let active_profile_is_anthropic = std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .and_then(|name| cfg.providers.get(&name))
                .is_some_and(|profile| {
                    matches!(
                        profile.provider_type,
                        crate::config::NamedProviderType::AnthropicCompatible
                    )
                });
            if active_profile_is_anthropic {
                crate::provider_catalog::clear_anthropic_profile_env();
            }
            provider
        } else {
            None
        };

        let openai = if has_openai_creds {
            external::instantiate_expected_external_provider(external::OPENAI_RUNTIME)
        } else {
            None
        };

        let copilot_api = if has_copilot_api {
            // The composition-root factory handles construction, tier-detection
            // scheduling (eager vs non-interactive deferral), and init-done
            // signaling; None means credentials were missing or invalid.
            let copilot_init_start = std::time::Instant::now();
            let provider =
                external::instantiate_expected_external_provider(external::COPILOT_RUNTIME);
            match &provider {
                Some(_) => crate::logging::info(&format!(
                    "Copilot API provider initialized (direct API) in {}ms",
                    copilot_init_start.elapsed().as_millis()
                )),
                None => crate::logging::info("Failed to initialize Copilot API (no credentials)"),
            }
            provider
        } else {
            None
        };

        let antigravity_provider = if has_antigravity_creds {
            external::instantiate_expected_external_provider(external::ANTIGRAVITY_RUNTIME)
        } else {
            None
        };

        let gemini_provider = if has_gemini_creds {
            external::instantiate_expected_external_provider(external::GEMINI_RUNTIME)
        } else {
            None
        };

        let cursor_provider = if has_cursor_creds {
            external::instantiate_expected_external_provider(external::CURSOR_RUNTIME)
        } else {
            None
        };

        let bedrock_provider = if has_bedrock_creds {
            Some(Arc::new(new_bedrock_provider()))
        } else {
            None
        };

        let active_named_profile_is_anthropic = std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
            .ok()
            .or_else(|| default_named_provider_profile.clone())
            .and_then(|name| cfg.providers.get(&name))
            .is_some_and(|profile| {
                matches!(
                    profile.provider_type,
                    crate::config::NamedProviderType::AnthropicCompatible
                )
            });
        let openrouter = if has_openrouter_creds && !active_named_profile_is_anthropic {
            let named_profile = std::env::var("JCODE_NAMED_PROVIDER_PROFILE")
                .ok()
                .or_else(|| default_named_provider_profile.clone());
            let spec = named_profile
                .as_deref()
                .and_then(|profile_name| {
                    cfg.providers.get(profile_name).map(|profile| {
                        external::OpenRouterRuntimeSpec::NamedProfile {
                            name: profile_name.to_string(),
                            config: profile.clone(),
                        }
                    })
                })
                .unwrap_or(external::OpenRouterRuntimeSpec::Default);
            match external::instantiate_openrouter_runtime(spec) {
                Ok(p) => Some(p),
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
            claude: anthropic.is_some(),
            copilot: copilot_api.is_some(),
            antigravity: antigravity_provider.is_some(),
            gemini: gemini_provider.is_some(),
            cursor: cursor_provider.is_some(),
            bedrock: bedrock_provider.is_some(),
            openrouter: openrouter.is_some(),
            copilot_premium_zero,
        };
        let mut active = Self::auto_default_provider(availability);

        if copilot_premium_zero && matches!(active, ActiveProvider::Copilot) {
            crate::logging::info(
                "Copilot premium mode is Zero (free requests) - defaulting to Copilot provider",
            );
        }

        if let Some(initial) = initial_provider {
            active = initial;
            let is_configured = availability.is_configured(initial);
            if is_configured {
                let display = if matches!(initial, ActiveProvider::OpenRouter) {
                    crate::provider_catalog::active_openai_compatible_display_name()
                        .unwrap_or_else(|| Self::provider_key(initial).to_string())
                } else {
                    Self::provider_key(initial).to_string()
                };
                crate::logging::info(&format!(
                    "Using initial provider '{}' from CLI/environment",
                    display
                ));
            } else {
                crate::logging::warn(&format!(
                    "Initial provider '{}' is not configured; requests will fail until credentials are available or another model is selected",
                    Self::provider_key(initial)
                ));
            }
        } else if let Some(pref) = provider_state.default_provider_key() {
            if let Some(selection) = provider_state.default_provider_selection() {
                let preferred = selection.active_provider();
                let is_configured = provider_state
                    .preferred_provider_is_configured(availability)
                    .unwrap_or(false);
                if is_configured {
                    active = preferred;
                    crate::logging::info(&format!(
                        "Using preferred provider '{}' from config via {}",
                        pref,
                        provider_state
                            .preferred_provider_display_label()
                            .unwrap_or_else(|| selection.display_label())
                    ));
                } else {
                    crate::logging::warn(&format!(
                        "Preferred provider '{}' is not configured, using auto-detected default",
                        pref
                    ));
                }
            } else {
                crate::logging::warn(&format!(
                    "Unknown default_provider '{}' in config (expected: claude|openai|copilot|antigravity|gemini|cursor|bedrock|openrouter or an OpenAI-compatible profile such as deepseek|comtegra|zai|openai-compatible)",
                    pref
                ));
            }
        }

        let result = Self {
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
            session_id: RwLock::new(None),
            active: RwLock::new(active),
            startup_notices: RwLock::new(Vec::new()),
            initial_provider,
            catalog_scope: next_catalog_scope(),
            routes_memo: Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        // An explicit CLI/environment provider selection owns startup routing.
        // Applying the configured default model here can reactivate its configured
        // provider/profile before the caller pins a dual-auth credential mode.
        if result.initial_provider.is_none()
            && let Some(model) = provider_state.default_model()
        {
            if let Err(e) =
                result.set_config_default_model(model, provider_state.default_provider_key())
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
        let runtimes_ms = runtimes_start.elapsed().as_millis();
        let route_start = std::time::Instant::now();
        result.spawn_openai_catalog_refresh_if_needed();
        result.select_fresh_session_pool_route();
        let route_ms = route_start.elapsed().as_millis();
        crate::logging::info(&format!(
            "[TIMING] provider_init parts: creds={creds_ms}ms, runtimes={runtimes_ms}ms, route={route_ms}ms"
        ));
        crate::logging::info(&format!(
            "[TIMING] provider_init: anthropic={}, openai={}, copilot={}, antigravity={}, gemini={}, cursor={}, bedrock={}, openrouter={}, total={}ms",
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
                .bedrock
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

    /// Fresh sessions choose the first ready account in priority order. A saved
    /// default model remains a preference within the selected account, not a
    /// sticky billing identity. An explicit initial provider owns routing instead.
    /// This is deliberately a local
    /// model/account binding: never promote the fallback or change active logins.
    fn select_fresh_session_pool_route(&self) {
        if self.initial_provider.is_some() {
            return;
        }
        // A reconnect burst forks one provider per client. Each choice reads every
        // saved credential and pool approval across the whole route catalog, so 40
        // parallel forks took seconds. Identical forks pick the same route, so share
        // one recent decision. The key covers the inputs that matter (pre-selection
        // provider state, auth and catalog generations). A short TTL bounds how stale
        // quota and cooldown state can get.
        const FRESH_ROUTE_MEMO_TTL: std::time::Duration = std::time::Duration::from_secs(3);
        static FRESH_ROUTE_MEMO: Mutex<Option<FreshRouteMemo>> = Mutex::new(None);
        static FRESH_ROUTE_BUILD_LOCK: Mutex<()> = Mutex::new(());
        let key = (
            self.routes_memo_key_unscoped(),
            pricing::auth_pricing_generation(),
            catalog_generation(),
            crate::auth::account_pool::AccountPool::load(),
        );
        let cached = || {
            FRESH_ROUTE_MEMO
                .lock()
                .ok()
                .and_then(|memo| memo.clone())
                .filter(|memo| memo.key == key && memo.at.elapsed() < FRESH_ROUTE_MEMO_TTL)
        };
        let apply = |memo: FreshRouteMemo| match memo.spec {
            None => true,
            Some(spec) => self.set_model(&spec).is_ok(),
        };
        if let Some(memo) = cached()
            && apply(memo)
        {
            return;
        }
        let _guard = FRESH_ROUTE_BUILD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(memo) = cached()
            && apply(memo)
        {
            return;
        }
        let spec = self.compute_fresh_session_pool_route(key.3.clone());
        if let Ok(mut memo) = FRESH_ROUTE_MEMO.lock() {
            *memo = Some(FreshRouteMemo {
                key,
                at: std::time::Instant::now(),
                spec,
            });
        }
    }

    /// Bind the first ready pool route and return its spec (None if nothing bound).
    fn compute_fresh_session_pool_route(
        &self,
        pool: crate::auth::account_pool::AccountPool,
    ) -> Option<String> {
        let routes: Vec<_> = self
            .model_routes()
            .into_iter()
            .filter_map(|route| {
                let spec = jcode_provider_core::RouteSelection::from_model_route(&route)
                    .routed_model_spec();
                let approved = route.available && Self::is_failover_route_pool_approved(&spec);
                let (method, _) = spec.split_once(':')?;
                let mut exhausted = false;
                let key = if let Some((method, label)) = method.split_once('@') {
                    let provider = match method {
                        "claude-oauth" => "claude",
                        "openai-oauth" => "openai",
                        _ => return None,
                    };
                    let kind = if provider == "claude" {
                        crate::usage::MultiAccountProviderKind::Anthropic
                    } else {
                        crate::usage::MultiAccountProviderKind::OpenAI
                    };
                    exhausted = models::provider_unavailability_detail_for_account_label(
                        provider,
                        Some(label),
                    )
                    .is_some()
                        || crate::usage::account_known_exhausted(kind, label);
                    if !pool.is_member(
                        &crate::auth::account_pool::account_key(provider, Some(label)),
                        true,
                    ) || !models::is_agent_model_for_failover(&route.model, provider)
                    {
                        return None;
                    }
                    crate::auth::account_pool::account_key(provider, Some(label))
                } else {
                    match method {
                        "claude-api" | "anthropic-api-key" => "anthropic-api".to_owned(),
                        "openai-api-key" => "openai-api".to_owned(),
                        other => other.to_owned(),
                    }
                };
                if !approved && !exhausted {
                    return None;
                }
                Some((key, route.model, spec, approved && !exhausted))
            })
            .collect();
        let mut keys = Vec::new();
        for (key, _, _, _) in &routes {
            if !keys.iter().any(|(existing, _)| existing == key) {
                keys.push((key.clone(), true));
            }
        }
        let cfg = crate::config::config();
        let configured_model = cfg.provider.default_model.as_deref();
        let configured_model = configured_model.map(|model| {
            explicit_model_provider_prefix(model)
                .map(|(_, _, bare)| bare)
                .or_else(|| {
                    model.split_once(':').and_then(|(method, bare)| {
                        jcode_provider_core::oauth_account_label(method).map(|_| bare)
                    })
                })
                .unwrap_or(model)
        });
        // Only when every ready route fails do we bind a known exhausted pool
        // login. The request router can then explain the quota failure instead
        // of silently falling through to an unrelated saved manual API key.
        for ready in [true, false] {
            for key in pool.partition(&keys).0 {
                // Exclusion is authoritative even if a malformed saved pool also
                // lists the same billing identity among its members.
                if pool.excluded.iter().any(|excluded| excluded == key) {
                    continue;
                }
                let mut candidates: Vec<_> = routes
                    .iter()
                    .filter(|route| route.0 == key && route.3 == ready)
                    .collect();
                candidates.sort_by_key(|(_, model, _, _)| {
                    (
                        configured_model != Some(model.as_str()),
                        jcode_provider_core::model_generation_rank(model),
                    )
                });
                for (_, _, spec, _) in candidates {
                    if self.set_model(spec).is_ok() {
                        return Some(spec.clone());
                    }
                }
            }
        }
        None
    }

    pub(super) fn spawn_openai_catalog_refresh_if_needed(&self) {
        let Some(provider) = self.openai_provider() else {
            return;
        };
        if !begin_openai_model_catalog_refresh() {
            return;
        }

        tokio::spawn(async move {
            if let Err(err) = provider.prefetch_models().await {
                crate::logging::info(&format!(
                    "Failed to refresh OpenAI model catalog from provider bootstrap: {}",
                    err
                ));
            }
            finish_openai_model_catalog_refresh();
        });
    }

    pub(super) fn spawn_anthropic_catalog_refresh_if_needed(&self) {
        let api_stale = anthropic::load_anthropic_api_key().is_ok()
            && should_refresh_anthropic_model_catalog_for_scope(
                &anthropic_catalog_scope_for_route(false),
            );
        let oauth_stale = auth::claude::load_credentials().is_ok()
            && should_refresh_anthropic_model_catalog_for_scope(
                &anthropic_catalog_scope_for_route(true),
            );
        if !api_stale && !oauth_stale {
            return;
        }
        let Some(provider) = self.anthropic_provider() else {
            return;
        };
        // The direct runtime multiplexes API key and OAuth. Its prefetch owns
        // independent scoped refresh guards, including when invoked elsewhere.
        tokio::spawn(async move {
            if let Err(err) = provider.prefetch_models().await {
                crate::logging::info(&format!(
                    "Failed to refresh Anthropic model catalogs: {err}"
                ));
            }
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
        if provider.initial_provider.is_none()
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
        if provider.initial_provider.is_none()
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
