use super::{
    AmbientConfig, Config, DiffDisplayMode, DisplayConfig, ProviderConfig,
    SessionPickerResumeAction, SwarmSpawnMode, ToolConfig, config_env_fingerprint,
    populate_context_limits_from_config_ref,
};
use std::ffi::OsString;
use std::path::Path;

fn restore_env_var(key: &str, previous: Option<OsString>) {
    if let Some(previous) = previous {
        crate::env::set_var(key, previous);
    } else {
        crate::env::remove_var(key);
    }
}

#[test]
fn test_openai_reasoning_effort_defaults_to_low() {
    assert_eq!(
        ProviderConfig::default().openai_reasoning_effort.as_deref(),
        Some("low")
    );
}

#[test]
fn test_openai_fast_mode_defaults_to_priority() {
    assert_eq!(
        ProviderConfig::default().openai_service_tier.as_deref(),
        Some("priority")
    );
}

#[test]
fn preserve_reasoning_context_defaults_to_enabled() {
    assert!(ProviderConfig::default().preserve_reasoning_context);
}

#[test]
fn swarm_spawn_mode_defaults_to_visible() {
    assert_eq!(
        Config::default().agents.swarm_spawn_mode,
        SwarmSpawnMode::Visible
    );
}

#[test]
fn swarm_spawn_mode_parses_supported_values() {
    let cfg: Config = toml::from_str("[agents]\nswarm_spawn_mode = \"headless\"\n")
        .expect("headless swarm_spawn_mode should parse");
    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Headless);

    let cfg: Config = toml::from_str("[agents]\nswarm_spawn_mode = \"auto\"\n")
        .expect("auto swarm_spawn_mode should parse");
    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Auto);

    let cfg: Config = toml::from_str("[agents]\nswarm_spawn_mode = \"visible\"\n")
        .expect("visible swarm_spawn_mode should parse");
    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Visible);
}

#[test]
fn swarm_spawn_mode_rejects_invalid_values() {
    let result = toml::from_str::<Config>("[agents]\nswarm_spawn_mode = \"background\"\n");
    assert!(result.is_err());
}

#[test]
fn swarm_spawn_mode_as_str_round_trips() {
    for mode in [
        SwarmSpawnMode::Visible,
        SwarmSpawnMode::Headless,
        SwarmSpawnMode::Auto,
    ] {
        assert_eq!(SwarmSpawnMode::parse(mode.as_str()), Some(mode));
    }
}

#[test]
fn test_env_override_swarm_spawn_mode() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_SWARM_SPAWN_MODE");
    crate::env::set_var("JCODE_SWARM_SPAWN_MODE", "headless");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.agents.swarm_spawn_mode, SwarmSpawnMode::Headless);

    restore_env_var("JCODE_SWARM_SPAWN_MODE", prev);
}

#[test]
fn test_env_override_swarm_model() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_SWARM_MODEL");
    crate::env::set_var("JCODE_SWARM_MODEL", "claude-opus-4-6");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.agents.swarm_model.as_deref(), Some("claude-opus-4-6"));

    // Empty value clears the override back to "inherit".
    crate::env::set_var("JCODE_SWARM_MODEL", "  ");
    let mut cfg = Config::default();
    cfg.agents.swarm_model = Some("preset".to_string());
    cfg.apply_env_overrides();
    assert_eq!(cfg.agents.swarm_model, None);

    restore_env_var("JCODE_SWARM_MODEL", prev);
}

#[test]
fn spawn_hook_defaults_to_none_and_parses_from_toml() {
    assert_eq!(Config::default().terminal.spawn_hook, None);

    let cfg: Config = toml::from_str("[terminal]\nspawn_hook = \"tmux new-window\"\n")
        .expect("spawn_hook should parse");
    assert_eq!(cfg.terminal.spawn_hook.as_deref(), Some("tmux new-window"));
}

