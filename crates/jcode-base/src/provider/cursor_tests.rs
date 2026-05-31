use super::*;

#[test]
fn available_models_include_composer_models() {
    let provider = CursorCliProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"composer-1"));
    assert!(models.contains(&"composer-1.5"));
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
    assert!(models.iter().any(|model| model == "composer-1.5"));
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
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = CursorCliProvider::persisted_catalog_path().expect("catalog path");
    crate::storage::write_json(
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
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn set_model_accepts_composer_models() {
    let provider = CursorCliProvider::new();

    provider.set_model("composer-1").unwrap();
    assert_eq!(provider.model(), "composer-1");

    provider.set_model("composer-1.5").unwrap();
    assert_eq!(provider.model(), "composer-1.5");
}

#[test]
fn runtime_cursor_api_key_reads_env() {
    let previous = std::env::var_os("CURSOR_API_KEY");
    crate::env::set_var("CURSOR_API_KEY", "cursor-env-test");

    assert_eq!(runtime_cursor_api_key().as_deref(), Some("cursor-env-test"));

    if let Some(previous) = previous {
        crate::env::set_var("CURSOR_API_KEY", previous);
    } else {
        crate::env::remove_var("CURSOR_API_KEY");
    }
}

#[test]
fn think_router_splits_reasoning_and_text() {
    let mut router = ThinkRouter::default();
    let events = router.push_chunk("hello<think>secret</think>world");
    assert!(matches!(events[0], StreamEvent::TextDelta(_)));
    assert!(matches!(events[1], StreamEvent::ThinkingStart));
    assert!(matches!(events[2], StreamEvent::ThinkingDelta(_)));
    assert!(matches!(events[3], StreamEvent::ThinkingEnd));
    assert!(matches!(events[4], StreamEvent::TextDelta(_)));
}
