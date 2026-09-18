// `/pricing` must name the `[pricing.providers.<vendor>].file` that priced the
// model.
//
// The report's own text used to list only "models.dev, a provider cache, or the
// fallback estimate" for the no-card case, while the `reference request` figure
// printed in the same report came from a file. This drives the real command so
// the assertion is about what a user sees, not just the renderer's strings.

/// A vendor file body with DeepSeek's model at 3.0/6.0 USD, in the only
/// accepted shape (`models` at the top level, no outer provider key).
const VENDOR_FILE: &str =
    r#"{"models":{"deepseek-v4-pro":{"cost":{"input":3.0,"output":6.0}}}}"#;

#[test]
fn pricing_command_names_the_vendor_file_that_priced_the_model() {
    with_temp_jcode_home(|| {
        let home =
            std::path::PathBuf::from(std::env::var_os("JCODE_HOME").expect("the test home is set"));
        let file_path = home.join("corp-mirror.json");
        std::fs::write(&file_path, VENDOR_FILE).expect("write vendor file");
        std::fs::write(
            home.join("config.toml"),
            format!(
                r#"
[pricing.providers.deepseek]
file = "{}"
currency = "USD"
"#,
                file_path.display()
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
            text.contains("no inline `[pricing.providers]` card prices this model"),
            "with no card, the report starts from the lower layers: {text}"
        );
        assert!(
            text.contains("`[pricing.providers.deepseek]` file"),
            "the vendor file that priced the model must be named: {text}"
        );
        assert!(
            text.contains(&file_path.display().to_string()),
            "the report must name the file path it read: {text}"
        );
    });
}
