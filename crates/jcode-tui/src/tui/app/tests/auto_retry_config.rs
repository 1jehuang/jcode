// Unit tests for the auto-retry config resolution on `App`.
//
// `effective_auto_retry_base_delay_secs` and `effective_auto_retry_max_attempts`
// check the active named provider profile override first, then fall back to
// the global `[provider]` value captured on `App` at construction. These tests
// pin both branches of each resolution: the global-default fallback, the
// per-profile override, and the missing/empty-profile degradation paths.
//
// Wrapped in a `mod auto_retry_config` so the test paths contain the filter
// `auto_retry_config` used by the suite runner.
mod auto_retry_config {
    use super::*;

    /// The global `[provider]` value is returned verbatim when the session has
    /// no active named provider profile (`session.provider_key` is `None`).
    #[test]
    fn base_delay_returns_global_when_no_profile() {
        with_temp_jcode_home(|| {
            write_test_config("[provider]\nauto_retry_base_delay_secs = 7\n");
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.session.provider_key = None;

            assert_eq!(app.effective_auto_retry_base_delay_secs(), 7);
            // The App field mirrors the global value parsed at construction, so
            // the fallback path returns the same number stored on App.
            assert_eq!(app.auto_retry_base_delay_secs, 7);
        });
    }

    /// A per-profile override takes precedence over the global `[provider]`
    /// value when the matching named provider profile is active.
    #[test]
    fn base_delay_uses_profile_override() {
        with_temp_jcode_home(|| {
            write_test_config(
                "[provider]\nauto_retry_base_delay_secs = 7\n\
                 [providers.shared-gateway]\nauto_retry_base_delay_secs = 42\n",
            );
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.session.provider_key = Some("shared-gateway".to_string());

            assert_eq!(
                app.effective_auto_retry_base_delay_secs(),
                42,
                "per-profile override must win over the global value"
            );
            // The global value is still what App captured at construction; only
            // the resolution prefers the live profile override.
            assert_eq!(app.auto_retry_base_delay_secs, 7);
        });
    }

    /// The global `[provider]` value is returned verbatim when no named
    /// provider profile is active.
    #[test]
    fn max_attempts_returns_global_when_no_profile() {
        with_temp_jcode_home(|| {
            write_test_config("[provider]\nauto_retry_max_attempts = 4\n");
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.session.provider_key = None;

            assert_eq!(app.effective_auto_retry_max_attempts(), 4);
            assert_eq!(app.auto_retry_max_attempts, 4);
        });
    }

    /// A per-profile override takes precedence over the global value.
    #[test]
    fn max_attempts_uses_profile_override() {
        with_temp_jcode_home(|| {
            write_test_config(
                "[provider]\nauto_retry_max_attempts = 4\n\
                 [providers.shared-gateway]\nauto_retry_max_attempts = 9\n",
            );
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.session.provider_key = Some("shared-gateway".to_string());

            assert_eq!(
                app.effective_auto_retry_max_attempts(),
                9,
                "per-profile override must win over the global value"
            );
            assert_eq!(app.auto_retry_max_attempts, 4);
        });
    }

    /// When `session.provider_key` points at a profile that does not exist in
    /// the config, the resolution degrades to the global value instead of
    /// panicking.
    #[test]
    fn falls_back_to_global_when_profile_missing() {
        with_temp_jcode_home(|| {
            write_test_config("[provider]\nauto_retry_base_delay_secs = 5\n");
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            // A provider key that has no matching [providers.*] section.
            app.session.provider_key = Some("no-such-profile".to_string());

            assert_eq!(app.effective_auto_retry_base_delay_secs(), 5);
        });
    }

    /// When the active profile exists but does not set the override, the global
    /// value is used (a `None` override must not short-circuit the fallback).
    #[test]
    fn falls_back_to_global_when_profile_has_no_override() {
        with_temp_jcode_home(|| {
            write_test_config(
                "[provider]\nauto_retry_max_attempts = 6\n\
                 [providers.shared-gateway]\nbase_url = \"https://example.invalid\"\n",
            );
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.session.provider_key = Some("shared-gateway".to_string());

            assert_eq!(app.effective_auto_retry_max_attempts(), 6);
        });
    }

    /// `set_selected_model_as_sidecar` is a no-op when no model picker is open
    #[test]
    fn remote_provider_label_overrides_session_key() {
        with_temp_jcode_home(|| {
            write_test_config(
                "[provider]\nauto_retry_max_attempts = 4\n\
                 [providers.shared-gateway]\nauto_retry_max_attempts = 9\n",
            );
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.is_remote = true;
            // The stale key the server never updates on a remote `/model` switch.
            app.session.provider_key = Some("copilot".to_string());
            // The server reports a named profile's id verbatim
            // (`runtime_display_name`, openrouter-runtime lib.rs).
            app.remote_provider_name = Some("shared-gateway".to_string());

            assert_eq!(
                app.auto_retry_provider_profile_key(),
                Some("shared-gateway".to_string()),
                "remote label must win over the stale session key"
            );
        });
    }

