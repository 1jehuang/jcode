use super::*;
use anyhow::{Result, anyhow};

#[test]
fn pkce_verifier_and_challenge_are_different() {
    let (verifier, challenge) = generate_pkce();
    assert_ne!(verifier, challenge);
    assert_eq!(verifier.len(), 64);
    assert!(!challenge.is_empty());
}

#[test]
fn pkce_challenge_is_base64url() {
    let (_, challenge) = generate_pkce();
    assert!(!challenge.contains('+'));
    assert!(!challenge.contains('/'));
    assert!(!challenge.contains('='));
}

#[test]
fn pkce_challenge_is_sha256_of_verifier() {
    let (verifier, challenge) = generate_pkce();
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let expected = URL_SAFE_NO_PAD.encode(hash);
    assert_eq!(challenge, expected);
}

#[test]
fn pkce_generates_unique_values() {
    let (v1, c1) = generate_pkce();
    let (v2, c2) = generate_pkce();
    assert_ne!(v1, v2);
    assert_ne!(c1, c2);
}

#[test]
fn state_is_random_hex() {
    let state = generate_state();
    assert_eq!(state.len(), 32);
    assert!(state.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn state_generates_unique_values() {
    let s1 = generate_state();
    let s2 = generate_state();
    assert_ne!(s1, s2);
}

#[test]
fn oauth_tokens_serialization_roundtrip() -> Result<()> {
    let tokens = OAuthTokens {
        access_token: "at_abc".to_string(),
        refresh_token: "rt_def".to_string(),
        expires_at: 1234567890,
        id_token: Some("idt_ghi".to_string()),
        scopes: Vec::new(),
    };
    let json = serde_json::to_string(&tokens)?;
    let parsed: OAuthTokens = serde_json::from_str(&json)?;
    assert_eq!(parsed.access_token, "at_abc");
    assert_eq!(parsed.refresh_token, "rt_def");
    assert_eq!(parsed.expires_at, 1234567890);
    assert_eq!(parsed.id_token, Some("idt_ghi".to_string()));
    Ok(())
}

#[test]
fn oauth_tokens_without_id_token() -> Result<()> {
    let tokens = OAuthTokens {
        access_token: "at".to_string(),
        refresh_token: "rt".to_string(),
        expires_at: 0,
        id_token: None,
        scopes: Vec::new(),
    };
    let json = serde_json::to_string(&tokens)?;
    assert!(!json.contains("id_token"));
    let parsed: OAuthTokens = serde_json::from_str(&json)?;
    assert!(parsed.id_token.is_none());
    Ok(())
}

#[test]
fn save_openai_tokens_uses_jcode_home_sandbox() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());

    let tokens = OAuthTokens {
        access_token: "at_sandbox".to_string(),
        refresh_token: "rt_sandbox".to_string(),
        expires_at: 1234567890,
        id_token: Some("id_sandbox".to_string()),
        scopes: Vec::new(),
    };

    save_openai_tokens(&tokens)?;

    let auth_path = temp.path().join("openai-auth.json");
    assert!(auth_path.exists(), "expected {}", auth_path.display());

    let creds = crate::auth::codex::load_credentials()?;
    assert_eq!(creds.access_token, "at_sandbox");
    assert_eq!(creds.refresh_token, "rt_sandbox");
    assert_eq!(creds.id_token.as_deref(), Some("id_sandbox"));
    assert_eq!(creds.expires_at, Some(1234567890));
    Ok(())
}

#[test]
fn save_claude_tokens_preserves_existing_account_metadata() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());

    crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        label: "claude-1".to_string(),
        access: "old_access".to_string(),
        refresh: "old_refresh".to_string(),
        expires: 1,
        email: Some("user@example.com".to_string()),
        subscription_type: Some("pro".to_string()),
        scopes: vec!["user:inference".to_string()],
    })?;

    let refreshed = OAuthTokens {
        access_token: "new_access".to_string(),
        refresh_token: "new_refresh".to_string(),
        expires_at: 2,
        id_token: None,
        scopes: Vec::new(),
    };
    save_claude_tokens_for_account(&refreshed, "claude-1")?;

    let account = crate::auth::claude::list_accounts()?
        .into_iter()
        .find(|account| account.label == "claude-1")
        .expect("claude account should exist");
    assert_eq!(account.access, "new_access");
    assert_eq!(account.refresh, "new_refresh");
    assert_eq!(account.email.as_deref(), Some("user@example.com"));
    assert_eq!(account.subscription_type.as_deref(), Some("pro"));
    assert_eq!(account.scopes, vec!["user:inference".to_string()]);
    Ok(())
}

#[test]
fn claude_oauth_constants() {
    assert!(!claude::CLIENT_ID.is_empty());
    assert_eq!(
        claude::AUTHORIZE_URL,
        "https://claude.com/cai/oauth/authorize"
    );
    assert_eq!(
        claude::CONSOLE_AUTHORIZE_URL,
        "https://platform.claude.com/oauth/authorize"
    );
    assert_eq!(
        claude::TOKEN_URL,
        "https://platform.claude.com/v1/oauth/token"
    );
    assert!(claude::PROFILE_URL.starts_with("https://"));
    assert_eq!(
        claude::REDIRECT_URI,
        "https://platform.claude.com/oauth/code/callback"
    );
    assert!(claude::SCOPES.contains("user:inference"));
    assert!(claude::SCOPES.contains("user:sessions:claude_code"));
    assert!(claude::REFRESH_SCOPES.contains("user:file_upload"));
}

