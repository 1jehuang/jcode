use super::*;
use crate::tests::{ENV_LOCK, EnvVarGuard};
use jcode_base::config::{NamedProviderAuth, NamedProviderConfig, NamedProviderModelConfig};

#[test]
fn named_chat_capabilities_reject_generation_requests_and_preserve_future_catalog_models() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let profile = NamedProviderConfig {
        base_url: "http://127.0.0.1:9/v1".into(),
        auth: NamedProviderAuth::None,
        model_catalog: true,
        default_model: Some("future-worker-2099".into()),
        models: vec![
            NamedProviderModelConfig {
                id: "future-worker-2099".into(),
                supports_chat: Some(false),
                ..Default::default()
            },
            NamedProviderModelConfig {
                id: "future-chat-2099".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let provider = OpenRouterProvider::new_named_openai_compatible("example", &profile).unwrap();
    assert!(provider.set_model("FUTURE-WORKER-2099").is_err());
    assert!(provider.fork().set_model("future-worker-2099").is_err());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(provider.complete(&[], &[], "fixture", None))
        .err()
        .expect("generation default must be rejected before sending");
    assert!(error.to_string().contains("chat-capable"));
    *provider.model.blocking_write() = "example:future-worker-2099".into();
    let error = runtime
        .block_on(provider.complete(&[], &[], "fixture", None))
        .err()
        .expect("qualified generation route must be rejected");
    assert!(error.to_string().contains("chat-capable"));
    provider.set_model("future-chat-2099").unwrap();
    {
        let mut cache = provider.models_cache.blocking_write();
        cache.models = parse_openai_compatible_models_response(
            r#"{"data":[{"id":"future-live-chat-2099"},{"id":"future-worker-2099"}]}"#,
        )
        .unwrap();
        cache.fetched = true;
    }
    let models = provider.available_models_display();
    assert!(models.iter().any(|model| model == "future-live-chat-2099"));
    assert!(!models.iter().any(|model| model == "future-worker-2099"));
    provider.set_model("future-live-chat-2099").unwrap();
}
