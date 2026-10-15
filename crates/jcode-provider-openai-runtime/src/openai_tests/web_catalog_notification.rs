// Sequence native HTTP completion before delayed offline browser discovery.
#[tokio::test]
async fn delayed_browser_discovery_publishes_update_and_invalidates_warm_routes() {
    use jcode_base::bus::{Bus, BusEvent};
    use jcode_base::provider::MultiProvider;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _env = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _user_home = EnvVarGuard::set_path("HOME", temp.path());
    let _config_home = EnvVarGuard::set_path("XDG_CONFIG_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let _mode = EnvVarGuard::remove("JCODE_OPENAI_CREDENTIAL_MODE");
    let _runtime = EnvVarGuard::remove("JCODE_RUNTIME_PROVIDER");
    let _key = EnvVarGuard::remove("OPENAI_API_KEY");
    let _anthropic = EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let _openrouter = EnvVarGuard::remove("OPENROUTER_API_KEY");
    jcode_base::config::Config::invalidate_cache();
    jcode_base::bus::reset_models_updated_publish_state_for_tests();
    let bus = Bus::global();
    let mut updates = bus.subscribe();
    let provider = OpenAIProvider::new(api_key_creds("sk-delayed-browser-test"));
    let routes = MultiProvider::from_auth_status(Default::default());
    let has_web = |items: Vec<jcode_base::provider::ModelRoute>| {
        items.iter().any(|route| route.model == "gpt-6[web]" && route.available)
    };
    assert!(!has_web(routes.model_routes()));

    let (release_browser, browser_released) = tokio::sync::oneshot::channel();
    let (browser_saved, saved) = tokio::sync::oneshot::channel();
    assert!(!provider.prefetch_browser_models_with(async move {
        browser_released.await?;
        let models = jcode_base::provider::chatgpt_web::refresh_models_from_discovery_for_tests(
            serde_json::json!({"models":[{"label":"GPT-6","available":true,"detail":"observed fixture","reasoning_options":[{"value":"medium","label":"Medium","available":true}]}]}),
        ).await?;
        browser_saved.send(()).unwrap();
        Ok(models)
    }).await.unwrap());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let _api_base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &base);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        let n = stream.read(&mut bytes).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes[..n]).starts_with("GET /v1/models "));
        let body = r#"{"data":[{"id":"gpt-9.9-before-browser"}]}"#;
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    // The already-running optional refresh coalesces the public call's browser
    // future. Only the real native HTTP request can complete before release.
    tokio::time::timeout(Duration::from_secs(2), provider.prefetch_models()).await.unwrap().unwrap();
    server.await.unwrap();
    assert!(jcode_base::provider::cached_openai_model_ids_for_scope(&provider.catalog_scope()).unwrap().contains(&"gpt-9.9-before-browser".to_string()));
    assert!(jcode_base::provider::chatgpt_web::cached_models().is_empty());
    // Consume the real credential catalog event and warm its resulting route
    // memo before browser discovery is released.
    assert!(matches!(updates.recv().await.unwrap(), BusEvent::ModelsUpdated));
    assert!(!has_web(routes.model_routes()));

    release_browser.send(()).unwrap();
    saved.await.unwrap();
    assert!(jcode_base::provider::chatgpt_web::find_model("gpt-6[web]").unwrap().available);
    let persisted: serde_json::Value = jcode_base::storage::read_json(&temp.path().join("chatgpt_web_model_catalog_cache.json")).unwrap();
    assert_eq!(persisted["models"][0]["model"], "gpt-6[web]");
    let event = tokio::time::timeout(Duration::from_secs(2), updates.recv()).await;
    let notified = matches!(event, Ok(Ok(BusEvent::ModelsUpdated)));
    let fresh_route = has_web(routes.model_routes());
    assert!(notified && fresh_route, "delayed discovery notified={notified}, fresh available route={fresh_route}");
}
