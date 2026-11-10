//! Fable earned-reset integration boundary.
//!
//! The official launch offer documents redemption in claude.ai Settings > Usage,
//! but does not publish an API contract. The installed Claude Code 2.1.267 only
//! implements the distinct `juniper_tide` five-hour reset program. Reusing that
//! program for Fable would spend the wrong entitlement and leave weekly usage
//! exhausted. See docs/anthropic-fable-resets.md for evidence and prerequisites.

use anyhow::Result;

/// Attempt an earned Fable reset for exactly the requested OAuth account.
///
/// `true` must mean a verified reset cleared the exhausted limit. Until a real
/// Fable contract is available, this returns `false` for every account, even if
/// `provider.anthropic_auto_fable_reset` is enabled. It never loads credentials,
/// falls back to the active account, sends a request, clears usage state, or
/// spends either Fable grants or juniper_tide resets. Consequently concurrent
/// agents and retries cannot cause an uncertain or duplicate spend.
///
/// Do not replace this with the session-limit reset API. Before enabling claims,
/// implement fresh account-bound exhaustion checks, cross-process serialization,
/// and durable suppression of uncertain redemptions using the verified contract.
pub(crate) async fn auto_reset_fable_usage(_account_label: Option<String>) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fable_reset_fails_closed_for_default_and_explicit_accounts() {
        for label in [None, Some("missing-account-no-fallback".to_string())] {
            assert!(!auto_reset_fable_usage(label).await.unwrap());
        }
    }

    #[tokio::test]
    async fn concurrent_and_repeated_fable_calls_never_report_a_reset() {
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..32 {
            calls.spawn(auto_reset_fable_usage(Some("same-account".to_string())));
        }
        while let Some(result) = calls.join_next().await {
            assert!(!result.unwrap().unwrap());
        }
        assert!(
            !auto_reset_fable_usage(Some("same-account".to_string()))
                .await
                .unwrap()
        );
    }

    #[test]
    fn fable_automatic_reset_is_independently_opt_in() {
        let config = crate::config::ProviderConfig::default();
        assert!(!config.anthropic_auto_fable_reset);
    }
}
