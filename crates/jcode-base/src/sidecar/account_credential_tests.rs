#[test]
fn test_backend_selection_prefers_openai() {
    // Make backend selection deterministic by isolating credentials.
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp jcode home");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

    codex::upsert_account_from_tokens("openai-1", "sk-test-key-123", "", None, None)
        .expect("write OpenAI test auth");
    crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        record_id: None,
        label: "claude-1".to_string(),
        access: "claude-access".to_string(),
        refresh: "claude-refresh".to_string(),
        expires: 4_102_444_800_000,
        email: None,
        scopes: Vec::new(),
        subscription_type: None,
    })
    .expect("write Claude test auth");

    let sidecar = Sidecar::with_configured_model(None);
    assert_eq!(sidecar.backend, SidecarBackend::OpenAI);
    assert_eq!(sidecar.model, SIDECAR_OPENAI_MODEL);
    codex::set_active_account_override(None);
    crate::auth::claude::set_active_account_override(None);
}
