use super::*;

#[test]
fn test_remote_known_model_keeps_server_context_after_local_refresh() {
    // A model-family guess must not overwrite the running endpoint's smaller limit.
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("strata".to_string());
    let model = "qwen3.8-flash-next-ud-q4_k_xl-abliterated";
    app.remote_provider_model = Some(model.to_string());

    app.update_context_limit_for_model(model, Some(196_608));
    app.update_context_limit_for_model(model, None);
    assert_eq!(app.context_limit, 196_608);
    assert_eq!(
        app.registry.compaction().try_read().unwrap().token_budget(),
        196_608
    );

    // A server update remains authoritative when switching to another model.
    app.update_context_limit_for_model("another-model", Some(65_536));
    app.remote_provider_model = Some("another-model".to_string());
    app.update_context_limit_for_model("another-model", None);
    assert_eq!(app.context_limit, 65_536);
}

#[test]
fn test_remote_context_window_does_not_follow_a_different_model() {
    // Changing models invalidates the window reported for the previous model.
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("strata".to_string());
    let model = "qwen3.8-flash-next-ud-q4_k_xl-abliterated";
    app.update_context_limit_for_model(model, Some(196_608));

    app.update_context_limit_for_model("gpt-5.4", None);
    assert_eq!(app.context_limit, 1_000_000);
    app.update_context_limit_for_model(model, None);
    assert_eq!(app.context_limit, 262_144);
}

#[test]
fn test_remote_context_window_is_cleared_when_provider_changes() {
    // The same model name can have a different limit on another provider.
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("strata".to_string());
    let model = "qwen3.8-flash-next-ud-q4_k_xl-abliterated";
    app.remote_provider_model = Some(model.to_string());
    app.update_context_limit_for_model(model, Some(196_608));

    app.replace_remote_model_catalog_snapshot(jcode_provider_core::ModelCatalogSnapshot::new(
        Some("other".to_string()),
        Some(model.to_string()),
        Vec::new(),
        Vec::new(),
    ));
    app.update_context_limit_for_model(model, None);
    assert_eq!(app.context_limit, 262_144);
}
