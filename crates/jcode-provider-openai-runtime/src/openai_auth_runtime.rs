//! Credential refresh and cancellation-safe commit for Responses requests.
use super::*;

pub(in super::super) async fn openai_access_token(
    account_label: Option<String>,
    credentials: &Arc<RwLock<CodexCredentials>>,
) -> anyhow::Result<String> {
    if let Some(label) = account_label.as_deref() {
        let is_oauth = {
            let tokens = credentials.read().await;
            !tokens.refresh_token.is_empty() || tokens.id_token.is_some()
        };
        if is_oauth {
            // Cloud handoff/login can replace the file while this daemon keeps
            // its provider alive. Never send or refresh the old cached copy.
            let mut tokens = credentials.write().await;
            *tokens = jcode_base::auth::codex::load_credentials_for_account(label)?;
        }
    }
    let (access_token, refresh_token, needs_refresh, is_oauth) = {
        let tokens = credentials.read().await;
        if tokens.access_token.is_empty() {
            anyhow::bail!("OpenAI access token is empty");
        }

        let should_refresh = if let Some(expires_at) = tokens.expires_at {
            expires_at < chrono::Utc::now().timestamp_millis() + 300_000
                && !tokens.refresh_token.is_empty()
        } else {
            false
        };

        (
            tokens.access_token.clone(),
            tokens.refresh_token.clone(),
            should_refresh,
            !tokens.refresh_token.is_empty() || tokens.id_token.is_some(),
        )
    };

    if is_oauth && let Some(label) = account_label.as_deref() {
        // A deleted explicit login must not continue using its cached bearer.
        jcode_base::auth::codex::load_credentials_for_account(label)?;
    }

    if !needs_refresh {
        return Ok(access_token);
    }

    if refresh_token.is_empty() {
        return Ok(access_token);
    }

    force_refresh_openai_token(credentials, &refresh_token, account_label).await
}

/// Unconditionally refresh the OpenAI access token using the stored refresh
/// token, persisting the rotated credentials in place. Used when the server
/// rejects the current access token (401/403) even though it had not yet hit
/// its local expiry window.
pub(in super::super) async fn force_refresh_openai_token(
    credentials: &Arc<RwLock<CodexCredentials>>,
    refresh_token: &str,
    account_label: Option<String>,
) -> anyhow::Result<String> {
    // Rotation, persistence, and the in-memory commit form one transaction.
    // It runs as its own task so a cancelled caller (dropped stream, aborted
    // turn) cannot strand rotated tokens on disk while this provider keeps the
    // consumed refresh token in memory. The network wait inside is bounded by
    // `oauth::OPENAI_REFRESH_TIMEOUT`.
    let refresh_token = refresh_token.to_string();
    refresh_and_commit_openai_tokens(credentials, async move {
        match account_label {
            Some(label) => oauth::refresh_openai_tokens_for_account(&refresh_token, &label).await,
            None => oauth::refresh_openai_tokens(&refresh_token).await,
        }
    })
    .await
}

/// Run `refresh` (rotation plus persistence) and the in-memory commit as one
/// task that survives cancellation of the caller.
pub(in super::super) async fn refresh_and_commit_openai_tokens(
    credentials: &Arc<RwLock<CodexCredentials>>,
    refresh: impl std::future::Future<Output = anyhow::Result<oauth::OAuthTokens>> + Send + 'static,
) -> anyhow::Result<String> {
    let credentials = Arc::clone(credentials);
    oauth::run_refresh_transaction(async move {
        let refreshed = refresh.await?;
        Ok(commit_refreshed_openai_tokens(&credentials, refreshed).await)
    })
    .await
}

async fn commit_refreshed_openai_tokens(
    credentials: &Arc<RwLock<CodexCredentials>>,
    refreshed: oauth::OAuthTokens,
) -> String {
    let mut tokens = credentials.write().await;
    let account_id = tokens.account_id.clone();
    let id_token = refreshed
        .id_token
        .clone()
        .or_else(|| tokens.id_token.clone());
    let new_access_token = refreshed.access_token.clone();

    *tokens = CodexCredentials {
        access_token: new_access_token.clone(),
        refresh_token: refreshed.refresh_token,
        id_token,
        account_id,
        expires_at: Some(refreshed.expires_at),
    };

    new_access_token
}
