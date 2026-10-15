// Public prefetch workflow, real session lock and local HTTP, never a live browser.

async fn credential_catalog_finishes_while_browser_turn_holds_lock(oauth: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _env = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let _mode = EnvVarGuard::remove("JCODE_OPENAI_CREDENTIAL_MODE");
    jcode_base::auth::codex::set_active_account_override(None);
    let session = jcode_base::provider::chatgpt_web::lock_session().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let _api_base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &base);
    let _oauth_endpoint = jcode_base::provider::models::openai_catalog_test_endpoint(format!("{base}/models")).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let size = stream.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..size]);
        assert!(request.starts_with("GET /v1/models "), "{request}");
        let body = r#"{"data":[{"id":"gpt-9.9-review-test"}]}"#;
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    let provider = OpenAIProvider::new(if oauth { oauth_creds() } else { api_key_creds("sk-lock-test") });
    *provider.model.try_write().unwrap() = "gpt-5.4".to_string();
    let scope = provider.catalog_scope();
    tokio::time::timeout(Duration::from_secs(2), provider.prefetch_models()).await
        .expect("credential prefetch must finish before releasing the browser turn lock").unwrap();
    server.await.unwrap();
    let persisted: serde_json::Value = jcode_base::storage::read_json(&jcode_base::storage::app_config_dir().unwrap().join("openai_model_catalog_cache.json")).unwrap();
    assert_eq!(persisted["scopes"][&scope]["models"], serde_json::json!(["gpt-9.9-review-test"]));
    assert!(jcode_base::provider::cached_openai_model_ids_for_scope(&scope).unwrap().contains(&"gpt-9.9-review-test".to_string()));
    // Optional discovery is still blocked on this guard. No further await after
    // releasing it, so test-runtime shutdown cancels discovery before bridge I/O.
    drop(session);
}

#[tokio::test]
async fn api_catalog_finishes_before_browser_session_lock_is_released() {
    credential_catalog_finishes_while_browser_turn_holds_lock(false).await;
}

#[tokio::test]
async fn oauth_catalog_finishes_before_browser_session_lock_is_released() {
    credential_catalog_finishes_while_browser_turn_holds_lock(true).await;
}

#[test]
fn explicit_legacy_browser_selection_keeps_sol_max_in_either_catalog_order() {
    let _guard = jcode_base::storage::lock_test_env();
    let _model = EnvVarGuard::set("JCODE_OPENAI_MODEL", CHATGPT_WEB_MODEL);
    for sol_first in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let mut models = vec![
            serde_json::json!({"model":"gpt-6[web]","label":"GPT-6","available":true,"detail":"fixture","expected_upstream_models":[]}),
            serde_json::json!({"model":"gpt-5.6-sol[web]","label":"GPT-5.6 Sol","available":true,"detail":"fixture","expected_upstream_models":[]}),
        ];
        if sol_first { models.reverse(); }
        jcode_base::storage::write_json(&temp.path().join("chatgpt_web_model_catalog_cache.json"), &serde_json::json!({"version":1,"observed_at":1,"models":models,"reasoning":{}})).unwrap();
        let provider = OpenAIProvider::new_browser_only();
        provider.apply_browser_catalog_refresh(Ok(jcode_base::provider::chatgpt_web::cached_models())).unwrap();
        assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
        assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
        assert_eq!(provider.available_efforts(), ["max","swarm","swarm-deep"]);
        assert_eq!(jcode_base::provider::chatgpt_web::legacy_web_alias(&provider.model()), Some(("gpt-5.6-sol[web]","max")));
        assert_eq!(provider.fork().reasoning_effort().as_deref(), Some("max"));
    }
}

#[test]
fn manual_legacy_selection_is_not_an_unset_browser_default() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::remove("JCODE_OPENAI_MODEL");
    let provider = OpenAIProvider::new_browser_only();
    provider.set_model(CHATGPT_WEB_MODEL).unwrap();
    seed_browser_catalog(temp.path(), "gpt-6[web]", "GPT-6");
    provider.apply_browser_catalog_refresh(Ok(jcode_base::provider::chatgpt_web::cached_models())).unwrap();
    assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
}

#[test]
fn explicit_legacy_model_is_not_replaced_when_its_family_is_absent() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _model = EnvVarGuard::set("JCODE_OPENAI_MODEL", CHATGPT_WEB_MODEL);
    let provider = OpenAIProvider::new_browser_only();
    provider.apply_browser_catalog_refresh(Ok(Vec::new())).unwrap();
    assert_eq!(provider.model(), CHATGPT_WEB_MODEL);
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
    assert!(jcode_base::provider::chatgpt_web::find_model("gpt-5.6-sol[web]").is_none());
}
