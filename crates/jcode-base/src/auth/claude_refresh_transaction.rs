use super::*;

pub(super) async fn refresh_claude_tokens_with(
    refresh_token: &str,
    label: &str,
    request: impl std::future::Future<Output = Result<OAuthTokens>>,
) -> Result<OAuthTokens> {
    let expected = claude_auth::capture_refresh_account(label, refresh_token)?;
    let tokens = request.await?;
    claude_auth::commit_captured_refresh(label, expected.as_ref(), refresh_token, &tokens)?;
    Ok(tokens)
}

#[cfg(test)]
#[path = "claude_refresh_transaction_tests.rs"]
mod tests;
