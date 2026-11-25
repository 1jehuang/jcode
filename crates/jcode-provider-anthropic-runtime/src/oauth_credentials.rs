use super::*;

pub(super) async fn resolve_oauth_access_token(
    credentials: &Arc<RwLock<Option<CachedCredentials>>>,
    account_label: &Option<String>,
) -> Result<(String, bool)> {
    let cached = credentials.read().await;
    let (fresh_creds, refresh_label) = load_scoped_claude_credentials(account_label)?;
    ensure_inference_scope(&fresh_creds)?;
    let source = (
        fresh_creds.access_token.clone(),
        fresh_creds.refresh_token.clone(),
    );
    if let Some(creds) = cached.as_ref() {
        let now = chrono::Utc::now().timestamp_millis();
        if creds.source == source && creds.expires_at > now + 300_000 {
            return Ok((creds.access_token.clone(), true));
        }
    }
    drop(cached);

    let now = chrono::Utc::now().timestamp_millis();

    // A refresh token the provider already rejected as unrecoverable cannot
    // start working again. Retrying it added a doomed network round-trip to
    // the critical path of every single turn, so fail straight through to the
    // caller's API-key fallback instead. Rejection blocks refresh, not a
    // still-live access token, which falls through to be cached and used.
    let rejected = !fresh_creds.refresh_token.is_empty()
        && auth::refresh_state::refresh_token_is_known_rejected(
            "claude",
            &fresh_creds.refresh_token,
        );
    if rejected && fresh_creds.expires_at <= now {
        anyhow::bail!(
            "Claude OAuth refresh token was previously rejected by Anthropic and cannot be refreshed. Run `jcode login --provider claude` to mint a fresh token."
        );
    }

    // Check if token needs refresh (expired or expiring within 5 minutes)
    if !rejected && fresh_creds.expires_at < now + 300_000 && !fresh_creds.refresh_token.is_empty()
    {
        jcode_base::logging::info("OAuth token expired or expiring soon, attempting refresh...");

        match oauth::refresh_claude_tokens_for_account(&fresh_creds.refresh_token, &refresh_label)
            .await
        {
            Ok(refreshed) => {
                jcode_base::logging::info("OAuth token refreshed successfully");
                return Ok((
                    commit_claude_refresh(credentials, account_label, source, refreshed).await?,
                    true,
                ));
            }
            Err(e) => {
                if let Some(current) = replacement_after_refresh_failure(account_label, &source)? {
                    return Ok((current, true));
                }
                jcode_base::logging::error(&format!("OAuth token refresh failed: {}", e));
                // A still-unexpired token may remain usable until its
                // actual expiry even when a proactive refresh fails. Once
                // expired, however, returning it only guarantees a 401 and
                // prevents Auto mode from trying its API-key fallback.
                if fresh_creds.expires_at <= now {
                    anyhow::bail!("Claude OAuth token is expired and refresh failed: {e:#}");
                }
            }
        }
    }

    if fresh_creds.expires_at <= now {
        anyhow::bail!(
            "Claude OAuth token is expired and no usable refresh token is available. Run `jcode login --provider claude` to refresh OAuth credentials"
        );
    }

    // A competing request may have held the cache lock while the login changed.
    let mut cached = credentials.write().await;
    let (current, _) = load_scoped_claude_credentials(account_label)?;
    ensure_inference_scope(&current)?;
    anyhow::ensure!(
        current.expires_at > chrono::Utc::now().timestamp_millis(),
        "Claude login changed during token resolution; retry with the current login"
    );
    *cached = Some(CachedCredentials {
        access_token: current.access_token.clone(),
        refresh_token: current.refresh_token.clone(),
        expires_at: current.expires_at,
        source: (current.access_token.clone(), current.refresh_token),
    });
    Ok((current.access_token, true))
}

