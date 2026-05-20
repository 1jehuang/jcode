use super::*;
use anyhow::{Result, anyhow};
use std::collections::HashMap;

fn utf8_body(body: Vec<u8>) -> Result<String> {
    String::from_utf8(body).map_err(|e| anyhow!(e))
}

fn json_body(body: Vec<u8>) -> Result<serde_json::Value> {
    serde_json::from_slice(&body).map_err(|e| anyhow!(e))
}

fn require_json_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("missing JSON string field: {key}"))
}

fn require_param<'a>(pairs: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    pairs
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("missing form/query param: {key}"))
}

#[test]
fn claude_exchange_request_uses_json_like_claude_code() -> Result<()> {
    let (_url, content_type, _body) =
        build_claude_exchange_request("code123", "verifier456", claude::REDIRECT_URI, None);
    assert_eq!(content_type, "application/json");
    assert_ne!(content_type, "application/x-www-form-urlencoded");
    Ok(())
}

#[test]
fn claude_exchange_request_body_is_json() -> Result<()> {
    let (_url, _ct, body) =
        build_claude_exchange_request("code123", "verifier456", claude::REDIRECT_URI, None);
    let body = json_body(body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "authorization_code");
    Ok(())
}

#[test]
fn claude_refresh_request_uses_json_like_claude_code() -> Result<()> {
    let (_url, content_type, _body) = build_claude_refresh_request("rt_test");
    assert_eq!(content_type, "application/json");
    assert_ne!(content_type, "application/x-www-form-urlencoded");
    Ok(())
}

#[test]
fn claude_refresh_request_body_is_json() -> Result<()> {
    let (_url, _ct, body) = build_claude_refresh_request("rt_test");
    let body = json_body(body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "refresh_token");
    Ok(())
}

// ========================
// Claude exchange request body validation
// ========================

#[test]
fn claude_exchange_request_contains_required_fields() -> Result<()> {
    let (_url, _ct, body) = build_claude_exchange_request(
        "auth_code_xyz",
        "verifier_abc",
        "https://example.com/callback",
        None,
    );
    let body = json_body(body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "authorization_code");
    assert_eq!(require_json_str(&body, "client_id")?, claude::CLIENT_ID);
    assert_eq!(require_json_str(&body, "code")?, "auth_code_xyz");
    assert_eq!(require_json_str(&body, "code_verifier")?, "verifier_abc");
    assert_eq!(
        require_json_str(&body, "redirect_uri")?,
        "https://example.com/callback"
    );
    assert_eq!(require_json_str(&body, "state")?, "verifier_abc");
    Ok(())
}

#[test]
fn claude_exchange_request_includes_state_when_present() -> Result<()> {
    let (_url, _ct, body) = build_claude_exchange_request(
        "code",
        "verifier",
        claude::REDIRECT_URI,
        Some("state_value"),
    );
    let body = json_body(body)?;
    assert_eq!(require_json_str(&body, "state")?, "state_value");
    Ok(())
}

#[test]
fn claude_exchange_request_targets_correct_url() -> Result<()> {
    let (url, _ct, _body) = build_claude_exchange_request("c", "v", claude::REDIRECT_URI, None);
    assert_eq!(url, "https://platform.claude.com/v1/oauth/token");
    Ok(())
}

// ========================
// Claude refresh request body validation
// ========================

#[test]
fn claude_refresh_request_contains_required_fields() -> Result<()> {
    let (_url, _ct, body) = build_claude_refresh_request("rt_refresh_token_value");
    let body = json_body(body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "refresh_token");
    assert_eq!(
        require_json_str(&body, "refresh_token")?,
        "rt_refresh_token_value"
    );
    assert_eq!(require_json_str(&body, "client_id")?, claude::CLIENT_ID);
    assert_eq!(require_json_str(&body, "scope")?, claude::REFRESH_SCOPES);
    Ok(())
}

