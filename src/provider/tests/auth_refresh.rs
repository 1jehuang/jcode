#[derive(Clone)]
struct SetModelAuthRefreshMockProvider {
    refreshed: Arc<std::sync::atomic::AtomicBool>,
    attempts: Arc<std::sync::atomic::AtomicUsize>,
    selected_model: Arc<std::sync::Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl Provider for SetModelAuthRefreshMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        unimplemented!("SetModelAuthRefreshMockProvider")
    }

    fn name(&self) -> &str {
        "set-model-auth-refresh-mock"
    }

    fn model(&self) -> String {
        self.selected_model
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "gpt-5.4".to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.refreshed.load(std::sync::atomic::Ordering::SeqCst) {
            anyhow::bail!("Claude credentials not available");
        }
        *self.selected_model.lock().unwrap() = Some(model.to_string());
        Ok(())
    }

    fn on_auth_changed(&self) {
        self.refreshed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[test]
fn test_set_model_with_auth_refresh_reloads_auth_and_retries_once() {
    let provider = SetModelAuthRefreshMockProvider {
        refreshed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        attempts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        selected_model: Arc::new(std::sync::Mutex::new(None)),
    };

    set_model_with_auth_refresh(&provider, "claude-opus-4-6").expect("auth refresh retry succeeds");

    assert!(provider.refreshed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        provider.attempts.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "restore should try once, reload auth, then retry once"
    );
    assert_eq!(provider.model(), "claude-opus-4-6");
}

#[test]
fn test_on_auth_changed_hot_initializes_openai_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenAI),
        };

        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "test-access-token",
            "test-refresh-token",
            None,
            None,
        )
        .expect("save test OpenAI auth");

        provider.on_auth_changed();

        assert!(provider.openai_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "OpenAI" && route.api_method == "openai-oauth" && route.available
        }));
    });
}

#[test]
fn test_on_auth_changed_refreshes_existing_openai_provider_credentials() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "stale-access-token",
            "test-refresh-token",
            None,
            None,
        )
        .expect("save stale test OpenAI auth");

        let existing = Arc::new(openai::OpenAIProvider::new(
            crate::auth::codex::load_credentials().expect("load stale openai credentials"),
        ));

        crate::auth::codex::upsert_account_from_tokens(
            "openai-1",
            "fresh-access-token",
            "test-refresh-token",
            None,
            None,
        )
        .expect("save fresh test OpenAI auth");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(Some(existing.clone())),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenAI),
        };

        provider.on_auth_changed();

        let openai = provider
            .openai_provider()
            .expect("existing openai provider");
        let loaded = runtime.block_on(async { openai.test_access_token().await });
        assert_eq!(loaded, "fresh-access-token");
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_anthropic_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Claude),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Claude),
        };

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "test-access-token".to_string(),
            refresh: "test-refresh-token".to_string(),
            expires: i64::MAX,
            email: None,
            scopes: Vec::new(),
            subscription_type: None,
        })
        .expect("save test Claude auth");

        provider.on_auth_changed();

        assert!(provider.anthropic_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "Anthropic" && route.api_method == "claude-oauth" && route.available
        }));
    });
}

#[test]
fn test_anthropic_model_routes_keep_plain_4_6_available_without_extra_usage() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Claude),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Claude),
        };

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "test-access-token".to_string(),
            refresh: "test-refresh-token".to_string(),
            expires: i64::MAX,
            email: None,
            scopes: Vec::new(),
            subscription_type: None,
        })
        .expect("save test Claude auth");

        provider.on_auth_changed();

        let routes = provider.model_routes();
        let plain_opus = routes
            .iter()
            .find(|route| {
                route.provider == "Anthropic"
                    && route.api_method == "claude-oauth"
                    && route.model == "claude-opus-4-6"
            })
            .expect("plain opus route");
        assert!(plain_opus.available);
        assert!(plain_opus.detail.is_empty());

        let opus_1m = routes
            .iter()
            .find(|route| {
                route.provider == "Anthropic"
                    && route.api_method == "claude-oauth"
                    && route.model == "claude-opus-4-6[1m]"
            })
            .expect("1m opus route");
        assert!(!opus_1m.available);
        assert_eq!(opus_1m.detail, "requires extra usage");
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_openrouter_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        with_env_var("OPENROUTER_API_KEY", "test-openrouter-key", || {
            with_env_var("JCODE_OPENROUTER_MODEL_CATALOG", "0", || {
                let runtime = enter_test_runtime();
                let _enter = runtime.enter();

                let provider = MultiProvider {
                    claude: RwLock::new(None),
                    anthropic: RwLock::new(None),
                    openai: RwLock::new(None),
                    copilot_api: RwLock::new(None),
                    antigravity: RwLock::new(None),
                    gemini: RwLock::new(None),
                    cursor: RwLock::new(None),
                    bedrock: RwLock::new(None),
                    openrouter: RwLock::new(None),
                    active: RwLock::new(ActiveProvider::OpenRouter),
                    use_claude_cli: false,
                    startup_notices: RwLock::new(Vec::new()),
                    forced_provider: Some(ActiveProvider::OpenRouter),
                };

                provider.on_auth_changed();

                assert!(provider.openrouter.read().unwrap().is_some());
                assert!(
                    provider
                        .model_routes()
                        .iter()
                        .any(|route| { route.api_method == "openrouter" && route.available })
                );
            })
        })
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_copilot_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        with_env_var("GITHUB_TOKEN", "gho_test_token", || {
            crate::auth::AuthStatus::invalidate_cache();
            let runtime = enter_test_runtime();
            let _enter = runtime.enter();

            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(None),
                active: RwLock::new(ActiveProvider::Copilot),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::Copilot),
            };

            provider.on_auth_changed();

            assert!(provider.copilot_api.read().unwrap().is_some());
            assert!(provider.model_routes().iter().any(|route| {
                route.provider == "Copilot" && route.api_method == "copilot" && route.available
            }));
        })
    });
}

