use super::*;

#[test]
fn available_models_include_composer_models() {
    let provider = CursorCliProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"composer-2"));
    assert!(models.contains(&"composer-2.5"));
    assert!(models.contains(&"grok-4.6"));
    assert!(models.contains(&"kimi-k3"));
}

#[test]
fn decode_agent_models_reads_raw_and_connect_framed_payloads() {
    let model = |id: &str| {
        let mut nested = Vec::new();
        nested.extend([0x0a, id.len() as u8]);
        nested.extend(id.as_bytes());
        let mut entry = vec![0x0a, nested.len() as u8];
        entry.extend(nested);
        entry
    };
    let mut payload = model("grok-4.6");
    payload.extend(model("kimi-k3"));

    assert_eq!(
        decode_agent_models(&payload).unwrap(),
        vec!["grok-4.6", "kimi-k3"]
    );

    let mut framed = vec![0];
    framed.extend((payload.len() as u32).to_be_bytes());
    framed.extend(payload);
    assert_eq!(
        decode_agent_models(&framed).unwrap(),
        vec!["grok-4.6", "kimi-k3"]
    );
}

#[test]
fn decode_agent_models_rejects_truncated_payloads() {
    assert!(decode_agent_models(&[0x0a, 0x05, 0x0a]).is_err());
    assert!(decode_agent_models(&[0x80]).is_err());
}

#[test]
fn available_models_display_includes_custom_current_model() {
    let provider = CursorCliProvider::new();
    provider.set_model("future-cursor-model").unwrap();

    let models = provider.available_models_display();
    assert!(models.contains(&"future-cursor-model".to_string()));
}

#[test]
fn available_models_display_prefers_fetched_cursor_models() {
    let provider = CursorCliProvider::new();
    *provider.fetched_models.write().unwrap() = vec![
        "claude-4-sonnet-thinking".to_string(),
        "gpt-5.2".to_string(),
    ];

    let models = provider.available_models_display();
    assert_eq!(
        models.first().map(|model| model.as_str()),
        Some("claude-4-sonnet-thinking")
    );
    assert!(models.iter().any(|model| model == "gpt-5.2"));
    assert!(models.iter().any(|model| model == "composer-2.5"));
}

#[test]
fn merge_cursor_models_deduplicates_dynamic_entries() {
    let models = merge_cursor_models(
        &[
            "composer-2".to_string(),
            "claude-4-sonnet-thinking".to_string(),
            "claude-4-sonnet-thinking".to_string(),
        ],
        "claude-4-sonnet-thinking",
    );

    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "claude-4-sonnet-thinking")
            .count(),
        1
    );
    assert!(models.iter().any(|model| model == "composer-2"));
}

#[test]
fn available_models_display_seeds_from_persisted_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    jcode_base::env::set_var("JCODE_HOME", temp.path());

    let path = CursorCliProvider::persisted_catalog_path().expect("catalog path");
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec!["cursor-disk-model".to_string()],
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("write persisted catalog");

    let provider = CursorCliProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"cursor-disk-model".to_string())
    );

    if let Some(prev_home) = prev_home {
        jcode_base::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_base::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn set_model_accepts_composer_models() {
    let provider = CursorCliProvider::new();

    provider.set_model("composer-2").unwrap();
    assert_eq!(provider.model(), "composer-2");

    provider.set_model("composer-2.5").unwrap();
    assert_eq!(provider.model(), "composer-2.5");
}

#[test]
fn runtime_cursor_api_key_reads_env() {
    let previous = std::env::var_os("CURSOR_API_KEY");
    jcode_base::env::set_var("CURSOR_API_KEY", "cursor-env-test");

    assert_eq!(runtime_cursor_api_key().as_deref(), Some("cursor-env-test"));

    if let Some(previous) = previous {
        jcode_base::env::set_var("CURSOR_API_KEY", previous);
    } else {
        jcode_base::env::remove_var("CURSOR_API_KEY");
    }
}
