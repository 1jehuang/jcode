use super::ModelRoute;
use super::catalog_routes::{
    append_cursor_acp_routes, multiprovider_model_routes, remote_model_routes_fallback,
    remote_model_routes_lightweight_fallback, simplified_model_routes_for_picker,
};
use super::external::CURSOR_ACP_RUNTIME;

/// Cursor ACP advertises foundation models from multiple vendors under one
/// provider label. The names-only remote fallback must label every advertised
/// model as a `Cursor ACP` route instead of reclassifying `claude-*` /
/// `gpt-*` / `gemini-*` ids into Anthropic / OpenAI / Gemini routes.
#[test]
fn remote_fallback_labels_cursor_acp_models_under_cursor_acp() {
    let _guard = crate::storage::lock_test_env();

    let models = vec![
        "claude-opus-5[thinking=true,budget_tokens=10000]".to_string(),
        "gpt-5.3-codex[]".to_string(),
        "gemini-3.1-pro[]".to_string(),
        "grok-4.5[...]".to_string(),
        "default[]".to_string(),
    ];

    let routes = remote_model_routes_fallback(Some(CURSOR_ACP_RUNTIME), &models);

    assert_eq!(
        routes.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(),
        models.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        "all advertised models should appear"
    );
    assert!(
        routes.iter().all(|r| r.provider == "Cursor ACP"
            && r.api_method == CURSOR_ACP_RUNTIME
            && r.available),
        "every route should be labeled Cursor ACP, not reclassified by prefix"
    );
    assert!(
        !routes
            .iter()
            .any(|r| r.provider == "Anthropic" || r.provider == "OpenAI" || r.provider == "Gemini"),
        "no model should be reclassified into a local credential route"
    );
}

/// The lightweight fallback (used for large names-only catalogs) must also
/// use the `Cursor ACP` display label instead of the raw `cursor-acp` key.
#[test]
fn remote_lightweight_fallback_uses_cursor_acp_display_label() {
    let _guard = crate::storage::lock_test_env();

    let models = vec![
        "claude-opus-5[thinking=true]".to_string(),
        "gpt-5.3-codex[]".to_string(),
    ];

    let routes = remote_model_routes_lightweight_fallback(
        Some(CURSOR_ACP_RUNTIME),
        &models,
        "claude-opus-5[thinking=true]",
    );

    assert!(routes.iter().all(|r| {
        r.provider == "Cursor ACP" && r.api_method == CURSOR_ACP_RUNTIME && r.available
    }));
}

/// The simplified model picker (TUI `/model` fast snapshot) must also label
/// every Cursor ACP model under the `Cursor ACP` provider instead of
/// reclassifying them by prefix into Anthropic/OpenAI/Gemini routes.
#[test]
fn simplified_picker_labels_cursor_acp_models_under_cursor_acp() {
    let _guard = crate::storage::lock_test_env();

    let models = vec![
        "claude-opus-5[thinking=true,budget_tokens=10000]".to_string(),
        "gpt-5.3-codex[]".to_string(),
        "gemini-3.1-pro[]".to_string(),
        "grok-4.5[...]".to_string(),
        "default[]".to_string(),
    ];

    let routes = simplified_model_routes_for_picker(
        CURSOR_ACP_RUNTIME,
        "claude-opus-5[thinking=true]",
        models.clone(),
    );

    assert_eq!(
        routes.iter().map(|r| r.model.as_str()).collect::<Vec<_>>(),
        models.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        "all advertised models should appear"
    );
    assert!(
        routes.iter().all(|r| r.provider == "Cursor ACP"
            && r.api_method == CURSOR_ACP_RUNTIME
            && r.available),
        "every route should be labeled Cursor ACP, not reclassified by prefix"
    );
    assert!(
        !routes
            .iter()
            .any(|r| r.provider == "Anthropic" || r.provider == "OpenAI" || r.provider == "Gemini"),
        "no model should be reclassified into a local credential route"
    );
}