#[test]
fn terminal_preferred_defaults_to_none_and_parses_from_toml() {
    assert_eq!(Config::default().terminal.preferred, None);

    let cfg: Config = toml::from_str("[terminal]\npreferred = \"ghostty\"\n")
        .expect("preferred should parse");
    assert_eq!(cfg.terminal.preferred.as_deref(), Some("ghostty"));
}

#[test]
fn hooks_config_defaults_and_parses_from_toml() {
    let defaults = Config::default().hooks;
    assert_eq!(defaults.turn_start, None);
    assert_eq!(defaults.turn_end, None);
    assert_eq!(defaults.session_start, None);
    assert_eq!(defaults.session_end, None);
    assert_eq!(defaults.pre_tool, None);
    assert_eq!(defaults.post_tool, None);
    assert_eq!(defaults.pre_tool_timeout_ms, 5000);

    let cfg: Config = toml::from_str(
        "[hooks]\nturn_start = \"notify-start\"\nturn_end = \"notify-turn\"\npre_tool = \"~/bin/policy\"\npre_tool_timeout_ms = 1500\n",
    )
    .expect("hooks config should parse");
    assert_eq!(cfg.hooks.turn_start.as_deref(), Some("notify-start"));
    assert_eq!(cfg.hooks.turn_end.as_deref(), Some("notify-turn"));
    assert_eq!(cfg.hooks.pre_tool.as_deref(), Some("~/bin/policy"));
    assert_eq!(cfg.hooks.pre_tool_timeout_ms, 1500);
}

#[test]
fn test_env_override_lifecycle_hooks() {
    let _guard = crate::storage::lock_test_env();
    let prev_turn_end = std::env::var_os("JCODE_HOOK_TURN_END");
    let prev_timeout = std::env::var_os("JCODE_HOOK_PRE_TOOL_TIMEOUT_MS");

    crate::env::set_var("JCODE_HOOK_TURN_END", "my-notifier --fast");
    crate::env::set_var("JCODE_HOOK_PRE_TOOL_TIMEOUT_MS", "250");
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(cfg.hooks.turn_end.as_deref(), Some("my-notifier --fast"));
    assert_eq!(cfg.hooks.pre_tool_timeout_ms, 250);

    // Empty env value disables a config-file hook.
    crate::env::set_var("JCODE_HOOK_TURN_END", " ");
    let mut cfg = Config::default();
    cfg.hooks.turn_end = Some("from-config".to_string());
    cfg.apply_env_overrides();
    assert_eq!(cfg.hooks.turn_end, None);

    restore_env_var("JCODE_HOOK_TURN_END", prev_turn_end);
    restore_env_var("JCODE_HOOK_PRE_TOOL_TIMEOUT_MS", prev_timeout);
}

#[test]
fn test_env_override_spawn_hook() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_SPAWN_HOOK");
    crate::env::set_var("JCODE_SPAWN_HOOK", "kitty @ launch --type=tab --");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(
        cfg.terminal.spawn_hook.as_deref(),
        Some("kitty @ launch --type=tab --")
    );

    // Empty env value disables a config-file hook.
    crate::env::set_var("JCODE_SPAWN_HOOK", "  ");
    let mut cfg = Config::default();
    cfg.terminal.spawn_hook = Some("tmux new-window".to_string());
    cfg.apply_env_overrides();
    assert_eq!(cfg.terminal.spawn_hook, None);

    restore_env_var("JCODE_SPAWN_HOOK", prev);
}

#[test]
fn test_env_override_focus_hook() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_FOCUS_HOOK");
    crate::env::set_var("JCODE_FOCUS_HOOK", "niri-focus-jcode");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(cfg.terminal.focus_hook.as_deref(), Some("niri-focus-jcode"));

    // Empty env value disables a config-file hook.
    crate::env::set_var("JCODE_FOCUS_HOOK", "");
    let mut cfg = Config::default();
    cfg.terminal.focus_hook = Some("wmctrl -a".to_string());
    cfg.apply_env_overrides();
    assert_eq!(cfg.terminal.focus_hook, None);

    restore_env_var("JCODE_FOCUS_HOOK", prev);
}

