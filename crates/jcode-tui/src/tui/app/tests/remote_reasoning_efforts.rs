// Remote reasoning-effort cycling and `/effort` must honor a configured
// per-model `reasoning_map` on a named OpenAI-compatible profile: the TUI is a
// remote client, so the ladder cannot come from the local provider handle and
// must be resolved from the same config.toml the server reads.

#[derive(Clone, Default)]
struct EffortRecordingProvider {
    set_calls: StdArc<StdMutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Provider for EffortRecordingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("EffortRecordingProvider")
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        self.set_calls.lock().unwrap().push(effort.to_string());
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn with_mapped_profile_env<T>(ssh_remote: Option<&str>, config: &str, f: impl FnOnce() -> T) -> T {
    let _env_guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let saved_env = [
        "JCODE_HOME",
        "JCODE_SSH_REMOTE",
    ]
    .map(|key| (key, std::env::var_os(key)));

    crate::env::set_var("JCODE_HOME", temp.path());
    match ssh_remote {
        Some(host) => crate::env::set_var("JCODE_SSH_REMOTE", host),
        None => crate::env::remove_var("JCODE_SSH_REMOTE"),
    }

    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config parent");
    std::fs::write(&config_path, config).expect("write config");
    crate::config::Config::invalidate_cache();

    let result = f();

    for (key, value) in saved_env {
        if let Some(value) = value {
            crate::env::set_var(key, value);
        } else {
            crate::env::remove_var(key);
        }
    }
    crate::config::Config::invalidate_cache();
    result
}

const MAPPED_PROFILE_CONFIG: &str = r#"
[providers.mockgw]
base_url = "http://127.0.0.1:18923/v1"
default_model = "mock-model"
display_name = "Mock Gateway"

[[providers.mockgw.models]]
id = "mock-model"

[providers.mockgw.models.reasoning_map]
[providers.mockgw.models.reasoning_map.high]
reasoning_effort = "very_high"
[providers.mockgw.models.reasoning_map.xhigh]
reasoning_effort = "xhigh"
[providers.mockgw.models.reasoning_map.medium]
disabled = true
"#;

#[test]
fn remote_reasoning_efforts_prefers_configured_map_over_family_inference() {
    with_mapped_profile_env(None, MAPPED_PROFILE_CONFIG, || {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("mockgw".to_string());
        app.remote_provider_model = Some("mock-model".to_string());

        let (provider, model) = app.remote_effort_identity();
        let efforts =
            super::remote_reasoning_efforts(provider.as_deref(), model.as_deref());
        assert_eq!(
            efforts,
            vec![
                "high".to_string(),
                "xhigh".to_string(),
                "swarm".to_string(),
                "swarm-deep".to_string(),
            ],
            "local remote client must resolve the map rungs (medium disabled) plus swarm sentinels"
        );

        // Family inference alone returns an empty ladder for a generic named
        // profile, which used to disable effort cycling entirely.
        let inferred =
            jcode_provider_core::inferred_reasoning_efforts(Some("mockgw"), Some("mock-model"));
        assert!(
            inferred.is_empty(),
            "precondition: static family inference cannot see named profiles"
        );

        // Cycling from the lowest enabled rung must move within the mapped
        // ladder and never offer the disabled medium rung.
        let recorder = EffortRecordingProvider::default();
        let set_calls = recorder.set_calls.clone();
        app.provider = StdArc::new(recorder);
        app.remote_reasoning_effort = Some("high".to_string());
        app.cycle_effort(1);
        let notice = app.status_notice();
        assert!(
            notice
                .as_deref()
                .is_some_and(|n| n.contains("Effort:") && !n.contains("medium")),
            "cycling must pick a mapped rung, got: {notice:?}"
        );
        let applied = set_calls.lock().unwrap().clone();
        assert_eq!(
            applied,
            vec!["xhigh".to_string()],
            "cycling up from the lowest mapped rung must land on xhigh, skipping disabled medium"
        );
    });
}

#[test]
fn remote_reasoning_efforts_matches_profile_display_name() {
    with_mapped_profile_env(None, MAPPED_PROFILE_CONFIG, || {
        // The server reports the profile display_name as the provider name.
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("Mock Gateway".to_string());
        app.remote_provider_model = Some("mock-model".to_string());

        let (provider, model) = app.remote_effort_identity();
        let efforts =
            super::remote_reasoning_efforts(provider.as_deref(), model.as_deref());
        assert_eq!(
            efforts,
            vec![
                "high".to_string(),
                "xhigh".to_string(),
                "swarm".to_string(),
                "swarm-deep".to_string(),
            ]
        );
    });
}

#[test]
fn ssh_remote_falls_back_to_family_inference_not_local_config() {
    with_mapped_profile_env(Some("some-host"), MAPPED_PROFILE_CONFIG, || {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("mockgw".to_string());
        app.remote_provider_model = Some("mock-model".to_string());

        // SSH remotes must not trust the local config.toml: the server
        // machine may hold a different one.
        let (provider, model) = app.remote_effort_identity();
        let efforts =
            super::remote_reasoning_efforts(provider.as_deref(), model.as_deref());
        assert!(
            efforts.is_empty(),
            "SSH remote must use static inference, which is empty for named profiles"
        );
    });
}

#[test]
fn remote_reasoning_efforts_falls_back_when_no_map_configured() {
    let config = r#"
[providers.mockgw]
base_url = "http://127.0.0.1:18923/v1"
default_model = "mock-model"

[[providers.mockgw.models]]
id = "mock-model"
"#;
    with_mapped_profile_env(None, config, || {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("mockgw".to_string());
        app.remote_provider_model = Some("mock-model".to_string());

        // No map: fall back to family inference (empty for a generic named
        // profile), which reports "not available" instead of a bogus ladder.
        let (provider, model) = app.remote_effort_identity();
        let efforts =
            super::remote_reasoning_efforts(provider.as_deref(), model.as_deref());
        assert!(efforts.is_empty());
    });
}