/// When Cursor ACP has not yet discovered any models, the simplified picker
/// should still show the current model as a `Cursor ACP` route (not a generic
/// `"current"` route) so it matches the CLI's `model_routes()` output.
#[test]
fn simplified_picker_cursor_acp_empty_models_falls_back_to_current_model() {
    let _guard = crate::storage::lock_test_env();

    let routes = simplified_model_routes_for_picker(
        CURSOR_ACP_RUNTIME,
        "claude-opus-5[thinking=true]",
        Vec::<String>::new(),
    );

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].model, "claude-opus-5[thinking=true]");
    assert_eq!(routes[0].provider, "Cursor ACP");
    assert_eq!(routes[0].api_method, CURSOR_ACP_RUNTIME);
    assert!(routes[0].available);
}

/// When the `cursor_acp` pool slot is populated, the TUI `/model` picker
/// (`multiprovider_model_routes`) must surface Cursor ACP routes regardless of
/// the active `ActiveProvider`. This reproduces the `--provider auto` scenario
/// where Cursor ACP previously never appeared because the slot was missing.
#[test]
fn multiprovider_model_routes_includes_cursor_acp_when_slot_is_populated() {
    with_clean_provider_test_env(|| {
        // The populated-slot test uses test_multi_provider_with_cursor_acp
        // (openai=None, anthropic=None), so multiprovider_model_routes does
        // not spawn any background catalog-refresh tasks and needs no runtime
        // guard. Use OpenAI as the active provider to simulate --provider auto
        // (or any non-cursor-acp active provider). Cursor ACP is selected via
        // the external active-provider mechanism, not via ActiveProvider.
        let provider = test_multi_provider_with_cursor_acp();
        let routes = multiprovider_model_routes(&provider);

        let cursor_acp_routes: Vec<&ModelRoute> = routes
            .iter()
            .filter(|r| r.provider == "Cursor ACP" && r.api_method == "cursor-acp")
            .collect();

        assert!(
            !cursor_acp_routes.is_empty(),
            "multiprovider_model_routes should include Cursor ACP routes when the slot is populated, got: {routes:?}"
        );
        assert!(
            cursor_acp_routes.iter().all(|r| r.available),
            "all Cursor ACP routes should be available"
        );
    });
}

/// When the `cursor_acp` pool slot is empty, no Cursor ACP routes should leak
/// into the picker output.
#[test]
fn multiprovider_model_routes_has_no_cursor_acp_when_slot_is_empty() {
    with_clean_provider_test_env(|| {
        // The empty-slot test uses test_multi_provider_with_openai (openai=Some),
        // so multiprovider_model_routes may spawn a background OpenAI catalog
        // refresh via tokio::spawn. Enter a runtime context so the spawn does
        // not panic.
        let runtime = enter_test_runtime();
        let _enter = runtime.enter();

        let provider = test_multi_provider_with_openai();
        let routes = multiprovider_model_routes(&provider);

        assert!(
            routes
                .iter()
                .all(|r| !(r.provider == "Cursor ACP" && r.api_method == "cursor-acp")),
            "no Cursor ACP routes expected when slot is empty, got: {routes:?}"
        );
    });
}

/// `cursor_acp_provider()` returns the registered Cursor ACP runtime when the
/// `cursor_acp` slot is populated.
#[test]
fn cursor_acp_provider_returns_runtime_when_slot_is_populated() {
    let provider = test_multi_provider_with_cursor_acp();
    let cursor_acp = provider
        .cursor_acp_provider()
        .expect("cursor-acp slot should be populated");
    assert_eq!(cursor_acp.name(), "cursor-acp");
}

/// `cursor_acp_provider()` returns `None` when no Cursor ACP runtime is
/// registered (the `cursor_acp` slot is empty).
#[test]
fn cursor_acp_provider_is_none_when_slot_is_empty() {
    let provider = test_multi_provider_with_cursor();
    assert!(
        provider.cursor_acp_provider().is_none(),
        "cursor-acp slot should be empty"
    );
}

