use super::*;

#[tokio::test]
async fn same_email_workspace_pin_sends_its_own_bearer_and_account_header() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account(
        "personal",
        "personal-access",
        "personal-refresh",
        "personal-id",
    );
    let work = save_account("work", "work-access", "work-refresh", "work-id");
    for label in [&personal, &work] {
        jcode_base::auth::codex::update_account_profile(label, Some("same@example.com".into()))
            .unwrap();
    }
    jcode_base::auth::codex::set_active_account(&personal).unwrap();
    let provider =
        OpenAIProvider::new(jcode_base::auth::codex::load_credentials_for_account(&work).unwrap());
    provider.set_oauth_account(Some(&work)).unwrap();
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    let pin = jcode_base::auth::codex::pin_for_label(&work).unwrap();
    // Refresh and email changes must not change a workspace's identity.
    save_account(&work, "work-rotated", "work-refresh-rotated", "work-id");
    jcode_base::auth::codex::update_account_profile(&work, Some("same@example.com".into()))
        .unwrap();
    assert_eq!(
        jcode_base::auth::codex::resolve_pin(&pin).as_deref(),
        Some(work.as_str())
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/responses", listener.local_addr().unwrap());
    jcode_base::env::set_var("JCODE_TEST_OPENAI_RESPONSES_URL", &url);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n").await.unwrap();
        String::from_utf8(bytes).unwrap().to_ascii_lowercase()
    });
    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        openai_stream_runtime::stream_response(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            Arc::clone(&provider.credentials),
            provider.account_pin.clone(),
            serde_json::json!({"model":"gpt-5.4", "input":[], "stream":true}),
            "test".into(),
            tx,
        ),
    )
    .await
    .expect("local request must finish");
    let request = server.await.unwrap();
    assert!(
        request.contains("authorization: bearer work-rotated\r\n"),
        "{request}"
    );
    assert!(
        request.contains("chatgpt-account-id: work-id\r\n"),
        "{request}"
    );
    assert!(!request.contains("personal-access"));
    jcode_base::auth::codex::update_account_profile(&work, Some("renamed@example.com".into()))
        .unwrap();
    assert_eq!(
        jcode_base::auth::codex::resolve_pin(&pin).as_deref(),
        Some(work.as_str())
    );
    // Pin removal must never retarget this workspace to the same-email account.
    jcode_base::auth::codex::remove_account(&work).unwrap();
    assert_eq!(jcode_base::auth::codex::resolve_pin(&pin), None);
    let _ = result; // The assertion is the actual HTTP request, not SSE output.
    drain.await.unwrap();
}

#[tokio::test]
async fn api_key_request_ignores_deleted_oauth_pin() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let label = save_account("work", "oauth-fixture", "refresh-fixture", "work-id");
    let provider =
        OpenAIProvider::new(jcode_base::auth::codex::load_credentials_for_account(&label).unwrap());
    provider.set_oauth_account(Some(&label)).unwrap();
    jcode_base::env::set_var("OPENAI_API_KEY", "sk-fixture-explicit-api");
    provider
        .set_credential_mode(OpenAICredentialMode::ApiKey)
        .unwrap();
    jcode_base::auth::codex::remove_account(&label).unwrap();
    assert!(
        provider.account_pin.get().is_some(),
        "mode changes preserve the OAuth pin"
    );
    assert_eq!(
        openai_stream_runtime::openai_access_token(&provider.credentials, &provider.account_pin)
            .await
            .unwrap(),
        "sk-fixture-explicit-api"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    jcode_base::env::set_var(
        "JCODE_TEST_OPENAI_RESPONSES_URL",
        format!("http://{}/responses", listener.local_addr().unwrap()),
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&chunk[..n]);
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n").await.unwrap();
        String::from_utf8(bytes).unwrap().to_ascii_lowercase()
    });
    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        openai_stream_runtime::stream_response(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            Arc::clone(&provider.credentials),
            provider.account_pin.clone(),
            serde_json::json!({"model":"gpt-5.4", "input":[], "stream":true}),
            "test".into(),
            tx,
        ),
    )
    .await
    .expect("local API-key request must finish");
    let request = tokio::time::timeout(std::time::Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(request.contains("authorization: bearer sk-fixture-explicit-api\r\n"));
    assert!(!request.contains("chatgpt-account-id:"));
    assert!(!request.contains("oauth-fixture"));
    drain.await.unwrap();
}

struct AccountSandbox {
    _home: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_runtime: Option<std::ffi::OsString>,
    previous_url: Option<std::ffi::OsString>,
    previous_account: Option<String>,
    previous_api_key: Option<std::ffi::OsString>,
}