#[tokio::test]
async fn fetch_claude_profile_email_reads_account_email() -> Result<()> {
    let body = serde_json::json!({
        "account": {
            "email": "user@example.com"
        }
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &body).await;

    let url = format!("http://127.0.0.1:{}/api/oauth/profile", port);
    let email = fetch_claude_profile_email_at_url("token", &url).await?;

    assert_eq!(email, Some("user@example.com".to_string()));
    Ok(())
}

#[tokio::test]
async fn fetch_claude_profile_email_handles_missing_email() -> Result<()> {
    let body = serde_json::json!({
        "account": {}
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &body).await;

    let url = format!("http://127.0.0.1:{}/api/oauth/profile", port);
    let email = fetch_claude_profile_email_at_url("token", &url).await?;

    assert!(email.is_none());
    Ok(())
}

#[tokio::test]
async fn fetch_claude_profile_email_propagates_http_error() -> Result<()> {
    let body = serde_json::json!({
        "error": "bad_token"
    })
    .to_string();
    let (port, _handle) = mock_token_server(401, &body).await;

    let url = format!("http://127.0.0.1:{}/api/oauth/profile", port);
    let err = fetch_claude_profile_email_at_url("token", &url)
        .await
        .expect_err("Profile fetch should fail")
        .to_string();

    assert!(err.contains("Profile fetch failed"));
    Ok(())
}

#[test]
fn openai_oauth_constants() {
    assert!(!openai::CLIENT_ID.is_empty());
    assert!(openai::AUTHORIZE_URL.starts_with("https://"));
    assert!(openai::TOKEN_URL.starts_with("https://"));
    assert!(openai::redirect_uri(openai::DEFAULT_PORT).starts_with("http"));
    assert!(!openai::SCOPES.is_empty());
}

#[tokio::test]
async fn wait_for_callback_async_parses_code() -> Result<()> {
    let state = "test_state_abc";
    let listener = bind_callback_listener(0)?;
    let port = listener.local_addr().map_err(|e| anyhow!(e))?.port();

    let state_clone = state.to_string();
    let handle =
        tokio::spawn(
            async move { wait_for_callback_async_on_listener(listener, &state_clone).await },
        );

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(
            format!(
                "GET /callback?code=test_code_123&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                state
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| anyhow!(e))?;

    let result = handle.await.map_err(|e| anyhow!(e))?;
    assert!(result.is_ok());
    assert_eq!(result?, "test_code_123");
    Ok(())
}

#[tokio::test]
async fn wait_for_callback_async_on_prebound_listener_parses_code() -> Result<()> {
    let state = "test_state_prebound";
    let listener = bind_callback_listener(0)?;
    let port = listener.local_addr().map_err(|e| anyhow!(e))?.port();

    let state_clone = state.to_string();
    let handle =
        tokio::spawn(
            async move { wait_for_callback_async_on_listener(listener, &state_clone).await },
        );

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(
            format!(
                "GET /callback?code=prebound_code&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                state
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| anyhow!(e))?;

    let result = handle.await.map_err(|e| anyhow!(e))?;
    assert!(result.is_ok());
    assert_eq!(result?, "prebound_code");
    Ok(())
}

#[tokio::test]
async fn wait_for_callback_async_ignores_wrong_state_until_valid_callback() -> Result<()> {
    let listener = bind_callback_listener(0)?;
    let port = listener.local_addr().map_err(|e| anyhow!(e))?.port();

    let handle = tokio::spawn(async move {
        wait_for_callback_async_on_listener(listener, "expected_state").await
    });

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(
            b"GET /callback?code=code123&state=wrong_state HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .map_err(|e| anyhow!(e))?;
    drop(stream);

    let mut valid_stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    valid_stream
        .write_all(
            b"GET /callback?code=code123&state=expected_state HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .map_err(|e| anyhow!(e))?;

    let result = handle.await.map_err(|e| anyhow!(e))?;
    assert!(result.is_ok());
    assert_eq!(result?, "code123");
    Ok(())
}

#[tokio::test]
async fn wait_for_callback_async_ignores_missing_code_until_valid_callback() -> Result<()> {
    let listener = bind_callback_listener(0)?;
    let port = listener.local_addr().map_err(|e| anyhow!(e))?.port();

    let handle =
        tokio::spawn(
            async move { wait_for_callback_async_on_listener(listener, "state123").await },
        );

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    use tokio::io::AsyncWriteExt;
    stream
        .write_all(b"GET /callback?state=state123 HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .map_err(|e| anyhow!(e))?;
    drop(stream);

    let mut valid_stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    valid_stream
        .write_all(
            b"GET /callback?code=valid_code&state=state123 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .map_err(|e| anyhow!(e))?;

    let result = handle.await.map_err(|e| anyhow!(e))?;
    assert!(result.is_ok());
    assert_eq!(result?, "valid_code");
    Ok(())
}

#[tokio::test]
async fn wait_for_callback_async_surfaces_provider_error() -> Result<()> {
    let listener = bind_callback_listener(0)?;
    let port = listener.local_addr().map_err(|e| anyhow!(e))?.port();

    let handle = tokio::spawn(async move {
        wait_for_callback_async_on_listener(listener, "expected_state").await
    });

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| anyhow!(e))?;
    use tokio::io::AsyncWriteExt;
    stream
            .write_all(
                b"GET /callback?error=access_denied&state=expected_state HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
            .map_err(|e| anyhow!(e))?;

    let result = handle.await.map_err(|e| anyhow!(e))?;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("OAuth provider returned error")
    );
    Ok(())
}
