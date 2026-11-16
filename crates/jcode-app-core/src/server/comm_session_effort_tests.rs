#[test]
fn swarm_spawn_effort_prefers_explicit_then_config_pin_then_inherit() {
    use super::super::resolve_swarm_spawn_effort;

    // Explicit spawn argument wins over the config pin (#1165).
    assert_eq!(
        resolve_swarm_spawn_effort(Some("low"), Some("medium")),
        Some("low".to_string())
    );
    // A missing or blank spawn argument falls back to `agents.swarm_effort`.
    assert_eq!(
        resolve_swarm_spawn_effort(None, Some("medium")),
        Some("medium".to_string())
    );
    assert_eq!(
        resolve_swarm_spawn_effort(Some("  "), Some(" medium ")),
        Some("medium".to_string())
    );
    // With neither, workers default to low effort.
    assert_eq!(
        resolve_swarm_spawn_effort(None, None),
        Some("low".to_string())
    );
    assert_eq!(
        resolve_swarm_spawn_effort(Some(""), Some("")),
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
