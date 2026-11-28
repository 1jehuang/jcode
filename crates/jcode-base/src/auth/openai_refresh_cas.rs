use super::*;

pub(super) async fn refresh_openai_tokens_with(
    refresh_token: &str,
    label: Option<&str>,
    request: impl std::future::Future<Output = Result<OAuthTokens>>,
) -> Result<OAuthTokens> {
    let result = async {
        let expected = label
            .map(|label| crate::auth::codex::capture_refresh_account(label, refresh_token))
            .transpose()?;
        let tokens = request.await?;
        if let Some(expected) = expected {
            crate::auth::codex::commit_refresh_account(&expected, &tokens)?;
        } else {
            crate::logging::info(
                "Refreshed OpenAI/Codex tokens from an external source without storing them in jcode auth",
            );
        }
        Ok(tokens)
    }
    .await;
    // Preserve the shared terminal rejection state for permanently invalid grants.
    crate::auth::refresh_state::record_refresh_outcome("openai", refresh_token, &result);
    result
}

#[cfg(test)]
#[path = "openai_refresh_cas_tests.rs"]
mod tests;