#[test]
fn test_memory_sidecar_enabled_defaults_true() {
    // The LLM precision-judge path is the only reliably productive memory mode,
    // so memory uses it by default. Users opt into the no-LLM hybrid path
    // explicitly by setting this false.
    let cfg = Config::default();
    assert!(cfg.agents.memory_sidecar_enabled);
}

#[test]
fn test_env_override_memory_sidecar() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MEMORY_MODEL");
    let prev_enabled = std::env::var_os("JCODE_MEMORY_SIDECAR_ENABLED");
    crate::env::set_var("JCODE_MEMORY_MODEL", "claude-haiku-4");
    crate::env::set_var("JCODE_MEMORY_SIDECAR_ENABLED", "true");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.agents.memory_model.as_deref(), Some("claude-haiku-4"));
    assert!(cfg.agents.memory_sidecar_enabled);

    restore_env_var("JCODE_MEMORY_MODEL", prev_model);
    restore_env_var("JCODE_MEMORY_SIDECAR_ENABLED", prev_enabled);
}

#[test]
fn tool_config_defaults_to_full_toolset() {
    let selection = ToolConfig::default().selection();
    assert!(selection.allowed_tools.is_none());
    assert!(selection.disabled_tools.contains("gmail"));
    assert!(selection.disabled_tools.contains("lsp"));
}

#[test]
fn tool_config_explicit_enabled_default_disabled_tools_opts_in() {
    let cfg = ToolConfig {
        enabled: vec!["gmail".to_string(), "lsp".to_string()],
        ..ToolConfig::default()
    };
    let selection = cfg.selection();
    let allowed = selection
        .allowed_tools
        .expect("explicit enabled is an allow-list");

    assert!(allowed.contains("gmail"));
    assert!(allowed.contains("lsp"));
    assert!(!selection.disabled_tools.contains("gmail"));
    assert!(!selection.disabled_tools.contains("lsp"));
}

#[test]
fn tool_config_all_enabled_sentinel_opts_in_gmail_without_allow_list() {
    let cfg = ToolConfig {
        enabled: vec!["*".to_string()],
        ..ToolConfig::default()
    };
    let selection = cfg.selection();

    assert!(selection.allowed_tools.is_none());
    assert!(!selection.disabled_tools.contains("gmail"));
    assert!(!selection.disabled_tools.contains("lsp"));
}

#[test]
fn tool_config_explicit_disabled_overrides_all_enabled_sentinel() {
    let cfg = ToolConfig {
        enabled: vec!["*".to_string()],
        disabled: vec!["gmail".to_string()],
        ..ToolConfig::default()
    };
    let selection = cfg.selection();

    assert!(selection.allowed_tools.is_none());
    assert!(selection.disabled_tools.contains("gmail"));
    assert!(!selection.disabled_tools.contains("lsp"));
}

#[test]
fn tool_config_acp_profile_allows_core_coding_plus_batch() {
    let cfg = ToolConfig {
        profile: "acp".to_string(),
        ..ToolConfig::default()
    };
    let allowed = cfg.allowed_tools().expect("acp profile is an allow-list");

    assert!(allowed.contains("bash"));
    assert!(allowed.contains("read"));
    assert!(allowed.contains("write"));
    assert!(allowed.contains("apply_patch"));
    assert!(allowed.contains("agentgrep"));
    assert!(allowed.contains("batch"));
    assert!(!allowed.contains("swarm"));
    assert!(!allowed.contains("subagent"));
    assert!(!allowed.contains("side_panel"));
}

#[test]
fn acp_config_defaults_to_standard_profile_and_acp_tools() {
    let cfg = Config::default();
    assert_eq!(cfg.acp.profile, "standard");
    assert_eq!(cfg.acp.tool_profile, "acp");
}