#[test]
fn claude_refresh_request_can_omit_scope_for_legacy_fallback() -> Result<()> {
    let (_url, _ct, body) = build_claude_refresh_request_with_scope("rt_refresh_token_value", None);
    let body = json_body(body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "refresh_token");
    assert_eq!(
        require_json_str(&body, "refresh_token")?,
        "rt_refresh_token_value"
    );
    assert_eq!(require_json_str(&body, "client_id")?, claude::CLIENT_ID);
    assert!(body.get("scope").is_none());
    Ok(())
}

#[test]
fn claude_refresh_invalid_scope_detection_matches_anthropic_error() {
    let err = anyhow::anyhow!(
        "Token refresh failed: {{\"error\": \"invalid_scope\", \"error_description\": \"The requested scope is invalid, unknown, or malformed.\"}}"
    );
    assert!(claude_refresh_error_is_invalid_scope(&err));
}

#[test]
fn claude_scope_validation_requires_inference_when_scope_is_reported() {
    let ok = vec!["user:profile".to_string(), "user:inference".to_string()];
    assert!(ensure_claude_inference_scope(&ok, "token refresh").is_ok());

    let missing = vec!["org:create_api_key".to_string(), "user:profile".to_string()];
    let err = ensure_claude_inference_scope(&missing, "token refresh")
        .expect_err("reported scopes without inference should fail")
        .to_string();
    assert!(err.contains("user:inference"), "unexpected error: {err}");

    // Some mock/legacy token endpoints omit `scope`; absence should not be
    // treated as proof that the token is bad.
    assert!(ensure_claude_inference_scope(&[], "token refresh").is_ok());
}

#[test]
fn claude_refresh_request_targets_correct_url() -> Result<()> {
    let (url, _ct, _body) = build_claude_refresh_request("rt");
    assert_eq!(url, "https://platform.claude.com/v1/oauth/token");
    Ok(())
}

// ========================
// OpenAI exchange request validation
// ========================

#[test]
fn openai_exchange_request_uses_form_urlencoded() -> Result<()> {
    let (_url, content_type, _body) =
        build_openai_exchange_request("code", "verifier", "http://localhost:1455/auth/callback");
    assert_eq!(content_type, "application/x-www-form-urlencoded");
    Ok(())
}

#[test]
fn openai_exchange_request_contains_required_fields() -> Result<()> {
    let (_url, _ct, body) = build_openai_exchange_request(
        "oai_code_123",
        "oai_verifier",
        "http://localhost:1455/auth/callback",
    );
    let body_str = utf8_body(body)?;
    assert!(body_str.contains("grant_type=authorization_code"));
    assert!(body_str.contains(&format!("client_id={}", openai::CLIENT_ID)));
    assert!(body_str.contains("code=oai_code_123"));
    assert!(body_str.contains("code_verifier=oai_verifier"));
    assert!(body_str.contains("redirect_uri="));
    Ok(())
}

#[test]
fn openai_exchange_request_targets_correct_url() -> Result<()> {
    let (url, _ct, _body) = build_openai_exchange_request("c", "v", "http://localhost/cb");
    assert_eq!(url, "https://auth.openai.com/oauth/token");
    Ok(())
}

// ========================
// OpenAI refresh request validation
// ========================

#[test]
fn openai_refresh_request_uses_form_urlencoded() -> Result<()> {
    let (_url, content_type, _body) = build_openai_refresh_request("rt_oai");
    assert_eq!(content_type, "application/x-www-form-urlencoded");
    Ok(())
}

#[test]
fn openai_refresh_request_contains_required_fields() -> Result<()> {
    let (_url, _ct, body) = build_openai_refresh_request("rt_oai_value");
    let body_str = utf8_body(body)?;
    assert!(body_str.contains("grant_type=refresh_token"));
    assert!(body_str.contains(&format!("client_id={}", openai::CLIENT_ID)));
    assert!(body_str.contains("refresh_token=rt_oai_value"));
    Ok(())
}

#[test]
fn openai_refresh_request_targets_correct_url() -> Result<()> {
    let (url, _ct, _body) = build_openai_refresh_request("rt");
    assert_eq!(url, "https://auth.openai.com/oauth/token");
    Ok(())
}

