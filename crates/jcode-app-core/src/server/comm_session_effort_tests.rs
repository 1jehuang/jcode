#[test]
fn swarm_spawn_effort_prefers_explicit_then_config_pin_then_root_inherit() {
    use super::super::resolve_swarm_spawn_effort;

    // Explicit spawn argument wins over the config pin (#1165).
    assert_eq!(
        resolve_swarm_spawn_effort(Some("low"), Some("medium"), None, Some("high")),
        Some("low".to_string())
    );
    // A missing or blank spawn argument falls back to `agents.swarm_effort`.
    assert_eq!(
        resolve_swarm_spawn_effort(None, Some("medium"), None, Some("high")),
        Some("medium".to_string())
    );
    assert_eq!(
        resolve_swarm_spawn_effort(Some("  "), Some(" medium "), None, None),
        Some("medium".to_string())
    );
    // Config pin wins over root sentinel inheritance.
    assert_eq!(
        resolve_swarm_spawn_effort(None, Some("high"), Some("swarm"), Some("none")),
        Some("high".to_string())
    );
    // When the root carries the `swarm` sentinel and no config pin is set,
    // workers inherit the configured `swarm_root_effort` value.
    // Key path: a coordinator in swarm mode with `swarm_root_effort = "none"`
    // spawns workers at effort "none".
    assert_eq!(
        resolve_swarm_spawn_effort(None, None, Some("swarm"), Some("none")),
        Some("none".to_string())
    );
    // A non-default "medium" root_swarm_effort propagates correctly.
    assert_eq!(
        resolve_swarm_spawn_effort(None, None, Some("swarm"), Some("medium")),
        Some("medium".to_string())
    );
    // Blank/whitespace root_swarm_effort falls through to the default.
    assert_eq!(
        resolve_swarm_spawn_effort(None, None, Some("swarm"), Some("  ")),
        Some("low".to_string())
    );
    // Non-swarm root effort is ignored (only the sentinel triggers inherit).
    assert_eq!(
        resolve_swarm_spawn_effort(None, None, Some("medium"), Some("none")),
        Some("low".to_string())
    );
    // With neither, workers default to low effort.
    assert_eq!(
        resolve_swarm_spawn_effort(None, None, None, None),
        Some("low".to_string())
    );
    assert_eq!(
        resolve_swarm_spawn_effort(Some(""), Some(""), None, None),
        Some("low".to_string())
    );
}

#[test]
fn per_call_auto_spawn_mode_defers_to_configured_mode() {
    use super::super::resolve_swarm_spawn_mode;
    use crate::config::SwarmSpawnMode::{Auto, Headless, Inline, Visible};

    // A model passing `auto` must not override an inline config with a
    // visible-terminal attempt (workers opened outside VS Code).
    assert_eq!(resolve_swarm_spawn_mode(Some(Auto), Inline), Inline);
    assert_eq!(resolve_swarm_spawn_mode(Some(Auto), Headless), Headless);
    assert_eq!(resolve_swarm_spawn_mode(None, Inline), Inline);
    // A configured `auto` keeps its visible-then-headless behavior.
    assert_eq!(resolve_swarm_spawn_mode(Some(Auto), Auto), Auto);
    assert_eq!(resolve_swarm_spawn_mode(None, Auto), Auto);
    // Explicit concrete modes still win over config.
    assert_eq!(resolve_swarm_spawn_mode(Some(Visible), Inline), Visible);
    assert_eq!(resolve_swarm_spawn_mode(Some(Headless), Visible), Headless);
    assert_eq!(resolve_swarm_spawn_mode(Some(Inline), Visible), Inline);
}