#[test]
fn test_startup_initializes_antigravity_when_cached_tokens_are_expired() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        crate::auth::antigravity::save_tokens(&crate::auth::antigravity::AntigravityTokens {
            access_token: "expired-access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expires_at: 1,
            email: None,
            project_id: None,
        })
        .expect("save expired antigravity auth");

        let auth_status = crate::auth::AuthStatus::check_fast();
        let provider = MultiProvider::from_auth_status(auth_status);

        assert!(provider.antigravity_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "Antigravity" && route.api_method == "cli" && route.available
        }));
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_antigravity_when_tokens_exist_but_are_expired() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        crate::auth::antigravity::save_tokens(&crate::auth::antigravity::AntigravityTokens {
            access_token: "expired-access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expires_at: 1,
            email: None,
            project_id: None,
        })
        .expect("save expired antigravity auth");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Antigravity),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Antigravity),
        };

        provider.on_auth_changed();

        assert!(provider.antigravity_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "Antigravity" && route.api_method == "cli" && route.available
        }));
    });
}

#[test]
fn test_multi_provider_antigravity_routes_do_not_include_legacy_duplicate_entries() {
    let provider = MultiProvider {
        claude: RwLock::new(None),
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(Some(Arc::new(antigravity::AntigravityProvider::new()))),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        active: RwLock::new(ActiveProvider::Antigravity),
        use_claude_cli: false,
        startup_notices: RwLock::new(Vec::new()),
        forced_provider: Some(ActiveProvider::Antigravity),
    };

    let routes = provider.model_routes();
    assert!(routes.iter().any(|route| {
        route.provider == "Antigravity" && route.api_method == "cli" && route.available
    }));
    assert!(
        !routes
            .iter()
            .any(|route| { route.provider == "Antigravity" && route.api_method == "antigravity" }),
        "legacy duplicate antigravity routes should not be emitted: {:?}",
        routes
    );
}

#[test]
fn test_summarize_model_catalog_refresh_ignores_display_only_age_suffix_changes() {
    let summary = summarize_model_catalog_refresh(
        vec!["anthropic/claude-sonnet-4".to_string()],
        vec!["anthropic/claude-sonnet-4".to_string()],
        vec![ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "Fireworks".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "fast, 5m ago".to_string(),
            cheapness: None,
        }],
        vec![ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "Fireworks".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "fast, 6m ago".to_string(),
            cheapness: None,
        }],
    );

    assert_eq!(
        summary.routes_changed, 0,
        "age-only detail churn should be ignored"
    );
}

#[test]
fn test_summarize_model_catalog_refresh_still_counts_meaningful_detail_changes() {
    let summary = summarize_model_catalog_refresh(
        vec!["anthropic/claude-sonnet-4".to_string()],
        vec!["anthropic/claude-sonnet-4".to_string()],
        vec![ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "Fireworks".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "fast, 5m ago".to_string(),
            cheapness: None,
        }],
        vec![ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "Fireworks".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "cached, 6m ago".to_string(),
            cheapness: None,
        }],
    );

    assert_eq!(summary.routes_changed, 1);
}

#[test]
fn test_on_auth_changed_hot_initializes_gemini_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        crate::auth::gemini::save_tokens(&crate::auth::gemini::GeminiTokens {
            access_token: "test-access-token".to_string(),
            refresh_token: "test-refresh-token".to_string(),
            expires_at: i64::MAX,
            email: None,
        })
        .expect("save test Gemini auth");

        let provider = MultiProvider {
            claude: RwLock::new(None),
            anthropic: RwLock::new(None),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Gemini),
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::Gemini),
        };

        provider.on_auth_changed();

        assert!(provider.gemini_provider().is_some());
        assert!(provider.model_routes().iter().any(|route| {
            route.provider == "Gemini" && route.api_method == "code-assist-oauth" && route.available
        }));
    });
}

#[test]
fn test_on_auth_changed_hot_initializes_cursor_and_marks_routes_available() {
    with_clean_provider_test_env(|| {
        with_env_var("CURSOR_API_KEY", "cursor-test-key", || {
            crate::auth::AuthStatus::invalidate_cache();
            let runtime = enter_test_runtime();
            let _enter = runtime.enter();

            let provider = MultiProvider {
                claude: RwLock::new(None),
                anthropic: RwLock::new(None),
                openai: RwLock::new(None),
                copilot_api: RwLock::new(None),
                antigravity: RwLock::new(None),
                gemini: RwLock::new(None),
                cursor: RwLock::new(None),
                bedrock: RwLock::new(None),
                openrouter: RwLock::new(None),
                active: RwLock::new(ActiveProvider::Cursor),
                use_claude_cli: false,
                startup_notices: RwLock::new(Vec::new()),
                forced_provider: Some(ActiveProvider::Cursor),
            };

            provider.on_auth_changed();

            assert!(provider.cursor.read().unwrap().is_some());
            assert!(provider.model_routes().iter().any(|route| {
                route.provider == "Cursor" && route.api_method == "cursor" && route.available
            }));
        })
    });
}
