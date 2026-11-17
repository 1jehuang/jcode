// Tests for the persisted pinned todo-header visibility setting.

use crate::config::{DisplayConfig, TodoHeaderMode};

#[test]
fn todo_header_config_round_trips_all_modes() {
    for (raw, mode) in [
        ("full", TodoHeaderMode::Full),
        ("minimal", TodoHeaderMode::Minimal),
        ("hidden", TodoHeaderMode::Hidden),
    ] {
        let config: DisplayConfig = serde_json::from_value(serde_json::json!({
            "todo_header": raw
        }))
        .expect("display config");
        assert_eq!(config.todo_header, mode);
        assert_eq!(config.todo_header_mode(), mode);

        let roundtrip: DisplayConfig =
            serde_json::from_str(&serde_json::to_string(&config).expect("serialize"))
                .expect("deserialize roundtrip");
        assert_eq!(roundtrip.todo_header, mode);
        assert_eq!(roundtrip.todo_header_mode(), mode);
    }
}

#[test]
fn todo_header_defaults_to_full_and_legacy_pin_off_means_hidden() {
    let missing: DisplayConfig = serde_json::from_str("{}").expect("display config");
    assert_eq!(missing.todo_header, TodoHeaderMode::Full);
    assert_eq!(missing.todo_header_mode(), TodoHeaderMode::Full);

    let legacy_off: DisplayConfig =
        serde_json::from_str(r#"{"pin_todos":false}"#).expect("legacy display config");
    assert_eq!(legacy_off.todo_header_mode(), TodoHeaderMode::Hidden);
}

#[test]
fn todo_header_cycle_advances_full_minimal_hidden_full() {
    assert_eq!(TodoHeaderMode::Full.cycle(), TodoHeaderMode::Minimal);
    assert_eq!(TodoHeaderMode::Minimal.cycle(), TodoHeaderMode::Hidden);
    assert_eq!(TodoHeaderMode::Hidden.cycle(), TodoHeaderMode::Full);

    let mut config = DisplayConfig::default();
    let mut seen = Vec::new();
    for _ in 0..4 {
        config.set_todo_header_mode(config.todo_header_mode().cycle());
        seen.push(config.todo_header_mode());
    }
    assert_eq!(
        seen,
        vec![
            TodoHeaderMode::Minimal,
            TodoHeaderMode::Hidden,
            TodoHeaderMode::Full,
            TodoHeaderMode::Minimal,
        ]
    );
    // The legacy flag follows the resolved mode so old front-ends agree.
    config.set_todo_header_mode(TodoHeaderMode::Hidden);
    assert!(!config.pin_todos);
    config.set_todo_header_mode(TodoHeaderMode::Minimal);
    assert!(config.pin_todos);
}