// ========================
// Auth URL construction
// ========================

#[test]
fn claude_auth_url_contains_required_params() -> Result<()> {
    let (verifier, challenge) = generate_pkce();
    let auth_url = format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        claude::AUTHORIZE_URL,
        claude::CLIENT_ID,
        urlencoding::encode(claude::REDIRECT_URI),
        urlencoding::encode(claude::SCOPES),
        challenge,
        verifier,
    );
    let parsed = url::Url::parse(&auth_url).map_err(|e| anyhow!(e))?;
    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(require_param(&params, "code")?, "true");
    assert_eq!(require_param(&params, "client_id")?, claude::CLIENT_ID);
    assert_eq!(require_param(&params, "response_type")?, "code");
    assert_eq!(
        require_param(&params, "redirect_uri")?,
        claude::REDIRECT_URI
    );
    assert_eq!(require_param(&params, "scope")?, claude::SCOPES);
    assert_eq!(require_param(&params, "code_challenge")?, challenge);
    assert_eq!(require_param(&params, "code_challenge_method")?, "S256");
    assert_eq!(require_param(&params, "state")?, verifier);
    assert_eq!(parsed.host_str(), Some("claude.com"));
    assert_eq!(parsed.path(), "/cai/oauth/authorize");
    Ok(())
}

#[test]
fn openai_auth_url_contains_required_params() -> Result<()> {
    let (_verifier, challenge) = generate_pkce();
    let state = generate_state();
    let redirect_uri = openai::redirect_uri(openai::DEFAULT_PORT);
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        openai::AUTHORIZE_URL,
        openai::CLIENT_ID,
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(openai::SCOPES),
        challenge,
        state,
    );
    let parsed = url::Url::parse(&auth_url).map_err(|e| anyhow!(e))?;
    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(require_param(&params, "response_type")?, "code");
    assert_eq!(require_param(&params, "client_id")?, openai::CLIENT_ID);
    assert_eq!(require_param(&params, "redirect_uri")?, redirect_uri);
    assert_eq!(require_param(&params, "scope")?, openai::SCOPES);
    assert_eq!(require_param(&params, "code_challenge")?, challenge);
    assert_eq!(require_param(&params, "code_challenge_method")?, "S256");
    assert_eq!(require_param(&params, "state")?, state);
    Ok(())
}

#[test]
fn claude_auth_url_with_dynamic_redirect_uri() -> Result<()> {
    let (verifier, challenge) = generate_pkce();
    let dynamic_redirect = "http://localhost:34531/callback";
    let auth_url = format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        claude::AUTHORIZE_URL,
        claude::CLIENT_ID,
        urlencoding::encode(dynamic_redirect),
        urlencoding::encode(claude::SCOPES),
        challenge,
        verifier,
    );
    let parsed = url::Url::parse(&auth_url).map_err(|e| anyhow!(e))?;
    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(require_param(&params, "redirect_uri")?, dynamic_redirect);
    Ok(())
}

// ========================
// Code parsing (plain code, URL, code#state)
// ========================

#[test]
fn parse_plain_auth_code() -> Result<()> {
    let input = "abc123def456";
    let (raw_code, state) = parse_claude_code_input(input)?;
    assert_eq!(raw_code, "abc123def456");
    assert!(state.is_none());
    Ok(())
}

#[test]
fn parse_code_from_url() -> Result<()> {
    let input = "https://example.com/callback?code=mycode123&state=mystate";
    let (raw_code, state) = parse_claude_code_input(input)?;
    assert_eq!(raw_code, "mycode123");
    assert_eq!(state, Some("mystate".to_string()));
    Ok(())
}

#[test]
fn parse_code_from_query_string() -> Result<()> {
    let input = "code=mycode456&state=s";
    let (raw_code, state) = parse_claude_code_input(input)?;
    assert_eq!(raw_code, "mycode456");
    assert_eq!(state, Some("s".to_string()));
    Ok(())
}

