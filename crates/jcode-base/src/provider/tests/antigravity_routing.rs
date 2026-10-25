fn test_multi_provider_with_openai_and_antigravity() -> MultiProvider {
    let provider = test_multi_provider_with_cursor();
    *provider
        .cursor
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *provider
        .openai
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(test_openai_runtime());
    *provider
        .antigravity
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(test_antigravity_runtime());
    *provider
        .active
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::OpenAI;
    provider
}

#[test]
fn unprefixed_model_reconciles_newly_available_credentials_before_antigravity_fallback() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai_and_antigravity();

        // 1. Simulate Gemini OAuth tokens newly written on disk while the
        // in-memory `gemini` slot is empty and `antigravity` is populated.
        crate::auth::gemini::save_tokens(&crate::auth::gemini::GeminiTokens {
            access_token: "gemini-access-token".to_string(),
            refresh_token: "gemini-refresh-token".to_string(),
            expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: Some("test@example.com".to_string()),
        })
        .expect("save gemini tokens");

        provider
            .set_model("gemini-3-flash")
            .expect("newly saved Gemini tokens must reconcile before falling back to Antigravity");
        assert_eq!(provider.active_provider(), ActiveProvider::Gemini);
        assert_eq!(provider.model(), "gemini-3-flash");

        // 2. Clear OAuth tokens and the in-memory `gemini` slot, then simulate
        // a Gemini Developer API key newly persisted on disk in default mode
        // (`force_oauth == false`).
        let _ = std::fs::remove_file(crate::auth::gemini::tokens_path().expect("tokens path"));
        *provider
            .gemini
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        provider.set_active_provider(ActiveProvider::OpenAI);
        crate::auth::gemini::save_api_key("AIzaSyTestGeminiKey").expect("save gemini api key");

        provider
            .set_model("gemini-3-flash")
            .expect("newly saved Gemini API key in default mode must reconcile to Gemini");
        assert_eq!(provider.active_provider(), ActiveProvider::Gemini);
        assert_eq!(provider.model(), "gemini-3-flash");

        // 3. Simulate Anthropic credentials newly added while the in-memory
        // `anthropic` slot is still empty.
        provider.set_active_provider(ActiveProvider::OpenAI);
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-key");

        provider
            .set_model("claude-sonnet-4-6")
            .expect("newly configured Anthropic credentials must reconcile before falling back to Antigravity");
        assert_eq!(provider.active_provider(), ActiveProvider::Claude);
        assert_eq!(provider.model(), "claude-sonnet-4-6");
    });
}

#[test]
fn unprefixed_gemini_model_falls_back_to_antigravity_when_only_api_key_saved_under_forced_oauth() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        crate::env::set_var("JCODE_GEMINI_FORCE_OAUTH", "1");
        crate::config::Config::invalidate_cache();
        assert!(crate::auth::gemini::force_oauth());

        let provider = test_multi_provider_with_openai_and_antigravity();

        // Persist a Gemini Developer API key after startup without OAuth tokens
        // while OAuth is forced (`uses_api_key() == false`).
        crate::auth::gemini::save_api_key("AIzaSyForcedOAuthKeyOnly")
            .expect("save gemini api key");
        assert!(crate::auth::gemini::has_api_key());
        assert!(!crate::auth::gemini::uses_api_key());

        provider
            .set_model("gemini-3-flash")
            .expect("forced-OAuth key-only state must not hot-init unusable Gemini slot and should fall back to Antigravity");
        assert!(
            provider.gemini_provider().is_none(),
            "Gemini slot must remain empty when OAuth is forced and no OAuth tokens exist"
        );
        assert_eq!(provider.active_provider(), ActiveProvider::Antigravity);
        assert_eq!(provider.model(), "gemini-3-flash");

        // Startup path must also respect `uses_api_key()` when OAuth is forced.
        let fresh = MultiProvider::new_with_auth_status(crate::auth::AuthStatus::default());
        assert!(
            fresh.gemini_provider().is_none(),
            "startup must not initialize Gemini from an API key when OAuth is forced"
        );
    });
}

#[test]
fn unprefixed_antigravity_known_model_falls_back_to_antigravity_when_target_provider_genuinely_missing()
 {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let provider = test_multi_provider_with_openai_and_antigravity();

        for model in [
            "gemini-4-argon",
            "gemini-4-argon-low",
            "gemini-4-argon-medium",
            "gemini-4-argon-high",
            "gemini-3.8-flash-high",
            "gemini-3-flash",
            "claude-sonnet-4-6",
        ] {
            provider.set_active_provider(ActiveProvider::OpenAI);
            provider
                .set_model(model)
                .unwrap_or_else(|err| panic!("expected {model} to fall back to Antigravity: {err}"));
            assert_eq!(
                provider.active_provider(),
                ActiveProvider::Antigravity,
                "{model} should select Antigravity when target provider credentials are absent"
            );
            assert_eq!(provider.model(), model);
        }

        // A Gemini model that is NOT served by Antigravity must not fall back
        // to Antigravity; it should report missing Gemini credentials.
        provider.set_active_provider(ActiveProvider::OpenAI);
        let err = provider
            .set_model("gemini-2.5-pro")
            .expect_err("non-Antigravity Gemini model should fail when Gemini is unconfigured");
        assert!(
            err.to_string().contains("Gemini credentials not available"),
            "unexpected error for non-Antigravity Gemini model: {err}"
        );
    });
}