impl AccountSandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        let previous_runtime = std::env::var_os("JCODE_RUNTIME_PROVIDER");
        let previous_url = std::env::var_os("JCODE_TEST_OPENAI_RESPONSES_URL");
        let previous_api_key = std::env::var_os("OPENAI_API_KEY");
        let previous_account = jcode_base::auth::codex::get_active_account_override();
        jcode_base::env::set_var("JCODE_HOME", home.path());
        jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-oauth");
        jcode_base::auth::codex::set_active_account_override(None);
        Self {
            _home: home,
            previous_home,
            previous_runtime,
            previous_url,
            previous_account,
            previous_api_key,
        }
    }
}

impl Drop for AccountSandbox {
    fn drop(&mut self) {
        jcode_base::auth::codex::set_active_account_override(self.previous_account.take());
        for (key, value) in [
            ("OPENAI_API_KEY", self.previous_api_key.take()),
            ("JCODE_HOME", self.previous_home.take()),
            ("JCODE_RUNTIME_PROVIDER", self.previous_runtime.take()),
            ("JCODE_TEST_OPENAI_RESPONSES_URL", self.previous_url.take()),
        ] {
            match value {
                Some(value) => jcode_base::env::set_var(key, value),
                None => jcode_base::env::remove_var(key),
            }
        }
    }
}

#[tokio::test]
async fn legacy_email_workspace_pin_is_unique_or_fails_closed() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let first = save_account("first", "first-token", "first-refresh", "first-id");
    jcode_base::auth::codex::update_account_profile(&first, Some("same@example.com".into()))
        .unwrap();
    let legacy = jcode_base::auth::AccountPin::new(&first, Some("same@example.com".into()));
    assert_eq!(
        jcode_base::auth::codex::resolve_pin(&legacy).as_deref(),
        Some(first.as_str())
    );
    let second = save_account("second", "second-token", "second-refresh", "second-id");
    jcode_base::auth::codex::update_account_profile(&second, Some("SAME@example.com".into()))
        .unwrap();
    assert_eq!(jcode_base::auth::codex::resolve_pin(&legacy), None);
    let provider =
        OpenAIProvider::new(jcode_base::auth::codex::load_credentials_for_account(&first).unwrap());
    provider.account_pin.set(Some(legacy.clone()));
    assert!(
        openai_access_token(&provider.credentials, &provider.account_pin)
            .await
            .is_err()
    );
    assert_eq!(provider.account_pin.get(), Some(legacy));
    let stable = jcode_base::auth::codex::pin_for_label(&second).unwrap();
    assert_eq!(stable.identity.as_deref(), Some("id:second-id"));
    jcode_base::auth::codex::remove_account(&first).unwrap();
    let relabeled = jcode_base::auth::codex::resolve_pin(&stable).unwrap();
    assert_eq!(
        jcode_base::auth::codex::load_credentials_for_account(&relabeled)
            .unwrap()
            .account_id
            .as_deref(),
        Some("second-id")
    );
}

fn save_account(label: &str, token: &str, refresh: &str, identity: &str) -> String {
    jcode_base::auth::codex::upsert_account(jcode_base::auth::codex::OpenAiAccount {
        label: label.into(),
        access_token: token.into(),
        refresh_token: refresh.into(),
        expires_at: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
        id_token: None,
        account_id: Some(identity.into()),
        email: None,
    })
    .unwrap()
}

#[tokio::test]
async fn active_oauth_email_follows_cached_unbound_credentials_not_global_account() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account(
        "new-personal",
        "personal-access",
        "personal-refresh",
        "personal-id",
    );
    let work = save_account("new-work", "work-access", "work-refresh", "work-id");
    jcode_base::auth::codex::update_account_profile(&personal, Some("personal@example.com".into()))
        .unwrap();
    jcode_base::auth::codex::update_account_profile(&work, Some("work@example.com".into()))
        .unwrap();
    jcode_base::auth::codex::set_active_account(&personal).unwrap();
    let provider = OpenAIProvider::new(
        jcode_base::auth::codex::load_credentials_for_account(&personal).unwrap(),
    );
    assert_eq!(provider.oauth_account_label(), None);
    jcode_base::auth::codex::set_active_account(&work).unwrap();
    assert_eq!(
        provider.active_oauth_account_email().as_deref(),
        Some("personal@example.com")
    );
    provider.credentials.write().await.access_token = "rotated-access".into();
    assert_eq!(
        provider.active_oauth_account_email().as_deref(),
        Some("personal@example.com")
    );

    // An actual API-key credential excludes saved identities even if access matches.
    {
        let mut credentials = provider.credentials.write().await;
        credentials.access_token = "personal-access".into();
        credentials.refresh_token.clear();
        credentials.id_token = None;
    }
    assert_eq!(provider.active_oauth_account_email(), None);
    {
        let mut credentials = provider.credentials.write().await;
        credentials.access_token = "unknown-access".into();
        credentials.refresh_token = "unknown-refresh".into();
    }
    assert_eq!(provider.active_oauth_account_email(), None);
    {
        let mut credentials = provider.credentials.write().await;
        // JWT payload: {"email":"id@example.com"}. No saved-account match.
        credentials.id_token = Some("header.eyJlbWFpbCI6ImlkQGV4YW1wbGUuY29tIn0.signature".into());
        assert_eq!(provider.active_oauth_account_email(), None);
    }
    assert_eq!(
        provider.active_oauth_account_email().as_deref(),
        Some("id@example.com")
    );
}

