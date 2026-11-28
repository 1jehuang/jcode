#[test]
fn memory_effort_config_reaches_provider_forks_unchanged() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp jcode home");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _openai = EnvVarGuard::unset("OPENAI_API_KEY");
    let _effort = EnvVarGuard::unset("JCODE_MEMORY_EFFORT");
    let _model = EnvVarGuard::unset("JCODE_MEMORY_MODEL");
    let stub = std::sync::Arc::new(EffortRecordingStub::default());
    crate::provider::set_active_provider(stub.clone());
    for effort in ["off", "auto", "HiGh", "swarm", "FutureProviderEffort-v9"] {
        std::fs::write(
            temp.path().join("config.toml"),
            format!(
                "[agents]\nmemory_model = \"effort-stub-model\"\nmemory_effort = \"  {effort}  \"\n"
            ),
        )
        .unwrap();
        *stub.set_effort.lock().unwrap() = None;
        let sidecar = Sidecar::new();
        assert_eq!(sidecar.backend_name(), "provider");
        assert_eq!(stub.recorded_effort().as_deref(), Some(effort));
        assert_eq!(sidecar.reasoning_override.as_deref(), Some(effort));

        *stub.set_effort.lock().unwrap() = None;
        let session = crate::session::Session::create_with_id("effort-boundary".into(), None, None);
        let sidecar = Sidecar::for_session(&session);
        assert_eq!(sidecar.backend_name(), "provider");
        assert_eq!(stub.recorded_effort().as_deref(), Some(effort));
    }
}

#[test]
fn openai_effort_wire_accepts_only_named_levels() {
    for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        for raw in [
            effort.to_string(),
            format!(" {} ", effort.to_ascii_uppercase()),
        ] {
            let request = build_openai_request("synthetic-model", "", "", false, Some(&raw));
            assert_eq!(request["reasoning"]["effort"].as_str(), Some(effort));
        }
    }
    for raw in [
        None,
        Some(""),
        Some(" \t"),
        Some("invalid"),
        Some("off"),
        Some("auto"),
        Some("swarm-deep"),
        Some("FutureProviderEffort-v9"),
    ] {
        let request = build_openai_request("synthetic-model", "", "", false, raw);
        assert!(request.get("reasoning").is_none(), "{raw:?}: {request}");
    }
}

#[test]
fn openai_effort_swarm_resolves_config_and_env_root() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _root = EnvVarGuard::unset("JCODE_SWARM_ROOT_EFFORT");
    let _effort = EnvVarGuard::unset("JCODE_MEMORY_EFFORT");
    std::fs::write(
        temp.path().join("config.toml"),
        "[agents]\nmemory_effort = \" SwArM \"\nswarm_root_effort = \"high\"\n",
    )
    .unwrap();
    let configured = normalized_memory_effort(crate::config::config().agents.memory_effort.clone());
    assert_eq!(configured.as_deref(), Some("SwArM"));
    for (root, expected) in [
        (None, "high"),
        (Some("minimal"), "minimal"),
        (Some("none"), "none"),
    ] {
        if let Some(root) = root {
            crate::env::set_var("JCODE_SWARM_ROOT_EFFORT", root);
        }
        for raw in [configured.as_deref().unwrap(), "swarm", " \tSWARM\n "] {
            let request = build_openai_request("synthetic-model", "", "", false, Some(raw));
            assert_eq!(
                request["reasoning"]["effort"].as_str(),
                Some(expected),
                "{raw:?}"
            );
        }
        let explicit = build_openai_request("synthetic-model", "", "", false, Some("low"));
        assert_eq!(explicit["reasoning"]["effort"].as_str(), Some("low"));
    }
}

#[test]
fn claude_effort_aliases_preserve_caps_and_budgets() {
    for raw in ["none", "off", " DISABLED ", "NoNe"] {
        let (thinking, output, limit) = claude_reasoning_parts("claude-opus-4-7", Some(raw), 2_000);
        assert!(thinking.is_none() && output.is_none(), "{raw}");
        assert_eq!(limit, 2_000);
        for model in ["claude-opus-5-5", "claude-fable-5-1"] {
            let (thinking, output, limit) = claude_reasoning_parts(model, Some(raw), 2_000);
            assert!(matches!(thinking, Some(ClaudeThinking::Adaptive { .. })));
            assert_eq!(output.unwrap().effort, "low");
            assert_eq!(limit, 2_000);
        }
    }
    for model in [
        "claude-opus-4-5",
        "claude-opus-4-7",
        "claude-opus-5-5",
        "claude-haiku-3",
    ] {
        for raw in [
            None,
            Some(""),
            Some(" \t"),
            Some("auto"),
            Some(" DeFaUlT "),
            Some("invalid"),
            Some("FutureProviderEffort-v9"),
        ] {
            let (thinking, output, limit) = claude_reasoning_parts(model, raw, 2_000);
            assert!(thinking.is_none() && output.is_none(), "{model}: {raw:?}");
            assert_eq!(limit, 2_000);
        }
    }
    for (raw, expected, budget) in [
        (" MiNiMaL ", "low", 1_024),
        (" LoW ", "low", 1_024),
        (" MeDiUm ", "medium", 4_096),
        (" HiGh ", "high", 8_192),
        (" XhIgH ", "high", 16_384),
        (" MaX ", "high", 16_384),
    ] {
        let (thinking, output, limit) = claude_reasoning_parts("claude-opus-4-5", Some(raw), 2_000);
        assert_eq!(output.unwrap().effort, expected);
        assert!(
            matches!(thinking, Some(ClaudeThinking::Enabled { budget_tokens }) if budget_tokens == budget)
        );
        assert_eq!(limit, 2_000.max(budget + CLAUDE_THINKING_ANSWER_HEADROOM));
    }
}

#[test]
fn claude_effort_swarm_resolves_configured_root() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _root = EnvVarGuard::unset("JCODE_SWARM_ROOT_EFFORT");
    std::fs::write(
        temp.path().join("config.toml"),
        "[agents]\nswarm_root_effort = \"high\"\n",
    )
    .unwrap();
    let expected = crate::prompt::swarm_root_reasoning_effort("swarm").unwrap();
    assert_eq!(expected, "high");
    let (thinking, output, limit) =
        claude_reasoning_parts("claude-opus-4-7", Some(" SwArM "), 2_000);
    let (expected_thinking, expected_output, expected_limit) =
        claude_reasoning_parts("claude-opus-4-7", Some(expected), 2_000);
    assert_eq!(
        serde_json::to_value(thinking).unwrap(),
        serde_json::to_value(expected_thinking).unwrap()
    );
    assert_eq!(
        output.map(|output| output.effort),
        expected_output.map(|output| output.effort)
    );
    assert_eq!(limit, expected_limit);
}