#[test]
fn parse_code_hash_state_format() -> Result<()> {
    let raw_code = "authcode789#statevalue";
    let (code, state) = parse_claude_code_input(raw_code)?;
    assert_eq!(code, "authcode789");
    assert_eq!(state, Some("statevalue".to_string()));
    Ok(())
}

#[test]
fn parse_code_without_hash() -> Result<()> {
    let raw_code = "authcode_no_hash";
    let (code, state) = parse_claude_code_input(raw_code)?;
    assert_eq!(code, "authcode_no_hash");
    assert!(state.is_none());
    Ok(())
}

#[test]
fn parse_code_trims_input_whitespace() -> Result<()> {
    let input = "   authcode_trim   ";
    let (code, state) = parse_claude_code_input(input)?;
    assert_eq!(code, "authcode_trim");
    assert!(state.is_none());
    Ok(())
}

#[test]
fn parse_code_url_with_whitespace_extracts_state() -> Result<()> {
    let input = "   https://example.com/callback?code=mycode&state=mystate   ";
    let (code, state) = parse_claude_code_input(input)?;
    assert_eq!(code, "mycode");
    assert_eq!(state, Some("mystate".to_string()));
    Ok(())
}

#[test]
fn parse_code_rejects_empty_input() {
    let err = parse_claude_code_input("   ").expect_err("empty input should fail");
    assert!(err.to_string().contains("No authorization code provided"));
}

#[test]
fn parse_code_rejects_empty_code_query_param() {
    let err = parse_claude_code_input("code=&state=abc")
        .expect_err("empty code query parameter should fail");
    assert!(err.to_string().contains("No authorization code provided"));
}

#[test]
fn parse_callback_input_requires_state() {
    let err = parse_callback_input_with_state("just-a-code")
        .expect_err("plain code should not satisfy stateful callback parsing");
    assert!(err.to_string().contains("full callback URL"));
}

#[test]
fn parse_callback_input_extracts_code_and_state() -> Result<()> {
    let (code, state) = parse_callback_input_with_state(
        "http://localhost:1455/auth/callback?code=mycode&state=mystate",
    )?;
    assert_eq!(code, "mycode");
    assert_eq!(state, "mystate");
    Ok(())
}

#[test]
fn claude_redirect_uri_uses_manual_callback_for_platform_url() -> Result<()> {
    let selected = claude_redirect_uri_for_input(
        "https://platform.claude.com/oauth/code/callback?code=abc&state=xyz",
        "http://localhost:9999/callback",
    );
    assert_eq!(selected, claude::REDIRECT_URI);
    Ok(())
}

#[test]
fn claude_redirect_uri_accepts_legacy_console_callback_url() -> Result<()> {
    let selected = claude_redirect_uri_for_input(
        "https://console.anthropic.com/oauth/code/callback?code=abc&state=xyz",
        "http://localhost:9999/callback",
    );
    assert_eq!(selected, claude::REDIRECT_URI);
    Ok(())
}

#[test]
fn claude_redirect_uri_keeps_localhost_fallback_for_raw_code() -> Result<()> {
    let selected = claude_redirect_uri_for_input("abc123", "http://localhost:9999/callback");
    assert_eq!(selected, "http://localhost:9999/callback");
    Ok(())
}

// ========================
// Mock server integration: Claude exchange
// ========================