    /// A catalog display name (profile id `nvidia-nim`) resolves via the
    /// openai-compatible catalog lookup, not the canonical map.
    #[test]
    fn remote_provider_label_resolves_catalog_profile() {
        with_temp_jcode_home(|| {
            write_test_config(
                "[providers.nvidia-nim]\nauto_retry_max_attempts = 2\n",
            );
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.is_remote = true;
            app.session.provider_key = Some("claude".to_string());
            app.remote_provider_name = Some("NVIDIA NIM".to_string());

            assert_eq!(
                app.auto_retry_provider_profile_key(),
                Some("nvidia-nim".to_string()),
                "catalog display name must resolve to the profile id"
            );
        });
    }

    /// Both remote label vocabularies for Anthropic (the ModelChanged
    /// `display_name()` "Claude" and the History-restore "Anthropic") must
    /// resolve to the same session key, and neither may pick up client-side
    /// env poison.
    #[test]
    fn remote_provider_label_resolves_anthropic_aliases_without_env_poison() {
        with_temp_jcode_home(|| {
            write_test_config("[provider]\nauto_retry_max_attempts = 4\n");
            crate::config::invalidate_config_cache();

            // Simulate a client whose last local provider was DeepSeek; the
            // old derive path consulted these and would mis-resolve. Guarded
            // so it cannot leak into the next test.
            let _env_guard = EnvRestoreGuard::capture(["JCODE_OPENROUTER_CACHE_NAMESPACE"]);
            crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", "deepseek");

            for label in ["Claude", "Anthropic", "Claude CLI"] {
                let mut app = create_test_app();
                app.is_remote = true;
                app.session.provider_key = Some("deepseek".to_string());
                app.remote_provider_name = Some(label.to_string());

                assert_eq!(
                    app.auto_retry_provider_profile_key(),
                    Some("claude".to_string()),
                    "label {label:?} must resolve to claude, not the env-derived key"
                );
            }
        });
    }

    /// An unknown remote label is kept lowercased verbatim: the openai
    /// catalog lookup misses (custom endpoint), and the canonical map has no
    /// arm for it, so the label itself becomes the lookup key. A matching
    /// `[providers.<id>]` section (lowercase id) still applies.
    #[test]
    fn remote_provider_label_unknown_keeps_verbatim_key() {
        with_temp_jcode_home(|| {
            write_test_config(
                "[provider]\nauto_retry_max_attempts = 4\n\
                 [providers.custom-gateway]\nauto_retry_max_attempts = 9\n",
            );
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.is_remote = true;
            app.session.provider_key = Some("claude".to_string());
            // Not a catalog display name, not a canonical provider name. The
            // server reports the profile id verbatim for user-defined
            // OpenAI-compatible endpoints.
            app.remote_provider_name = Some("custom-gateway".to_string());

            assert_eq!(
                app.auto_retry_provider_profile_key(),
                Some("custom-gateway".to_string()),
                "unknown label falls back to the lowercased verbatim label"
            );
            // The resolved key must actually hit the per-profile override
            // (this is the greptile finding: the retry used the stale
            // `session.provider_key` and read the wrong config section).
            assert_eq!(app.effective_auto_retry_max_attempts(), 9);
        });
    }

    /// A local session keeps using `session.provider_key` untouched: remote
    /// resolution only engages when `is_remote` is set.
    #[test]
    fn local_session_keeps_provider_key() {
        with_temp_jcode_home(|| {
            let mut app = create_test_app();
            app.is_remote = false;
            app.session.provider_key = Some("copilot".to_string());

            assert_eq!(
                app.auto_retry_provider_profile_key(),
                Some("copilot".to_string())
            );
        });
    }

    /// `set_selected_model_as_sidecar` is a no-op when no model picker is open
    /// (`inline_interactive_state` is `None`): it returns early without
    /// panicking and without mutating status notice or transcript state. This is
    /// the common path — the user hits Alt+S while no picker is visible. The
    /// full picker+model+route happy path needs a populated model picker fed by
    /// a provider catalog and a writable memory config, which is beyond a unit
    /// test of this function; here we pin the no-op guard branch.
    #[test]
    fn set_selected_model_as_sidecar_no_picker_is_noop() {
        with_temp_jcode_home(|| {
            let mut app = create_test_app();
            // No picker is open on a fresh app, so the guard branch runs.
            assert!(
                app.inline_interactive_state.is_none(),
                "precondition: a fresh app should have no inline interactive picker"
            );
            let notice_before = app.status_notice.clone();
            let messages_before = app.display_messages.len();

            app.set_selected_model_as_sidecar();

            // Early-return: nothing should have been mutated.
            assert_eq!(
                app.status_notice, notice_before,
                "no status notice should be set when there is no picker"
            );
            assert_eq!(
                app.display_messages.len(),
                messages_before,
                "no display message should be pushed when there is no picker"
            );
        });
    }
}