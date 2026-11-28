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
            // The getter reads the live global config, so the fallback path
            // reflects the parsed [provider] value without an App snapshot.
            assert_eq!(
                crate::config::config().provider.auto_retry_base_delay_secs,
                7
            );
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
            // The global value stays untouched; only the resolution prefers
            // the live profile override.
            assert_eq!(
                crate::config::config().provider.auto_retry_base_delay_secs,
                7
            );
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
            assert_eq!(crate::config::config().provider.auto_retry_max_attempts, 4);
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
            assert_eq!(crate::config::config().provider.auto_retry_max_attempts, 4);
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

    /// The stale session key loses to the server-reported provider label.
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
            write_test_config("[providers.nvidia-nim]\nauto_retry_max_attempts = 2\n");
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

    /// A forced connectivity wait (a real provider attempt failed) schedules
    /// its retry with the configured base delay, not the hardcoded 5s
    /// offline-probe cadence (greptile 4449298934). The offline probe re-wait
    /// (force == false) keeps the short fixed timing.
    #[test]
    fn forced_network_wait_uses_configured_base_delay() {
        with_temp_jcode_home(|| {
            write_test_config("[provider]\nauto_retry_base_delay_secs = 60\n");
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.rate_limit_pending_message = Some(PendingRemoteMessage {
                jarvis_voice: false,
                content: "continue".to_string(),
                images: vec![],
                is_system: true,
                system_reminder: None,
                auto_retry: false,
                retry_attempts: 0,
                retry_at: None,
            });

            assert!(app.schedule_pending_remote_network_wait_with_force("dns", true));
            let before = std::time::Instant::now();
            let retry_at = app
                .rate_limit_pending_message
                .as_ref()
                .and_then(|p| p.retry_at)
                .expect("forced wait schedules a retry");
            assert!(
                retry_at.duration_since(before) >= std::time::Duration::from_secs(55),
                "first forced retry must respect the 60s configured delay, not 5s"
            );
        });
    }

    /// The offline-probe re-wait keeps its short cadence regardless of a large
    /// configured base delay: no provider attempt happened, so the configured
    /// retry backoff must not slow down offline detection.
    #[test]
    fn probe_network_wait_keeps_fixed_cadence() {
        with_temp_jcode_home(|| {
            write_test_config("[provider]\nauto_retry_base_delay_secs = 60\n");
            crate::config::invalidate_config_cache();

            let mut app = create_test_app();
            app.rate_limit_pending_message = Some(PendingRemoteMessage {
                jarvis_voice: false,
                content: "continue".to_string(),
                images: vec![],
                is_system: true,
                system_reminder: None,
                auto_retry: true,
                retry_attempts: 2,
                retry_at: None,
            });

            assert!(app.schedule_pending_remote_network_wait("probe still failing"));
            let before = std::time::Instant::now();
            let retry_at = app
                .rate_limit_pending_message
                .as_ref()
                .and_then(|p| p.retry_at)
                .expect("probe wait schedules a retry");
            assert!(
                retry_at.duration_since(before) <= std::time::Duration::from_secs(20),
                "probe re-wait must stay on the short fixed cadence"
            );
        });
    }

    /// Alt+S in the runtime model picker persists the highlighted model as
    /// the memory sidecar model (`agents.memory_model` in config.toml).
    #[test]
    fn alt_s_sets_selected_model_as_memory_sidecar() {
        with_temp_jcode_home(|| {
            let mut app = create_test_app();
            app.is_remote = true;
            app.remote_provider_model = Some("gpt-5.3-codex".to_string());
            app.remote_available_entries =
                vec!["gpt-5.3-codex".to_string(), "gpt-5.2-codex".to_string()];
            app.open_model_picker();
            wait_for_model_picker_load(&mut app);
            // Capture which entry is actually highlighted so the assertion
            // follows the picker's initial selection order. Effort variants
            // append a " (high)" suffix to the display name; the persisted spec
            // is the bare model id.
            let highlighted = {
                let picker = app.inline_interactive_state.as_ref().expect("picker open");
                let idx = picker.filtered[picker.selected];
                let name = picker.entries[idx].name.clone();
                match name.split_once(" (") {
                    Some((base, _)) => base.to_string(),
                    None => name,
                }
            };

            app.handle_inline_interactive_key(KeyCode::Char('s'), KeyModifiers::ALT)
                .expect("Alt+S handled");

            let saved = crate::config::Config::load().agents.memory_model;
            let saved = saved.as_deref().expect("Alt+S must persist the sidecar model");
            assert!(
                saved.ends_with(&highlighted),
                "persisted sidecar model {saved} must be the highlighted model {highlighted}"
            );
            // The picker stays open: the shortcut changes the sidecar target
            // without dismissing the list.
            assert!(app.inline_interactive_state.is_some());
        });
    }

    /// `set_selected_model_as_sidecar` is a no-op when no model picker is open.
    #[test]
    fn alt_s_without_picker_does_nothing() {
        with_temp_jcode_home(|| {
            let mut app = create_test_app();
            app.handle_inline_interactive_key(KeyCode::Char('s'), KeyModifiers::ALT)
                .expect("Alt+S handled");
            assert!(crate::config::Config::load().agents.memory_model.is_none());
        });
    }
/// The auto-poke short-retry path requests a budget of 2, but a configured
/// `auto_retry_max_attempts = 1` must cap it: the retry notice reports the
/// real budget ("attempt 1/1"), and the second deterministic error stops
/// auto-poke instead of sneaking in a second attempt (greptile 4307529100).
#[test]
fn test_remote_short_auto_poke_retry_is_capped_by_configured_max_attempts() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\nauto_retry_max_attempts = 1\n");
        crate::config::invalidate_config_cache();

        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        assert_eq!(app.effective_auto_retry_max_attempts(), 1);

        app.auto_poke_incomplete_todos = true;
        app.queued_messages.push(
            "You have 1 incomplete todo. Continue working, or update the todo tool.".to_string(),
        );
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            jarvis_voice: false,
            content: "You have 1 incomplete todo. Continue working, or update the todo tool."
                .to_string(),
            images: vec![],
            is_system: true,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 21,
                message: "OpenAI API error 400 Bad Request: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );

        // The hard-coded short budget (2) must be capped to the configured
        // maximum: one attempt, reported as 1/1.
        assert!(app.auto_poke_incomplete_todos);
        let pending = app
            .rate_limit_pending_message
            .as_ref()
            .expect("capped short retry should still schedule the first attempt");
        assert_eq!(pending.retry_attempts, 1);
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.role == "system" && m.content.contains("attempt 1/1")),
            "the retry notice must report the capped budget, not 1/2"
        );

        // With the capped budget spent, the next deterministic error must
        // stop auto-poke instead of granting a second attempt.
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 22,
                message: "OpenAI API error 400 Bad Request: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );

        assert!(!app.auto_poke_incomplete_todos);
        assert!(app.rate_limit_pending_message.is_none());
    });
}

}