#[test]
fn tool_config_minimal_profile_allows_core_coding_tools() {
    let cfg = ToolConfig {
        profile: "minimal".to_string(),
        ..ToolConfig::default()
    };
    let allowed = cfg
        .allowed_tools()
        .expect("minimal profile is an allow-list");

    assert!(allowed.contains("bash"));
    assert!(allowed.contains("read"));
    assert!(allowed.contains("write"));
    assert!(allowed.contains("apply_patch"));
    assert!(allowed.contains("agentgrep"));
    assert!(!allowed.contains("browser"));
    assert!(!allowed.contains("swarm"));
}

#[test]
fn tool_config_explicit_enabled_and_disabled_lists_compose() {
    let cfg = ToolConfig {
        enabled: vec![
            "shell".to_string(),
            "read_file".to_string(),
            "browser".to_string(),
        ],
        disabled: vec!["browser".to_string()],
        ..ToolConfig::default()
    };
    let selection = cfg.selection();
    let allowed = selection
        .allowed_tools
        .expect("explicit enabled is an allow-list");

    assert!(allowed.contains("bash"));
    assert!(allowed.contains("read"));
    assert!(!allowed.contains("shell"));
    assert!(!allowed.contains("read_file"));
    assert!(!allowed.contains("browser"));
    assert!(selection.disabled_tools.contains("browser"));
}

#[test]
fn tool_config_none_profile_disables_all_tools() {
    let cfg = ToolConfig {
        profile: "none".to_string(),
        ..ToolConfig::default()
    };
    assert!(
        cfg.allowed_tools()
            .expect("none profile is empty")
            .is_empty()
    );
}

#[test]
fn tool_config_disabled_only_keeps_full_profile_with_deny_list() {
    let cfg = ToolConfig {
        disabled: vec!["browser".to_string(), "swarm".to_string()],
        ..ToolConfig::default()
    };
    let selection = cfg.selection();

    assert!(selection.allowed_tools.is_none());
    assert!(selection.disabled_tools.contains("browser"));
    assert!(selection.disabled_tools.contains("swarm"));
    assert!(selection.disabled_tools.contains("gmail"));
    assert!(selection.disabled_tools.contains("lsp"));
}

#[test]
fn test_generated_default_config_uses_low_openai_reasoning_effort() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = Config::create_default_config_file().expect("create default config file");
    let content = std::fs::read_to_string(path).expect("read default config file");

    assert!(
        content.contains("openai_reasoning_effort = \"low\""),
        "generated default config should use low OpenAI reasoning effort"
    );
    assert!(
        content.contains("openai_service_tier = \"priority\""),
        "generated default config should enable OpenAI fast mode"
    );
    assert!(
        content.contains("[tools]") && content.contains("profile = \"full\""),
        "generated default config should document tool profiles"
    );
    assert!(
        content.contains("[acp]") && content.contains("tool_profile = \"acp\""),
        "generated default config should document ACP profile settings"
    );
    assert!(
        content.contains("[agents]") && content.contains("swarm_spawn_mode = \"visible\""),
        "generated default config should document agent spawn defaults"
    );

    // Effort keys come from the per-platform keybinding registry; the template
    // placeholders must always be substituted.
    assert!(
        !content.contains("@EFFORT_INCREASE@") && !content.contains("@EFFORT_DECREASE@"),
        "generated default config should substitute effort key placeholders"
    );
    let expected_increase = if cfg!(target_os = "macos") {
        "effort_increase = \"cmd+right\""
    } else {
        "effort_increase = \"alt+right\""
    };
    assert!(
        content.contains(expected_increase),
        "generated default config should use the platform effort_increase default"
    );

    // The generated file must always be valid TOML for the current Config schema.
    let parsed: Config =
        toml::from_str(&content).expect("generated default config should parse as Config");
    assert_eq!(parsed.agents.swarm_spawn_mode, SwarmSpawnMode::Visible);

    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn global_config_cache_reloads_after_manual_file_edit() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    let path = Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config parent");
    std::fs::write(&path, "[display]\ncentered = false\n").expect("write initial config");

    assert!(!crate::config::config().display.centered);

    // Different length as well as mtime so the metadata fingerprint notices the
    // manual edit even on filesystems with coarse timestamp resolution.
    std::fs::write(&path, "[display]\ncentered = true\n# edited\n").expect("edit config");

    assert!(crate::config::config().display.centered);

    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}

