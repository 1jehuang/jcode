use super::*;

#[tokio::test]
async fn workspace_identity_pin_keeps_its_bearer_and_header_after_relabel_and_rotation() {
    let _env = TestEnv::new();
    let personal = save("personal", "personal-access", "personal-workspace");
    let work = save("work", "work-access", "work-workspace");
    for label in [&personal, &work] {
        codex::update_account_profile(label, Some("same@example.test".into())).unwrap();
    }
    codex::set_active_account(&work).unwrap();
    let session = provider();
    session.set_oauth_account(Some(&work)).unwrap();
    session
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    let original = session.oauth_account_pin().unwrap();
    codex::set_active_account(&personal).unwrap();
    codex::update_account_tokens(
        &work,
        "rotated-work",
        "rotated-refresh",
        None,
        Some("work-workspace".into()),
        Some(future_expiry()),
    )
    .unwrap();
    let mut store = codex::load_auth_file().unwrap();
    store
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == work)
        .unwrap()
        .label = "openai-panda".into();
    codex::save_auth_file(&store).unwrap();
    assert_eq!(
        session.oauth_account_label().as_deref(),
        Some("openai-panda")
    );
    let bearer = openai_access_token(session.oauth_account_pin(), &session.credentials)
        .await
        .unwrap();
    let credentials = session.credentials.read().await;
    let request = crate::openai_stream_runtime::response_http_request(
        &Client::new(),
        &OpenAIProvider::responses_url(&credentials),
        &bearer,
        true,
        credentials.account_id.as_deref(),
        None,
        Some("fixture-model"),
        None,
    )
    .build()
    .unwrap();
    assert_eq!(request.headers()["authorization"], "Bearer rotated-work");
    assert_eq!(request.headers()["chatgpt-account-id"], "work-workspace");
    drop(credentials);
    assert_eq!(
        jcode_base::auth::account_pins::resolve_openai(&original).unwrap(),
        "openai-panda"
    );
    let fork = session.fork();
    fork.set_oauth_account(Some(&personal)).unwrap();
    assert_eq!(
        session.oauth_account_label().as_deref(),
        Some("openai-panda")
    );
    codex::remove_account("openai-panda").unwrap();
    save("openai-panda", "replacement", "other-workspace");
    assert!(
        openai_access_token(session.oauth_account_pin(), &session.credentials)
            .await
            .is_err()
    );
    let _key = EnvGuard::set("OPENAI_API_KEY", "fixture-platform-key");
    session
        .set_credential_mode(OpenAICredentialMode::ApiKey)
        .unwrap();
    assert_eq!(token(&session).await, "fixture-platform-key");
    assert!(session.credentials.read().await.account_id.is_none());
    let api_fork = session.fork();
    assert_eq!(
        api_fork.credential_mode(),
        jcode_provider_core::CredentialMode::ApiKey
    );

    assert_eq!(api_fork.oauth_account_pin(), session.oauth_account_pin());
}

#[tokio::test]
async fn unknown_identity_binding_survives_token_rotation_and_rejects_reused_label() {
    let _env = TestEnv::new();
    let label = codex::upsert_account(codex::OpenAiAccount {
        record_id: None,
        label: "unknown".into(),
        access_token: "unknown-access".into(),
        refresh_token: "unknown-refresh".into(),
        id_token: None,
        account_id: None,
        email: None,
        expires_at: Some(future_expiry()),
    })
    .unwrap();
    let session = provider();
    session.set_oauth_account(Some(&label)).unwrap();
    codex::update_account_tokens(
        &label,
        "rotated-unknown",
        "rotated-refresh",
        None,
        None,
        Some(future_expiry()),
    )
    .unwrap();
    codex::update_account_profile(&label, Some("enriched@example.test".into())).unwrap();
    assert_eq!(token(&session).await, "rotated-unknown");
    save(&label, "unrelated-login", "other-workspace");
    assert!(
        openai_access_token(session.oauth_account_pin(), &session.credentials)
            .await
            .is_err()
    );
}
