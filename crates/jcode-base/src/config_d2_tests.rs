use super::tests::restore_env_var;
use super::{CONFIG_ENV_KEYS, Config, D2Layout, DiagramConfig, DiagramEngine};

#[test]
fn d2_diagram_documented_configuration_parses_and_round_trips() {
    let docs = include_str!("../../../docs/d2-diagrams.md");
    let example = docs
        .split("```toml\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let config: Config = toml::from_str(example).unwrap();
    assert!(config.features.mermaid);
    assert!(config.features.d2);
    assert_eq!(config.diagrams.preferred_engine, DiagramEngine::D2);
    assert_eq!(config.diagrams.d2_layout, D2Layout::Tala);
    assert_eq!(config.diagrams.d2_binary, "d2");
    assert_eq!(config.diagrams.bounded_d2_timeout_secs(), 10);
    let encoded = toml::to_string(&config).unwrap();
    let decoded: Config = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded.diagrams, config.diagrams);
}

#[test]
fn d2_diagram_configuration_preserves_defaults_and_unrelated_settings() {
    let defaults: Config = toml::from_str("").unwrap();
    assert!(defaults.features.d2);
    assert_eq!(defaults.diagrams, DiagramConfig::default());
    let config: Config = toml::from_str(
        "[features]\nd2 = false\n[display]\ncentered = true\n[diagrams]\npreferred_engine = 'unknown'\nd2_layout = 'unknown'\nd2_binary = '/local/d2'\n",
    ).unwrap();
    assert!(!config.features.d2);
    assert!(config.features.mermaid);
    assert!(config.display.centered);
    assert_eq!(config.diagrams.preferred_engine, DiagramEngine::Mermaid);
    assert_eq!(config.diagrams.d2_layout, D2Layout::Dagre);
    assert_eq!(config.diagrams.d2_binary, "/local/d2");
    let template: Config = toml::from_str(&Config::default_config_file_contents()).unwrap();
    assert_eq!(template.diagrams, DiagramConfig::default());
    assert!(template.features.d2);
}

#[test]
fn d2_diagram_environment_override_uses_standard_boolean_values() {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_ENABLE_D2");
    for (value, expected) in [
        ("off", false),
        ("1", true),
        ("false", false),
        ("invalid", true),
    ] {
        crate::env::set_var("JCODE_ENABLE_D2", value);
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.features.d2, expected);
    }
    restore_env_var("JCODE_ENABLE_D2", previous);
    assert!(CONFIG_ENV_KEYS.contains(&"JCODE_ENABLE_D2"));
}