#[test]
fn config_save_invalidates_global_config_cache() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    let mut cfg = Config::default();
    cfg.display.centered = false;
    cfg.save().expect("save initial config");
    assert!(!crate::config::config().display.centered);

    cfg.display.centered = true;
    cfg.save().expect("save updated config");
    assert!(crate::config::config().display.centered);

    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}

#[test]
fn config_env_fingerprint_ignores_runtime_only_jcode_vars() {
    let _guard = crate::storage::lock_test_env();
    let prev_runtime_provider = std::env::var_os("JCODE_RUNTIME_PROVIDER");
    let prev_active_provider = std::env::var_os("JCODE_ACTIVE_PROVIDER");
    let prev_display_centered = std::env::var_os("JCODE_DISPLAY_CENTERED");

    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
    crate::env::remove_var("JCODE_DISPLAY_CENTERED");
    let baseline = config_env_fingerprint();

    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "openai");
    crate::env::set_var("JCODE_ACTIVE_PROVIDER", "openai");
    assert_eq!(baseline, config_env_fingerprint());

    crate::env::set_var("JCODE_DISPLAY_CENTERED", "1");
    assert_ne!(baseline, config_env_fingerprint());

    restore_env_var("JCODE_RUNTIME_PROVIDER", prev_runtime_provider);
    restore_env_var("JCODE_ACTIVE_PROVIDER", prev_active_provider);
    restore_env_var("JCODE_DISPLAY_CENTERED", prev_display_centered);
}

#[test]
fn config_env_fingerprint_tracks_every_apply_env_override_var() {
    let override_source = include_str!("config/env_overrides.rs");
    let mut missing = Vec::new();

    for line in override_source.lines() {
        let Some(start) = line.find("std::env::var(\"") else {
            continue;
        };
        let rest = &line[start + "std::env::var(\"".len()..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let key = &rest[..end];
        if !crate::config::CONFIG_ENV_KEYS.contains(&key) {
            missing.push(key.to_string());
        }
    }

    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "CONFIG_ENV_KEYS must include every env var read by Config::apply_env_overrides; missing: {missing:?}"
    );
}

#[test]
fn cached_external_auth_trust_observes_manual_revocation() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    Config::invalidate_cache();

    let auth_file = dir.path().join("external-auth.json");
    std::fs::write(&auth_file, "{}\n").expect("write external auth file");
    Config::allow_external_auth_source_for_path("test_source", &auth_file)
        .expect("trust external auth path");
    assert!(Config::external_auth_source_allowed_for_path_cached(
        "test_source",
        &auth_file
    ));

    let path = Config::path().expect("config path");
    std::fs::write(
        &path,
        "[auth]\ntrusted_external_source_paths = []\n# manually revoked\n",
    )
    .expect("manually revoke external auth trust");

    assert!(!Config::external_auth_source_allowed_for_path_cached(
        "test_source",
        &auth_file
    ));

    restore_env_var("JCODE_HOME", prev_home);
    Config::invalidate_cache();
}

#[test]
fn test_ambient_visible_defaults_to_true() {
    assert!(AmbientConfig::default().visible);
}

#[test]
fn test_display_auto_server_reload_defaults_to_true() {
    assert!(DisplayConfig::default().auto_server_reload);
}

#[test]
fn test_display_alignment_defaults_to_left() {
    assert!(!DisplayConfig::default().centered);
}

#[test]
fn test_provider_failover_defaults_match_new_behavior() {
    let provider = Config::default().provider;
    assert_eq!(
        provider.cross_provider_failover,
        super::CrossProviderFailoverMode::Countdown
    );
    assert!(provider.same_provider_account_failover);
}

