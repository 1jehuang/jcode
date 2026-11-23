use super::*;

#[test]
fn test_remote_context_limit_survives_a_later_local_re_derivation() {
    // The server resolves the window for a model chosen server-side and sends
    // it on attach. The startup and model-switch paths then call
    // update_context_limit_for_model with None, which for a model absent from
    // the static catalog fell through to the client's inert provider and the
    // generic 200_000 default, overwriting a correct 1M value. Measured on
    // stealth/space-bunny-alpha@Stealth: the panel showed 200000 while the
    // route carried 1000000.
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("OpenRouter".to_string());
    app.remote_provider_model = Some("stealth/space-bunny-alpha@Stealth".to_string());

    app.update_context_limit_for_model("stealth/space-bunny-alpha@Stealth", Some(1_000_000));
    assert_eq!(
        app.context_limit, 1_000_000,
        "server window must be adopted"
    );

    // A later local re-derivation must not undo it.
    app.update_context_limit_for_model("stealth/space-bunny-alpha@Stealth", None);
    assert_eq!(
        app.context_limit, 1_000_000,
        "a None call must not downgrade the window the server already reported"
    );

    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(data.context_limit, Some(1_000_000));

    // The compaction budget has to move with it, or a 1M route keeps triggering
    // compaction about five times too early.
    let compaction = app.registry.compaction();
    if let Ok(manager) = compaction.try_read() {
        assert_eq!(manager.token_budget(), 1_000_000);
    }
}

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