#[tokio::test]
async fn claude_exchange_mock_server_receives_json() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at_mock",
        "refresh_token": "rt_mock",
        "expires_in": 3600,
        "id_token": "idt_mock"
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let result =
        exchange_code_at_url(&url, "code123", "verifier456", "https://redir", None).await?;

    assert_eq!(result.access_token, "at_mock");
    assert_eq!(result.refresh_token, "rt_mock");
    assert_eq!(result.id_token, Some("idt_mock".to_string()));

    let (method, _path, headers, body) = handle.await.map_err(|e| anyhow!(e))?;
    assert_eq!(method, "POST");
    assert_eq!(
        headers
            .get("content-type")
            .map(String::as_str)
            .ok_or_else(|| anyhow!("missing content-type header"))?,
        "application/json"
    );
    let body: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "authorization_code");
    assert_eq!(require_json_str(&body, "code")?, "code123");
    assert_eq!(require_json_str(&body, "code_verifier")?, "verifier456");
    assert_eq!(require_json_str(&body, "state")?, "verifier456");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_mock_server_with_state() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let _ = exchange_code_at_url(&url, "c", "v", "https://r", Some("my_state")).await?;

    let (_method, _path, _headers, body) = handle.await.map_err(|e| anyhow!(e))?;
    let body: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(require_json_str(&body, "state")?, "my_state");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_uses_state_from_url_query_when_present() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let _ = exchange_claude_code_at_url(
        &url,
        "query_state",
        "https://example.com/callback?code=test_code&state=query_state",
        "https://r",
    )
    .await?;

    let (_method, _path, _headers, body) = handle.await.map_err(|e| anyhow!(e))?;
    let body: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(require_json_str(&body, "state")?, "query_state");
    assert_eq!(require_json_str(&body, "code")?, "test_code");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_uses_claude_code_token_headers() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let _ = exchange_claude_code_at_url(&url, "verifier", "plain_code", "https://r").await?;

    let (_method, _path, headers, _body) = handle.await.map_err(|e| anyhow!(e))?;
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert!(
        !headers.contains_key("origin"),
        "unexpected Origin: {headers:?}"
    );
    assert!(
        !headers.contains_key("referer"),
        "unexpected Referer: {headers:?}"
    );
    assert!(
        !headers.contains_key("sec-fetch-mode"),
        "unexpected Sec-Fetch-Mode: {headers:?}"
    );
    Ok(())
}

#[tokio::test]
async fn claude_exchange_rejects_token_without_inference_scope() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600,
        "scope": "org:create_api_key user:profile"
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let err = exchange_claude_code_at_url(&url, "verifier", "plain_code", "https://r")
        .await
        .expect_err("token without user:inference should be rejected")
        .to_string();

    assert!(err.contains("user:inference"), "unexpected error: {err}");
    assert!(err.contains("Claude.ai OAuth"), "unexpected error: {err}");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_preserves_returned_scopes() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600,
        "scope": "user:profile user:inference user:sessions:claude_code"
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let tokens = exchange_claude_code_at_url(&url, "verifier", "plain_code", "https://r").await?;

    assert!(tokens.scopes.iter().any(|scope| scope == "user:inference"));
    Ok(())
}

