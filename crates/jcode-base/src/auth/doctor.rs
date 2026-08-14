use crate::auth::{AuthRefreshSupport, AuthState, ProviderAuthAssessment};
use crate::provider_catalog::{LoginProviderAuthKind, LoginProviderDescriptor};

pub const VALIDATION_STALE_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// True when `jcode provider-doctor` has a native-runtime driver for
/// `provider_id` (a provider whose live path is not OpenAI-compatible and so
/// cannot be exercised by the generic OpenAI-compatible doctor). Today this is
/// the Claude OAuth/subscription provider, the Antigravity (Google OAuth Cloud
/// Code) provider, and the generic native-runtime providers (OpenAI, Gemini,
/// Cursor, Copilot, Bedrock, jcode, Azure OpenAI).
///
/// The drivers themselves live downstream in the `jcode-provider-doctor`
/// crate (which re-exports this predicate); this roster lives here so
/// `live_tests` can annotate the monitoring roster without depending on that
/// crate. A sync test in `jcode-provider-doctor` asserts this list matches its
/// `NativeProviderKind` specs.
pub fn native_doctor_supports_provider(provider_id: &str) -> bool {
    matches!(
        crate::auth::lifecycle::normalized_auth_provider_id(Some(provider_id)),
        Some(
            "claude"
                | "antigravity"
                | "openai"
                | "gemini"
                | "cursor"
                | "copilot"
                | "bedrock"
                | "jcode"
                | "azure-openai"
        )
    )
}

pub fn validation_is_stale(checked_at_ms: i64) -> bool {
    let now_ms = chrono::Utc::now().timestamp_millis();
    now_ms.saturating_sub(checked_at_ms) > VALIDATION_STALE_AFTER_MS
}

pub fn needs_attention(
    assessment: &ProviderAuthAssessment,
    validation_result: Option<&str>,
) -> bool {
    assessment.state != AuthState::Available
        || assessment
            .last_refresh
            .as_ref()
            .and_then(|record| record.last_error.as_deref())
            .is_some()
        || assessment
            .last_validation
            .as_ref()
            .is_none_or(|record| !record.success || validation_is_stale(record.checked_at_ms))
        || validation_result.is_some_and(|result| result != "validation passed")
}

pub fn diagnostics(
    provider: LoginProviderDescriptor,
    assessment: &ProviderAuthAssessment,
    validation_result: Option<&str>,
) -> Vec<String> {
    let mut diagnostics = Vec::new();

    match assessment.state {
        AuthState::NotConfigured => diagnostics.push(format!(
            "{} is not configured for jcode yet.",
            provider.display_name
        )),
        AuthState::Expired => diagnostics.push(format!(
            "{} has credentials, but they are expired or incomplete.",
            provider.display_name
        )),
        AuthState::Available => {}
    }

    if let Some(record) = assessment.last_refresh.as_ref()
        && let Some(error) = record.last_error.as_deref()
    {
        diagnostics.push(format!("Last credential refresh failed: {}", error));
    }

    if assessment.state != AuthState::NotConfigured {
        match assessment.last_validation.as_ref() {
            None => diagnostics.push("No runtime validation has been recorded.".to_string()),
            Some(record) if !record.success => {
                diagnostics.push(format!(
                    "Last runtime validation failed: {}",
                    record.summary
                ));
            }
            Some(record) if validation_is_stale(record.checked_at_ms) => {
                diagnostics.push(format!(
                    "Last runtime validation is stale: {}",
                    record.summary
                ));
            }
            Some(_) => {}
        }
    }

    if let Some(result) = validation_result
        && result != "validation passed"
    {
        diagnostics.push(format!("Current validation run failed: {}", result));
    }

    diagnostics.dedup();
    diagnostics
}