#[test]
fn active_antigravity_keeps_precedence_for_unprefixed_known_models_and_explicit_prefixes_override()
{
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();
        let anthropic = test_anthropic_runtime();
        let openai = test_openai_runtime();
        let provider = test_multi_provider_with_cursor();
        *provider
            .anthropic
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(anthropic.clone());
        *provider
            .openai
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(openai.clone());
        *provider
            .antigravity
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(test_antigravity_runtime());
        *provider
            .gemini
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(test_gemini_runtime());
        *provider
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Antigravity;

        // When Antigravity is active, unprefixed Antigravity-known models stay
        // on Antigravity even if Gemini and Claude are also configured.
        for model in [
            "argon",
            "argon-high",
            "gemini-4-argon",
            "gemini-4-argon-low",
            "gemini-3-flash",
            "claude-sonnet-4-6",
        ] {
            provider.set_active_provider(ActiveProvider::Antigravity);
            provider
                .set_model(model)
                .unwrap_or_else(|err| panic!("active Antigravity should accept {model}: {err}"));
            assert_eq!(provider.active_provider(), ActiveProvider::Antigravity);
            assert_eq!(provider.model(), model);
        }

        // Antigravity-exclusive models (Argon tiers, Gemini 3.8 Flash tiers,
        // GPT-OSS 120B) must select Antigravity even when OpenAI is active and
        // Gemini/Claude runtimes are also configured.
        for model in [
            "argon",
            "argon-high",
            "gemini-4-argon",
            "gemini-4-argon-medium",
            "gemini-4-argon-high",
            "gemini-3.8-flash-high",
            "gpt-oss-120b-medium",
        ] {
            provider.set_active_provider(ActiveProvider::OpenAI);
            provider
                .set_model(model)
                .unwrap_or_else(|err| panic!("Antigravity-exclusive {model} should route to Antigravity: {err}"));
            assert_eq!(provider.active_provider(), ActiveProvider::Antigravity);
            assert_eq!(provider.model(), model);
        }

        // Explicit provider and credential prefixes override active Antigravity.
        provider
            .set_model("gemini:gemini-3-flash")
            .expect("explicit gemini prefix should select Gemini");
        assert_eq!(provider.active_provider(), ActiveProvider::Gemini);
        assert_eq!(provider.model(), "gemini-3-flash");

        provider
            .set_model("antigravity:gemini-4-argon-high")
            .expect("explicit antigravity prefix should select Antigravity");
        assert_eq!(provider.active_provider(), ActiveProvider::Antigravity);
        assert_eq!(provider.model(), "gemini-4-argon-high");

        provider
            .set_model("claude-oauth:claude-sonnet-4-6")
            .expect("explicit claude-oauth prefix should select Claude OAuth");
        assert_eq!(provider.active_provider(), ActiveProvider::Claude);
        assert_eq!(provider.model(), "claude-sonnet-4-6");
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::OAuth
        );

        provider
            .set_model("claude-api:claude-sonnet-4-6")
            .expect("explicit claude-api prefix should select Claude API key");
        assert_eq!(provider.active_provider(), ActiveProvider::Claude);
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::ApiKey
        );

        provider
            .set_model("openai-oauth:gpt-5.4")
            .expect("explicit openai-oauth prefix should select OpenAI OAuth");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenAI);
        assert_eq!(provider.model(), "gpt-5.4");
        assert_eq!(
            openai.credential_mode(),
            jcode_provider_core::CredentialMode::OAuth
        );

        provider
            .set_model("openai-api:gpt-5.4")
            .expect("explicit openai-api prefix should select OpenAI API key");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenAI);
        assert_eq!(
            openai.credential_mode(),
            jcode_provider_core::CredentialMode::ApiKey
        );

        // Unrelated bare models keep their existing routes even when Antigravity is active.
        provider.set_active_provider(ActiveProvider::Antigravity);
        provider
            .set_model("gpt-5.4")
            .expect("bare OpenAI model should still route to OpenAI");
        assert_eq!(provider.active_provider(), ActiveProvider::OpenAI);
        assert_eq!(provider.model(), "gpt-5.4");

        provider.set_active_provider(ActiveProvider::Antigravity);
        provider
            .set_model("composer-2")
            .expect("bare Cursor model should still route to Cursor");
        assert_eq!(provider.active_provider(), ActiveProvider::Cursor);
        assert_eq!(provider.model(), "composer-2");
    });
}
