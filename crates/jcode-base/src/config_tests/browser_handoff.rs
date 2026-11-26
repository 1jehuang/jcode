#[test]
fn browser_handoff_defaults_off_and_roundtrips() {
    for contents in ["", "[tools]\n"] {
        let config: Config = toml::from_str(contents).unwrap();
        assert!(!config.tools.browser_handoff);
    }
    for enabled in [false, true] {
        let config: Config =
            toml::from_str(&format!("[tools]\nbrowser_handoff = {enabled}\n")).unwrap();
        assert_eq!(config.tools.browser_handoff, enabled);
        let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.tools.browser_handoff, enabled);
    }
}
