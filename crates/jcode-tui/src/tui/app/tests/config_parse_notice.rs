// A config.toml that stops parsing is answered with defaults by `Config::load`,
// so the only symptom would be every setting quietly reverting. The notice makes
// that visible, is announced once rather than on every tick, and is re-armed
// after a fix so a second break is reported too.

#[test]
fn a_broken_config_is_announced_once_and_re_armed_after_a_fix() {
    with_temp_jcode_home(|| {
        let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
        let path = crate::config::Config::path().expect("config path");
        let broken = "[display\ncentered = true\n";

        std::fs::write(&path, broken).expect("write broken config");
        crate::config::invalidate_config_cache();

        assert!(
            app.refresh_config_parse_notice(),
            "a config that stopped parsing must be announced"
        );
        let last = app
            .display_messages()
            .last()
            .expect("the notice is a display message");
        assert_eq!(last.role, "error");
        assert!(
            last.content.contains("no longer parses as TOML"),
            "the notice must say what happened: {}",
            last.content
        );
        assert!(
            last.content.contains("being ignored"),
            "the notice must say the settings stopped applying: {}",
            last.content
        );
        assert!(
            !app.refresh_config_parse_notice(),
            "the same failure must not be announced again on every tick"
        );

        std::fs::write(&path, "[display]\ncentered = true\n").expect("fix config");
        crate::config::invalidate_config_cache();
        assert!(
            !app.refresh_config_parse_notice(),
            "a fixed config must not produce a notice"
        );

        // Same bytes as the first failure: if the dedupe did not reset on the
        // fix, the identical error text would suppress this second report.
        std::fs::write(&path, broken).expect("break config again");
        crate::config::invalidate_config_cache();
        assert!(
            app.refresh_config_parse_notice(),
            "a config broken again after a fix must be announced again"
        );
    });
}