pub(super) async fn force_refresh_oauth_token(
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
    account_label: &Option<String>,
) -> Result<String> {
    // Always start from the stored credential: after a relogin or account
    // switch the cache still holds the previous account's refresh token, and
    // refreshing that would mint a token for the wrong account.
    let (loaded, refresh_label, source) = {
        let mut cached = credentials.write().await;
        let (loaded, refresh_label) = load_scoped_claude_credentials(account_label)
            .context("Failed to load Claude credentials for forced refresh")?;
        ensure_inference_scope(&loaded)?;
        let source = (loaded.access_token.clone(), loaded.refresh_token.clone());
        let now = chrono::Utc::now().timestamp_millis();
        // Join a rotation/login that happened while waiting for the cache lock.
        if cached
            .as_ref()
            .is_some_and(|cached| cached.source != source)
            && !loaded.access_token.is_empty()
            && loaded.expires_at > now + 300_000
        {
            *cached = Some(CachedCredentials {
                access_token: loaded.access_token.clone(),
                refresh_token: loaded.refresh_token.clone(),
                expires_at: loaded.expires_at,
                source,
            });
            return Ok(loaded.access_token);
        }
        (loaded, refresh_label, source)
    };
    // A token we refreshed ourselves (e.g. from an external source that is not
    // rewritten on refresh) rotates the refresh token, so prefer the cached one
    // only while it still derives from the stored credential.
    let refresh_from_cache = {
        let cached = credentials.read().await;
        cached
            .as_ref()
            .filter(|c| c.source == source)
            .map(|c| c.refresh_token.clone())
            .filter(|t| !t.is_empty())
    };
    let refresh_token = match refresh_from_cache {
        Some(token) => token,
        None if loaded.refresh_token.is_empty() => {
            anyhow::bail!("No refresh token available in Claude credentials");
        }
        None => loaded.refresh_token,
    };

    let refreshed =
        match oauth::refresh_claude_tokens_for_account(&refresh_token, &refresh_label).await {
            Ok(refreshed) => refreshed,
            Err(err) => {
                if let Some(current) = replacement_after_refresh_failure(account_label, &source)? {
                    return Ok(current);
                }
                anyhow::bail!("OAuth refresh endpoint rejected the refresh token: {err:#}");
            }
        };

    commit_claude_refresh(&credentials, account_label, source, refreshed).await
}

/// Explicit bindings fail closed when removed; an unbound runtime follows default resolution.
fn load_scoped_claude_credentials(
    account_label: &Option<String>,
) -> Result<(auth::claude::ClaudeCredentials, String)> {
    let credentials = match account_label.as_deref() {
        Some(label) => auth::claude::load_credentials_for_account(label),
        None => auth::claude::load_credentials(),
    }?;
    let refresh_label = account_label
        .clone()
        .or_else(auth::claude::active_account_label)
        .unwrap_or_else(auth::claude::primary_account_label);
    Ok((credentials, refresh_label))
}

/// Write a finished Claude refresh into the session cache and return the
/// bearer the caller should use.
///
/// The login can change while the refresh awaits the OAuth response. The
/// result is only cached while the stored credential is still the one the
/// refresh started from (an external login that is not rewritten on refresh)
/// or the refreshed tokens themselves (a stored account whose refresh was
/// persisted). Otherwise another login is active now: leave the cache alone
/// and return that login's token instead of the old account's bearer.
async fn commit_claude_refresh(
    credentials: &Arc<RwLock<Option<CachedCredentials>>>,
    account_label: &Option<String>,
    source: (String, String),
    refreshed: oauth::OAuthTokens,
) -> Result<String> {
    let mut cached = credentials.write().await;
    let (stored, _) = load_scoped_claude_credentials(account_label)?;
    ensure_inference_scope(&stored)?;
    let stored_source = (stored.access_token.clone(), stored.refresh_token.clone());
    let persisted_refresh = stored.access_token == refreshed.access_token
        && stored.refresh_token == refreshed.refresh_token;
    if stored_source != source && !persisted_refresh {
        anyhow::ensure!(
            !stored.access_token.is_empty()
                && stored.expires_at > chrono::Utc::now().timestamp_millis(),
            "Claude login changed during refresh; retry with the current login"
        );
        ensure_inference_scope(&stored)?;
        return Ok(stored.access_token);
    }
    *cached = Some(CachedCredentials {
        access_token: refreshed.access_token.clone(),
        refresh_token: refreshed.refresh_token,
        expires_at: refreshed.expires_at,
        source,
    });
    Ok(refreshed.access_token)
}

// A stale disk refresh can be rejected by CAS after a newer login took over.
fn replacement_after_refresh_failure(
    account_label: &Option<String>,
    source: &(String, String),
) -> Result<Option<String>> {
    let (current, _) = load_scoped_claude_credentials(account_label)?;
    if current.access_token == source.0 && current.refresh_token == source.1 {
        return Ok(None);
    }
    ensure_inference_scope(&current)?;
    anyhow::ensure!(
        !current.access_token.is_empty()
            && current.expires_at > chrono::Utc::now().timestamp_millis(),
        "Claude login changed during refresh; retry with the current login"
    );
    Ok(Some(current.access_token))
}

fn ensure_inference_scope(credentials: &auth::claude::ClaudeCredentials) -> Result<()> {
    if !credentials.scopes.is_empty() && !oauth::claude_scopes_have_inference(&credentials.scopes) {
        anyhow::bail!(
            "Claude OAuth credentials are missing the required user:inference scope. Run `jcode login --provider claude` to sign in again."
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "oauth_credentials_tests.rs"]
mod tests;
