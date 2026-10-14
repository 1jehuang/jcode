use jcode_base::config::Config;

#[test]
fn menubar_is_enabled_by_default_and_can_be_disabled() {
    assert!(Config::default().menubar.enabled);

    let empty: Config = toml::from_str("").unwrap();
    assert!(empty.menubar.enabled);

    let disabled: Config = toml::from_str("[menubar]\nenabled = false\n").unwrap();
    assert!(!disabled.menubar.enabled);
}

#[test]
fn shipped_default_config_file_keeps_menubar_enabled() {
    let contents = Config::default_config_file_contents();
    assert!(contents.contains("[menubar]"));
    let parsed: Config = toml::from_str(&contents).unwrap();
    assert!(parsed.menubar.enabled);
}