#[tokio::test]
async fn oauth_account_binding_survives_global_switch_refresh_and_fork() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account(
        "new-personal",
        "personal-access",
        "personal-refresh-old",
        "personal-id",
    );
    let work = save_account("new-work", "work-access", "work-refresh", "work-id");
    jcode_base::auth::codex::set_active_account(&personal).unwrap();
    let provider = OpenAIProvider::new(
        jcode_base::auth::codex::load_credentials_for_account(&personal).unwrap(),
    );
    provider.set_oauth_account(Some(&personal)).unwrap();
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    assert_eq!(
        openai_access_token(&provider.credentials, &provider.account_pin)
            .await
            .unwrap(),
        "personal-access"
    );

    jcode_base::auth::codex::set_active_account(&work).unwrap();
    provider.reload_credentials_now();
    provider.invalidate_credentials().await;
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    assert_eq!(
        openai_access_token(&provider.credentials, &provider.account_pin)
            .await
            .unwrap(),
        "personal-access"
    );
    assert_eq!(
        provider.credentials.read().await.account_id.as_deref(),
        Some("personal-id")
    );
    assert_eq!(provider.catalog_scope(), personal);
    assert_eq!(
        provider.resolved_account_label(AccountProviderKind::OpenAi),
        Some(personal.clone())
    );

    // An already-rotated account refresh must join personal's single-flight
    // transaction even though work is now the globally active saved login.
    save_account(
        &personal,
        "personal-access-rotated",
        "personal-refresh-new",
        "personal-id",
    );
    let token = openai_stream_runtime::force_refresh_openai_token(
        &provider.credentials,
        &provider.account_pin,
        "personal-refresh-old",
    )
    .await
    .unwrap();
    assert_eq!(token, "personal-access-rotated");
    assert_eq!(
        provider.credentials.read().await.account_id.as_deref(),
        Some("personal-id")
    );
    assert_eq!(
        jcode_base::auth::codex::load_credentials_for_account(&work)
            .unwrap()
            .access_token,
        "work-access"
    );
    assert_eq!(
        jcode_base::auth::codex::active_account_label().as_deref(),
        Some(work.as_str())
    );

    provider
        .model_reasoning_efforts
        .write()
        .unwrap()
        .insert("personal-only".into(), vec!["max".into()]);
    let fork = provider.fork();
    assert_eq!(
        fork.oauth_account_label().as_deref(),
        Some(personal.as_str())
    );
    fork.set_oauth_account(Some(&work)).unwrap();
    fork.set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    assert!(
        provider
            .model_reasoning_efforts
            .read()
            .unwrap()
            .contains_key("personal-only")
    );
    assert_eq!(
        provider.oauth_account_label().as_deref(),
        Some(personal.as_str())
    );
    assert_eq!(
        provider.credentials.read().await.access_token,
        "personal-access-rotated"
    );
    assert_eq!(fork.oauth_account_label().as_deref(), Some(work.as_str()));
    assert!(fork.set_oauth_account(Some("missing-account")).is_err());
    assert_eq!(fork.oauth_account_label().as_deref(), Some(work.as_str()));
    provider.invalidate_credentials().await;
    assert_eq!(
        provider.credentials.read().await.access_token,
        "personal-access-rotated"
    );
    provider.set_oauth_account(Some(&work)).unwrap();
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    jcode_base::auth::codex::remove_account(&work).unwrap();
    assert!(
        provider.complete(&[], &[], "test", None).await.is_err(),
        "removed pinned account must fail before websocket continuation"
    );
    // Outside an explicit OAuth request the window pin drops a removed
    // account and follows the default login, never the removed bearer.
    assert_ne!(
        openai_access_token(&provider.credentials, &provider.account_pin)
            .await
            .ok()
            .as_deref(),
        Some("work-access"),
        "removed pinned account must not use cached bearer"
    );
}