#[tokio::test]
async fn claude_exchange_cloudflare_403_is_actionable() -> Result<()> {
    let challenge = "<!DOCTYPE html><title>Just a moment...</title><script src='/cdn-cgi/challenge-platform'></script>";
    let (port, _handle) = mock_token_server(403, challenge).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let err = exchange_claude_code_at_url(&url, "verifier", "plain_code", "https://r")
        .await
        .expect_err("Cloudflare challenge should fail with guidance")
        .to_string();

    assert!(err.contains("Cloudflare"), "unexpected error: {err}");
    assert!(err.contains("VPN"), "unexpected error: {err}");
    assert!(err.contains("--no-browser"), "unexpected error: {err}");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_rejects_state_mismatch() -> Result<()> {
    let result = exchange_claude_code_at_url(
        "http://127.0.0.1:1/v1/oauth/token",
        "expected_state",
        "https://example.com/callback?code=test_code&state=wrong_state",
        "https://r",
    )
    .await;

    let err = result.expect_err("state mismatch should fail before token exchange");
    assert!(
        err.to_string().contains("OAuth state mismatch"),
        "unexpected error: {err}"
    );
    Ok(())
}

#[test]
fn openai_docs_reference_current_callback_uri() -> Result<()> {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected = openai::default_redirect_uri();
    for relative in ["OAUTH.md", "README.md"] {
        let content = std::fs::read_to_string(repo_root.join(relative))?;
        assert!(
            content.contains(&expected),
            "{relative} should mention current OpenAI callback URI {expected}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn openai_callback_input_rejects_state_mismatch() -> Result<()> {
    let err = exchange_openai_callback_input(
        "verifier",
        "http://localhost:1455/auth/callback?code=abc123&state=wrong_state",
        "expected_state",
        "http://localhost:1455/auth/callback",
    )
    .await
    .expect_err("state mismatch should fail before token exchange");

    assert!(
        err.to_string().contains("OAuth state mismatch"),
        "unexpected error: {err}"
    );
    Ok(())
}

#[tokio::test]
async fn claude_exchange_falls_back_to_verifier_when_input_has_no_state() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let _ = exchange_claude_code_at_url(&url, "verifier_only", "plain_code", "https://r").await?;

    let (_method, _path, _headers, body) = handle.await.map_err(|e| anyhow!(e))?;
    let body: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(require_json_str(&body, "state")?, "verifier_only");
    assert_eq!(require_json_str(&body, "code")?, "plain_code");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_uses_verifier_when_input_state_is_empty() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let _ = exchange_claude_code_at_url(&url, "verifier_only", "plain_code#", "https://r").await?;

    let (_method, _path, _headers, body) = handle.await.map_err(|e| anyhow!(e))?;
    let body: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(require_json_str(&body, "state")?, "verifier_only");
    Ok(())
}

#[tokio::test]
async fn claude_exchange_mock_server_error_propagates() -> Result<()> {
    let error_body =
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid"}}"#;
    let (port, _handle) = mock_token_server(400, error_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let result = exchange_code_at_url(&url, "c", "v", "https://r", None).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Token exchange failed"));
    Ok(())
}

// ========================
// Mock server integration: Claude refresh
// ========================

#[tokio::test]
async fn claude_refresh_mock_server_receives_json() -> Result<()> {
    let success_body = serde_json::json!({
        "access_token": "at_refreshed",
        "refresh_token": "rt_refreshed",
        "expires_in": 7200
    })
    .to_string();
    let (port, handle) = mock_token_server(200, &success_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let result = refresh_tokens_at_url(&url, "old_refresh_token").await?;

    assert_eq!(result.access_token, "at_refreshed");
    assert_eq!(result.refresh_token, "rt_refreshed");

    let (method, _path, headers, body) = handle.await.map_err(|e| anyhow!(e))?;
    assert_eq!(method, "POST");
    assert_eq!(
        headers
            .get("content-type")
            .map(String::as_str)
            .ok_or_else(|| anyhow!("missing content-type header"))?,
        "application/json"
    );
    let body: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(require_json_str(&body, "grant_type")?, "refresh_token");
    assert_eq!(
        require_json_str(&body, "refresh_token")?,
        "old_refresh_token"
    );
    assert_eq!(require_json_str(&body, "client_id")?, claude::CLIENT_ID);
    assert_eq!(require_json_str(&body, "scope")?, claude::REFRESH_SCOPES);
    Ok(())
}

#[tokio::test]
async fn claude_refresh_mock_server_error_propagates() -> Result<()> {
    let error_body = r#"{"error":"invalid_grant"}"#;
    let (port, _handle) = mock_token_server(400, error_body).await;

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let result = refresh_tokens_at_url(&url, "expired_token").await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Token refresh failed")
    );
    Ok(())
}

// ========================
// Regression: Claude Code now sends JSON token exchange bodies
// ========================

#[tokio::test]
async fn claude_json_body_accepted_by_strict_server() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| anyhow!(e))?;
    let port = listener.local_addr().map_err(|e| anyhow!(e))?.port();

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.map_err(|e| anyhow!(e))?;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .map_err(|e| anyhow!(e))?;

        let mut content_type = String::new();
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.map_err(|e| anyhow!(e))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                let k = k.trim().to_lowercase();
                if k == "content-type" {
                    content_type = v.trim().to_string();
                }
                if k == "content-length" {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body).await.map_err(|e| anyhow!(e))?;
        }

        if !content_type.contains("application/json") {
            let error_resp = r#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid request format"}}"#;
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                error_resp.len(),
                error_resp
            );
            writer
                .write_all(response.as_bytes())
                .await
                .map_err(|e| anyhow!(e))?;
            return Ok::<bool, anyhow::Error>(false);
        }

        let success = serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "expires_in": 3600
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            success.len(),
            success
        );
        writer
            .write_all(response.as_bytes())
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(true)
    });

    let url = format!("http://127.0.0.1:{}/v1/oauth/token", port);
    let result = exchange_code_at_url(&url, "code", "verifier", "https://redir", None).await;

    let server_accepted = handle.await.map_err(|e| anyhow!(e))??;
    assert!(
        server_accepted,
        "Server should have accepted the Claude Code-style JSON request"
    );
    assert!(result.is_ok(), "Exchange should succeed with JSON");
    Ok(())
}

