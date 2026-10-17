#[test]
fn launch_hotkeys_config_round_trips_toml() {
    use jcode_config_types::{LaunchHotkeyEntry, LaunchHotkeysConfig};
    #[derive(serde::Serialize, serde::Deserialize, Default)]
    #[serde(default)]
    struct W {
        launch_hotkeys: LaunchHotkeysConfig,
    }
    let w = W {
        launch_hotkeys: LaunchHotkeysConfig {
            enabled: Some(true),
            imported: true,
            entries: vec![
                LaunchHotkeyEntry {
                    chord: "cmd+;".into(),
                    dir: "/Users/jeremy/jcode-github".into(),
                    label: "kcode-github".into(),
                    self_dev: false,
                },
                LaunchHotkeyEntry {
                    chord: "cmd+'".into(),
                    dir: "$HOME".into(),
                    label: "home".into(),
                    self_dev: false,
                },
            ],
        },
    };
    let toml = toml::to_string(&w).unwrap();
    let back: W = toml::from_str(&toml).unwrap();
    assert_eq!(back.launch_hotkeys.entries.len(), 2);
    assert_eq!(back.launch_hotkeys.enabled, Some(true));
    // Empty config (no section) -> defaults: no entries, enabled None.
    let empty: W = toml::from_str("").unwrap();
    assert!(empty.launch_hotkeys.entries.is_empty());
    assert_eq!(empty.launch_hotkeys.enabled, None);
    assert!(!empty.launch_hotkeys.imported);
}
