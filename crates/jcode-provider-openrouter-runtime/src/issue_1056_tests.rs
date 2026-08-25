use super::OpenRouterProvider;
use jcode_provider_core::Provider;

#[test]
fn mistral_models_use_their_configured_reasoning_defaults() {
    let profile: jcode_base::config::NamedProviderConfig = toml::from_str(
        r#"
type = "openai-compatible"
base_url = "https://api.mistral.ai/v1"
auth = "Bearer"
api_key = "test"
disable_reasoning_heuristics = true

[[models]]
id = "mistral-small-latest"
reasoning = true
reasoning_effort = "high"

[[models]]
id = "mistral-medium-latest"
reasoning = true
reasoning_effort = "max"
"#,
    )
    .expect("issue 1056 Mistral profile parses");

    let provider = OpenRouterProvider::new_named_openai_compatible("mistral", &profile)
        .expect("Mistral provider constructs");
    provider.set_model("mistral-small-latest").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));
    provider.set_model("mistral-medium-latest").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
}