#[test]
fn test_native_scrollbars_default_to_enabled() {
    let display = DisplayConfig::default();
    assert!(display.native_scrollbars.chat);
    assert!(display.native_scrollbars.side_panel);
}

#[test]
fn test_copy_badge_alt_label_defaults_to_auto_and_deserializes() {
    assert!(DisplayConfig::default().copy_badge_alt_label.is_empty());

    let cfg: Config = toml::from_str(
        r#"
        [display]
        copy_badge_alt_label = "Option"
        "#,
    )
    .expect("config should deserialize");

    assert_eq!(cfg.display.copy_badge_alt_label, "Option");
}

#[test]
fn test_session_picker_resume_action_defaults_to_current_terminal() {
    assert_eq!(
        Config::default().keybindings.session_picker_enter,
        SessionPickerResumeAction::CurrentTerminal
    );
    assert_eq!(
        SessionPickerResumeAction::CurrentTerminal.alternate(),
        SessionPickerResumeAction::NewTerminal
    );
}

#[test]
fn test_session_picker_resume_action_deserializes_kebab_case() {
    let cfg: Config = toml::from_str(
        r#"
        [keybindings]
        session_picker_enter = "current-terminal"
        "#,
    )
    .expect("config should deserialize");

    assert_eq!(
        cfg.keybindings.session_picker_enter,
        SessionPickerResumeAction::CurrentTerminal
    );
}

#[test]
fn test_env_override_auto_server_reload() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_AUTO_SERVER_RELOAD");
    crate::env::set_var("JCODE_AUTO_SERVER_RELOAD", "false");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert!(!cfg.display.auto_server_reload);

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_AUTO_SERVER_RELOAD", prev);
    } else {
        crate::env::remove_var("JCODE_AUTO_SERVER_RELOAD");
    }
}

#[test]
fn test_env_override_native_scrollbars() {
    let _guard = crate::storage::lock_test_env();
    let prev_chat = std::env::var_os("JCODE_CHAT_NATIVE_SCROLLBAR");
    let prev_side = std::env::var_os("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR");
    crate::env::set_var("JCODE_CHAT_NATIVE_SCROLLBAR", "true");
    crate::env::set_var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR", "false");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert!(cfg.display.native_scrollbars.chat);
    assert!(!cfg.display.native_scrollbars.side_panel);

    if let Some(prev) = prev_chat {
        crate::env::set_var("JCODE_CHAT_NATIVE_SCROLLBAR", prev);
    } else {
        crate::env::remove_var("JCODE_CHAT_NATIVE_SCROLLBAR");
    }
    if let Some(prev) = prev_side {
        crate::env::set_var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR", prev);
    } else {
        crate::env::remove_var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR");
    }
}

#[test]
fn test_env_override_diff_mode_full_inline() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_DIFF_MODE");
    crate::env::set_var("JCODE_DIFF_MODE", "full-inline");

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.display.diff_mode, DiffDisplayMode::FullInline);

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_DIFF_MODE", prev);
    } else {
        crate::env::remove_var("JCODE_DIFF_MODE");
    }
}

#[test]
fn test_env_override_trusted_external_auth_splits_source_and_path_entries() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
    crate::env::set_var(
        "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES",
        "legacy_source,claude_code_credentials|/tmp/auth.json",
    );

    let mut cfg = Config::default();
    cfg.apply_env_overrides();

    assert_eq!(cfg.auth.trusted_external_sources, vec!["legacy_source"]);
    assert_eq!(
        cfg.auth.trusted_external_source_paths,
        vec!["claude_code_credentials|/tmp/auth.json"]
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", prev);
    } else {
        crate::env::remove_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
    }
}