pub fn recommended_actions(
    provider: LoginProviderDescriptor,
    assessment: &ProviderAuthAssessment,
    validation_result: Option<&str>,
) -> Vec<String> {
    let mut actions = Vec::new();
    match assessment.state {
        AuthState::NotConfigured => actions.push(format!(
            "Connect it: jcode login --provider {}",
            provider.id
        )),
        AuthState::Expired
            if matches!(
                assessment.refresh_support,
                AuthRefreshSupport::ManualRelogin
            ) =>
        {
            actions.push(format!(
                "Re-run login; this provider cannot auto-refresh: jcode login --provider {}",
                provider.id
            ));
        }
        AuthState::Expired => actions.push(format!(
            "Refresh or replace the current login: jcode login --provider {}",
            provider.id
        )),
        AuthState::Available => {}
    }

    if let Some(error) = assessment
        .last_refresh
        .as_ref()
        .and_then(|record| record.last_error.as_deref())
    {
        let lower = error.to_ascii_lowercase();
        if lower.contains("invalid_grant") || lower.contains("refresh token") {
            actions.push(format!(
                "Replace the stale OAuth account/token: jcode login --provider {}",
                provider.id
            ));
        } else if lower.contains("rate_limit")
            || lower.contains("rate limited")
            || lower.contains("too many requests")
        {
            actions.push(
                "Wait for the provider rate limit to clear before retrying auth refresh."
                    .to_string(),
            );
        } else {
            actions.push(format!(
                "Retry credential refresh by re-running validation: jcode auth doctor {} --validate",
                provider.id
            ));
        }
    }

    if assessment.state != AuthState::NotConfigured {
        match assessment.last_validation.as_ref() {
            None => actions.push(format!(
                "Run runtime verification: jcode auth-test --provider {}",
                provider.id
            )),
            Some(record) if !record.success => {
                let summary = record.summary.to_ascii_lowercase();
                if summary.contains("402")
                    || summary.contains("payment required")
                    || summary.contains("balance exhausted")
                {
                    actions.push(
                        "Restore the provider's billing or usage balance. Re-authenticating will not fix this failure."
                            .to_string(),
                    );
                } else if summary.contains("429")
                    || summary.contains("quota exhausted")
                    || summary.contains("rate limit")
                    || summary.contains("too many requests")
                {
                    actions.push(
                        "Wait for or increase the provider quota before retrying. Re-authenticating will not fix this failure."
                            .to_string(),
                    );
                } else {
                    actions.push(format!(
                        "Inspect runtime readiness: jcode auth-test --provider {}",
                        provider.id
                    ));
                }
            }
            Some(record) if validation_is_stale(record.checked_at_ms) => actions.push(format!(
                "Refresh stale runtime verification: jcode auth-test --provider {}",
                provider.id
            )),
            Some(_) => {}
        }
    }

    if validation_result.is_some_and(|value| value != "validation passed") {
        actions.push(format!(
            "Re-run detailed auth diagnostics: jcode auth-test --provider {}",
            provider.id
        ));
    }

    if matches!(provider.auth_kind, LoginProviderAuthKind::OAuth)
        || matches!(provider.auth_kind, LoginProviderAuthKind::DeviceCode)
        || matches!(provider.auth_kind, LoginProviderAuthKind::Hybrid)
    {
        actions.push(format!(
            "For browser/callback issues, use the manual-safe flow: jcode login --provider {} --print-auth-url",
            provider.id
        ));
    }

    actions.push("Review current state: jcode auth status --json".to_string());
    actions.dedup();
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthCredentialSource, AuthExpiryConfidence, AuthRefreshSupport, AuthValidationMethod,
    };

    fn base_assessment() -> ProviderAuthAssessment {
        ProviderAuthAssessment {
            state: AuthState::Available,
            readiness: crate::auth::AuthReadinessLevel::RequestValid,
            method_detail: "OAuth".to_string(),
            credential_source: AuthCredentialSource::JcodeManagedFile,
            credential_source_detail: "~/.jcode/auth.json".to_string(),
            expiry_confidence: AuthExpiryConfidence::Exact,
            refresh_support: AuthRefreshSupport::Automatic,
            validation_method: AuthValidationMethod::TimestampCheck,
            last_validation: Some(crate::auth::validation::ProviderValidationRecord {
                checked_at_ms: chrono::Utc::now().timestamp_millis(),
                success: true,
                provider_smoke_ok: Some(true),
                tool_smoke_ok: Some(true),
                summary: "tool_smoke: ok".to_string(),
            }),
            last_refresh: None,
        }
    }

    #[test]
    fn refresh_failure_marks_available_provider_as_needing_attention() {
        let mut assessment = base_assessment();
        assessment.last_refresh = Some(crate::auth::refresh_state::ProviderRefreshRecord {
            last_attempt_ms: chrono::Utc::now().timestamp_millis(),
            last_success_ms: Some(chrono::Utc::now().timestamp_millis() - 1000),
            last_error: Some("invalid_grant: refresh token invalid".to_string()),
            rejected_refresh_fingerprint: None,
        });

        assert!(needs_attention(&assessment, None));
        assert!(
            diagnostics(
                crate::provider_catalog::CLAUDE_LOGIN_PROVIDER,
                &assessment,
                None
            )
            .iter()
            .any(|line| line.contains("Last credential refresh failed"))
        );
        assert!(
            recommended_actions(
                crate::provider_catalog::CLAUDE_LOGIN_PROVIDER,
                &assessment,
                None,
            )
            .iter()
            .any(|line| line.contains("Replace the stale OAuth account/token"))
        );
    }

    #[test]
    fn billing_validation_failure_does_not_recommend_reauthentication() {
        let mut assessment = base_assessment();
        let validation = assessment.last_validation.as_mut().unwrap();
        validation.success = false;
        validation.provider_smoke_ok = Some(false);
        validation.summary = "provider_smoke: API error (status 402 Payment Required): Grok Build usage balance exhausted".to_string();

        let actions = recommended_actions(
            crate::provider_catalog::login_providers()
                .iter()
                .copied()
                .find(|provider| provider.id == "grok-build")
                .unwrap(),
            &assessment,
            None,
        );
        assert!(
            actions
                .iter()
                .any(|action| action.contains("billing or usage balance"))
        );
        assert!(
            actions
                .iter()
                .any(|action| action.contains("Re-authenticating will not fix"))
        );
        assert!(
            !actions
                .iter()
                .any(|action| action.contains("Inspect runtime readiness"))
        );
    }

    #[test]
    fn stale_validation_marks_provider_as_needing_attention() {
        let mut assessment = base_assessment();
        assessment.last_validation = Some(crate::auth::validation::ProviderValidationRecord {
            checked_at_ms: chrono::Utc::now().timestamp_millis() - VALIDATION_STALE_AFTER_MS - 1,
            success: true,
            provider_smoke_ok: Some(true),
            tool_smoke_ok: Some(true),
            summary: "tool_smoke: ok".to_string(),
        });

        assert!(needs_attention(&assessment, None));
        assert!(
            recommended_actions(
                crate::provider_catalog::CLAUDE_LOGIN_PROVIDER,
                &assessment,
                None,
            )
            .iter()
            .any(|line| line.contains("Refresh stale runtime verification"))
        );
    }
}
