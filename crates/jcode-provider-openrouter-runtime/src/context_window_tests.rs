use crate::tests::{ENV_LOCK, EnvVarGuard};
use crate::*;
use tempfile::TempDir;

const MODEL: &str = "qwen3.8-flash-next-ud-q4_k_xl-abliterated";

fn strata_config(context_window: Option<usize>) -> jcode_base::config::NamedProviderConfig {
    jcode_base::config::NamedProviderConfig {
        base_url: "http://127.0.0.1:1/v1".into(),
        auth: jcode_base::config::NamedProviderAuth::None,
        requires_api_key: Some(false),
        default_model: Some(MODEL.into()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: MODEL.into(),
            context_window,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn catalog(n_ctx: usize) -> Vec<ModelInfo> {
    parse_openai_compatible_models_response(&format!(
        r#"{{"object":"list","data":[{{"id":"{MODEL}","object":"model","meta":{{"n_ctx":{n_ctx}}}}}]}}"#
    ))
    .expect("llama.cpp catalog")
}

#[test]
fn strata_configured_context_beats_live_and_family_on_session_paths() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().unwrap();
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let config = strata_config(Some(196_608));
    let provider = OpenRouterProvider::new_named_openai_compatible("strata", &config).unwrap();
    assert_eq!(provider.context_window(), 196_608);
    // A disagreeing live value makes the config > live precedence observable.
    let models = catalog(262_144);
    save_disk_cache_with_source_for_namespace("strata", &models, Some(&provider.api_base));
    provider.models_cache.blocking_write().models = models;
    assert_eq!(provider.context_window(), 196_608);
    assert_eq!(provider.fork().context_window(), 196_608);
    provider.models_cache.blocking_write().models.clear();
    assert_eq!(provider.context_window(), 196_608, "disk catalog");
    *provider.model.blocking_write() = format!("strata:{MODEL}");
    assert_eq!(provider.context_window(), 196_608);
}

#[test]
fn strata_live_n_ctx_beats_family_without_config() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().unwrap();
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let provider =
        OpenRouterProvider::new_named_openai_compatible("strata", &strata_config(None)).unwrap();
    let models = catalog(196_608);
    assert_eq!(models[0].context_length, Some(196_608));
    // A catalog publication can precede hydration of this instance's cache.
    jcode_base::provider::populate_context_limits(HashMap::from([(
        format!("strata:{MODEL}"),
        models[0].context_length.unwrap() as usize,
    )]));
    assert_eq!(provider.context_window(), 196_608, "shared live cache");
    for id in [MODEL.to_string(), format!("strata:{MODEL}")] {
        assert_eq!(
            jcode_base::provider::context_limit_for_model_with_provider(&id, Some("strata")),
            Some(196_608)
        );
    }
    save_disk_cache_with_source_for_namespace("strata", &models, Some(&provider.api_base));
    provider.models_cache.blocking_write().models = models;
    assert_eq!(provider.context_window(), 196_608);
    assert_eq!(provider.fork().context_window(), 196_608);
    let fresh =
        OpenRouterProvider::new_named_openai_compatible("strata", &strata_config(None)).unwrap();
    assert_eq!(fresh.context_window(), 196_608, "disk catalog");
    *provider.model.blocking_write() = format!("strata:{MODEL}");
    assert_eq!(provider.context_window(), 196_608);
}
