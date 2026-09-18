// `/pricing` must name the `[[pricing.sources]]` sheet that priced the model.
//
// The report's own text used to list only "models.dev, a provider cache, or the
// fallback estimate" for the no-card case, while the `reference request` figure
// printed in the same report came from a sheet. This drives the real command so
// the assertion is about what a user sees, not just the renderer's strings.

/// A sheet body in models.dev's shape, with DeepSeek's model at 3.0/6.0 USD.
const SHEET: &str =
    r#"{"deepseek":{"models":{"deepseek-v4-pro":{"cost":{"input":3.0,"output":6.0}}}}}"#;

#[test]
fn pricing_command_names_the_sheet_that_priced_the_model() {
    with_temp_jcode_home(|| {
        let home =
            std::path::PathBuf::from(std::env::var_os("JCODE_HOME").expect("the test home is set"));
        let sheet_path = home.join("corp-mirror.json");
        std::fs::write(&sheet_path, SHEET).expect("write sheet");
        std::fs::write(
            home.join("config.toml"),
            format!(
                r#"
[[pricing.sources]]
id = "corp-mirror"
file = "{}"
"#,
                sheet_path.display()
            ),
        )
        .expect("write config.toml");
        crate::config::invalidate_config_cache();

        let mut app = create_named_provider_test_app("deepseek", "deepseek-v4-pro");
        app.is_remote = true;
        app.remote_provider_name = Some("DeepSeek".to_string());
        app.remote_provider_model = Some("deepseek-v4-pro".to_string());
        app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::ApiKey);

        assert!(
            super::commands_pricing::handle_pricing_command(&mut app, "/pricing"),
            "`/pricing` must be handled"
        );
        let text = app
            .display_messages()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<String>();

        assert!(
            text.contains("no `[pricing]` rule prices this model"),
            "with no card, the report starts from the lower layers: {text}"
        );
        assert!(
            text.contains("[[pricing.sources]] sheet `corp-mirror` (USD)"),
            "the sheet that priced the model must be named: {text}"
        );
    });
}