#[test]
fn test_external_auth_source_allowed_for_path_matches_saved_entry() {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{}\n").expect("write auth file");

    let canonical = std::fs::canonicalize(&path).expect("canonical path");
    let mut cfg = Config::default();
    cfg.auth.trusted_external_source_paths = vec![format!(
        "test_source|{}",
        canonical.to_string_lossy().to_ascii_lowercase()
    )];

    assert!(cfg.external_auth_source_allowed_for_path_config("test_source", &path));
}

#[test]
fn test_external_auth_source_allowed_for_path_ignores_broad_legacy_entry() {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{}\n").expect("write auth file");

    let mut cfg = Config::default();
    cfg.auth.trusted_external_sources = vec!["test_source".to_string()];

    assert!(!cfg.external_auth_source_allowed_for_path_config("test_source", &path));
}

/// Regression test for issue #349: a removed/unknown `update_channel` value
/// (older configs could contain `"manual"`) must not fail the whole config
/// parse. A hard parse failure during the reload handoff left the reload
/// marker stuck in `starting` and clients re-requested the reload forever.
#[test]
fn unknown_update_channel_value_falls_back_to_stable_instead_of_failing_parse() {
    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"manual\"\n")
        .expect("unknown update_channel must not fail config parse");
    assert_eq!(
        cfg.features.update_channel,
        super::UpdateChannel::Stable,
        "unknown channel should fall back to the default"
    );

    // Other settings in the same config must survive the fallback.
    let cfg: Config = toml::from_str(
        "[features]\nupdate_channel = \"manual\"\nmemory = false\n\n[display]\ncentered = true\n",
    )
    .expect("config with unknown update_channel should parse");
    assert_eq!(cfg.features.update_channel, super::UpdateChannel::Stable);
    assert!(!cfg.features.memory);
    assert!(cfg.display.centered);
}

#[test]
fn known_update_channel_values_still_parse() {
    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"main\"\n")
        .expect("main update_channel should parse");
    assert_eq!(cfg.features.update_channel, super::UpdateChannel::Main);

    let cfg: Config = toml::from_str("[features]\nupdate_channel = \"stable\"\n")
        .expect("stable update_channel should parse");
    assert_eq!(cfg.features.update_channel, super::UpdateChannel::Stable);
}

#[test]
fn update_channel_parse_accepts_known_aliases_and_rejects_unknown() {
    use super::UpdateChannel;
    assert_eq!(UpdateChannel::parse("stable"), Some(UpdateChannel::Stable));
    assert_eq!(UpdateChannel::parse("release"), Some(UpdateChannel::Stable));
    assert_eq!(UpdateChannel::parse("main"), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse("nightly"), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse("edge"), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse(" Main "), Some(UpdateChannel::Main));
    assert_eq!(UpdateChannel::parse("manual"), None);
    assert_eq!(UpdateChannel::parse(""), None);
}

impl Config {
    fn external_auth_source_allowed_for_path_config(&self, source_id: &str, path: &Path) -> bool {
        let Ok(entry) = Self::trusted_external_auth_path_entry(source_id, path) else {
            return false;
        };
        self.auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
    }
}

#[test]
fn populate_context_limits_from_config_ref_seeds_global_cache() {
    use super::{NamedProviderConfig, NamedProviderModelConfig};

    // Regression test for issue #366: a named OpenAI-compatible provider with a
    // per-model `context_window` must be honored by the global context-limit
    // resolution path, not just the provider instance's own context_window().
    let model_id = "issue366-custom-gateway-model";
    let mut cfg = Config::default();
    cfg.providers.insert(
        "issue366-gateway".to_string(),
        NamedProviderConfig {
            base_url: "https://gateway.example.test/v1".to_string(),
            models: vec![NamedProviderModelConfig {
                id: model_id.to_string(),
                context_window: Some(1_000_000),
                input: Vec::new(),
            }],
            ..Default::default()
        },
    );

    populate_context_limits_from_config_ref(&cfg);

    assert_eq!(
        crate::provider::context_limit_for_model(model_id),
        Some(1_000_000),
        "global context-limit resolution should respect named provider context_window"
    );
}