// ========================
// Token response parsing
// ========================

#[tokio::test]
async fn exchange_parses_optional_id_token() -> Result<()> {
    let body_with = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600,
        "id_token": "idt_value"
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &body_with).await;
    let url = format!("http://127.0.0.1:{}/token", port);
    let result = exchange_code_at_url(&url, "c", "v", "r", None).await?;
    assert_eq!(result.id_token, Some("idt_value".to_string()));
    Ok(())
}

#[tokio::test]
async fn exchange_handles_missing_id_token() -> Result<()> {
    let body_without = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &body_without).await;
    let url = format!("http://127.0.0.1:{}/token", port);
    let result = exchange_code_at_url(&url, "c", "v", "r", None).await?;
    assert!(result.id_token.is_none());
    Ok(())
}

#[tokio::test]
async fn exchange_sets_expires_at_in_future() -> Result<()> {
    let body = serde_json::json!({
        "access_token": "at",
        "refresh_token": "rt",
        "expires_in": 3600
    })
    .to_string();
    let (port, _handle) = mock_token_server(200, &body).await;
    let url = format!("http://127.0.0.1:{}/token", port);
    let before = chrono::Utc::now().timestamp_millis();
    let result = exchange_code_at_url(&url, "c", "v", "r", None).await?;
    let after = chrono::Utc::now().timestamp_millis();
    assert!(result.expires_at >= before + 3600 * 1000);
    assert!(result.expires_at <= after + 3600 * 1000);
    Ok(())
}

// ========================
// Special characters / URL encoding
// ========================

#[test]
fn claude_exchange_handles_special_chars_in_code() -> Result<()> {
    let (_url, _ct, body) = build_claude_exchange_request(
        "code+with/special=chars&more",
        "verifier",
        claude::REDIRECT_URI,
        None,
    );
    let body = json_body(body)?;
    assert_eq!(
        require_json_str(&body, "code")?,
        "code+with/special=chars&more"
    );
    Ok(())
}

#[test]
fn openai_redirect_uri_format() {
    let uri = openai::redirect_uri(1455);
    assert_eq!(uri, "http://localhost:1455/auth/callback");
    let uri2 = openai::redirect_uri(9999);
    assert_eq!(uri2, "http://localhost:9999/auth/callback");
}

// ========================
// Provider token request content types match their upstream CLIs.
// ========================

#[test]
fn token_requests_use_expected_content_types() {
    let checks: Vec<(&str, String, &str)> = vec![
        (
            "claude_exchange",
            build_claude_exchange_request("c", "v", "r", None).1,
            "application/json",
        ),
        (
            "claude_exchange_with_state",
            build_claude_exchange_request("c", "v", "r", Some("s")).1,
            "application/json",
        ),
        (
            "claude_refresh",
            build_claude_refresh_request("rt").1,
            "application/json",
        ),
        (
            "openai_exchange",
            build_openai_exchange_request("c", "v", "r").1,
            "application/x-www-form-urlencoded",
        ),
        (
            "openai_refresh",
            build_openai_refresh_request("rt").1,
            "application/x-www-form-urlencoded",
        ),
    ];
    for (name, ct, expected) in checks {
        assert_eq!(ct, expected, "{name} must use {expected}, got {ct}");
    }
}
