use super::*;

/// Build a Claude token exchange request (extracted for testability).
/// Returns (url, content_type, body_bytes).
#[cfg(test)]
pub(super) fn build_claude_exchange_request(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    state: Option<&str>,
) -> (String, String, Vec<u8>) {
    let effective_state = state.unwrap_or(verifier);
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
        "client_id": claude::CLIENT_ID,
        "code_verifier": verifier,
        "state": effective_state,
    });
    (
        claude::TOKEN_URL.to_string(),
        "application/json".to_string(),
        serde_json::to_vec(&body).expect("Claude exchange test body should serialize"),
    )
}

/// Build a Claude token refresh request (extracted for testability).
#[cfg(test)]
pub(super) fn build_claude_refresh_request(refresh_token: &str) -> (String, String, Vec<u8>) {
    build_claude_refresh_request_with_scope(refresh_token, Some(claude::REFRESH_SCOPES))
}

/// Build a Claude token refresh request with configurable scope (extracted for testability).
#[cfg(test)]
pub(super) fn build_claude_refresh_request_with_scope(
    refresh_token: &str,
    scope: Option<&'static str>,
) -> (String, String, Vec<u8>) {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": claude::CLIENT_ID,
    });
    let mut body = body.as_object().expect("refresh body object").clone();
    if let Some(scope) = scope {
        body.insert(
            "scope".to_string(),
            serde_json::Value::String(scope.to_string()),
        );
    }
    (
        claude::TOKEN_URL.to_string(),
        "application/json".to_string(),
        serde_json::to_vec(&body).expect("Claude refresh test body should serialize"),
    )
}

/// Build an OpenAI token exchange request (extracted for testability).
#[cfg(test)]
pub(super) fn build_openai_exchange_request(
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> (String, String, Vec<u8>) {
    let body = format!(
        "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
        openai::CLIENT_ID,
        code,
        verifier,
        urlencoding::encode(redirect_uri)
    );
    (
        openai::TOKEN_URL.to_string(),
        "application/x-www-form-urlencoded".to_string(),
        body.into_bytes(),
    )
}

/// Build an OpenAI token refresh request (extracted for testability).
#[cfg(test)]
pub(super) fn build_openai_refresh_request(refresh_token: &str) -> (String, String, Vec<u8>) {
    let body = format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        openai::CLIENT_ID,
        urlencoding::encode(refresh_token)
    );
    (
        openai::TOKEN_URL.to_string(),
        "application/x-www-form-urlencoded".to_string(),
        body.into_bytes(),
    )
}

/// Exchange an auth code for tokens against a configurable URL.
/// Used by tests with a mock server.
#[cfg(test)]
pub(super) async fn exchange_code_at_url(
    token_url: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    state: Option<&str>,
) -> Result<OAuthTokens> {
    let effective_state = state.unwrap_or(verifier);
    let payload = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
        "client_id": claude::CLIENT_ID,
        "code_verifier": verifier,
        "state": effective_state,
    });

    let client = crate::provider::shared_http_client();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token exchange failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        id_token: Option<String>,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    Ok(OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: tokens.id_token,
        scopes: Vec::new(),
    })
}

/// Refresh tokens against a configurable URL.
/// Used by tests with a mock server.
#[cfg(test)]
pub(super) async fn refresh_tokens_at_url(
    token_url: &str,
    refresh_token: &str,
) -> Result<OAuthTokens> {
    let payload = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": claude::CLIENT_ID,
        "scope": claude::REFRESH_SCOPES,
    });

    let client = crate::provider::shared_http_client();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await?;
        anyhow::bail!("Token refresh failed: {}", text);
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    }

    let tokens: TokenResponse = resp.json().await?;
    let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

    Ok(OAuthTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        id_token: None,
        scopes: Vec::new(),
    })
}