/// `set_active_provider_external` stores the external runtime key and
/// `active_external_provider` round-trips it back.
#[test]
fn set_active_provider_external_round_trips_via_active_external_provider() {
    let provider = test_multi_provider_with_cursor_acp();
    provider.set_active_provider_external("cursor-acp");
    assert_eq!(
        provider.active_external_provider().as_deref(),
        Some("cursor-acp")
    );
}

/// `active_external_provider()` returns `None` when no external runtime has
/// been activated.
#[test]
fn active_external_provider_is_none_without_setting() {
    let provider = test_multi_provider_with_cursor_acp();
    assert_eq!(provider.active_external_provider(), None);
}

/// `append_cursor_acp_routes` appends the Cursor ACP model routes when the
/// `cursor_acp` slot is populated.
#[test]
fn append_cursor_acp_routes_appends_when_slot_is_populated() {
    let provider = test_multi_provider_with_cursor_acp();
    let mut routes = Vec::new();
    append_cursor_acp_routes(&provider, &mut routes);
    assert!(!routes.is_empty(), "cursor-acp routes should be appended");
    assert!(
        routes
            .iter()
            .all(|r| { r.provider == "Cursor ACP" && r.api_method == "cursor-acp" && r.available })
    );
    // The stub advertises two models.
    assert_eq!(routes.len(), 2);
}

/// `append_cursor_acp_routes` is a no-op when no Cursor ACP runtime is
/// registered (no routes are appended to the supplied vec).
#[test]
fn append_cursor_acp_routes_is_noop_when_slot_is_empty() {
    let provider = test_multi_provider_with_cursor();
    let mut routes = Vec::new();
    append_cursor_acp_routes(&provider, &mut routes);
    assert!(
        routes.is_empty(),
        "no cursor-acp routes should be appended when the slot is empty, got: {routes:?}"
    );
}

/// `spawn_cursor_acp_catalog_refresh_if_needed` bails out before spawning when
/// no Cursor ACP runtime is registered, so it must not panic outside a tokio
/// runtime context. This is the only deterministic path: the populated-provider
/// path calls `tokio::spawn` on a fire-and-forget task guarded by a
/// process-global in-flight atomic (`CURSOR_ACP_REFRESH_IN_FLIGHT`) that cannot
/// be reset from tests, and observes completion only after a live `prefetch`
/// (which spawns the real `agent acp` subprocess). That path is therefore not
/// unit-testable without a subprocess and is intentionally skipped here.
#[test]
fn spawn_cursor_acp_catalog_refresh_is_noop_when_slot_is_empty() {
    let provider = test_multi_provider_with_cursor();
    // Must return early without touching the in-flight guard or calling
    // tokio::spawn (which would panic without a runtime context).
    provider.spawn_cursor_acp_catalog_refresh_if_needed();
}

/// Saving a Cursor ACP route as the default (Ctrl+O in the picker) must keep
/// the cursor-acp provider key so the next startup re-instantiates the ACP
/// runtime instead of falling back to a bare model name routed through Auto
/// (previously OpenAI). Regression guard for the Greptile finding: the
/// selection used to drop the provider for `cursor-acp` routes because
/// `default_model_selection_from_route` had no `CursorAcp` arm.
#[test]
fn default_model_selection_from_route_preserves_cursor_acp_route() {
    let selection = crate::provider::MultiProvider::default_model_selection_from_route(
        "gpt-5.3-codex[]",
        CURSOR_ACP_RUNTIME,
        "Cursor ACP",
    );
    assert_eq!(
        selection.provider_key.as_deref(),
        Some("cursor-acp"),
        "Ctrl+O on a Cursor ACP route must persist the cursor-acp provider key"
    );
    assert_eq!(
        selection.model_spec, "gpt-5.3-codex[]",
        "the ACP runtime consumes bare advertised ids; no prefix is needed"
    );
}
