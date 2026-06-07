#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailureReason {
    BrowserOpenFailed,
    CallbackTimeout,
    CallbackPortUnavailable,
    NonInteractiveTerminal,
    ManualInputMissing,
    SaveFailed,
    PostLoginValidationFailed,
    ImportUnavailable,
    ExternalCliFailed,
    DeviceFlowFailed,
    RateLimited,
    OAuthExchangeFailed,
    Unknown,
}

impl AuthFailureReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::BrowserOpenFailed => "browser_open_failed",
            Self::CallbackTimeout => "callback_timeout",
            Self::CallbackPortUnavailable => "callback_port_unavailable",
            Self::NonInteractiveTerminal => "non_interactive_terminal",
            Self::ManualInputMissing => "manual_input_missing",
            Self::SaveFailed => "save_failed",
            Self::PostLoginValidationFailed => "post_login_validation_failed",
            Self::ImportUnavailable => "import_unavailable",
            Self::ExternalCliFailed => "external_cli_failed",
            Self::DeviceFlowFailed => "device_flow_failed",
            Self::RateLimited => "rate_limited",
            Self::OAuthExchangeFailed => "oauth_exchange_failed",
            Self::Unknown => "unknown",
        }
    }
}

pub fn classify_auth_failure_message(message: &str) -> AuthFailureReason {
    let lower = message.trim().to_ascii_lowercase();

    if lower.contains("timed out waiting for callback") || lower.contains("callback timeout") {
        AuthFailureReason::CallbackTimeout
    } else if lower.contains("callback port") && lower.contains("unavailable")
        || lower.contains("failed to bind callback")
        || lower.contains("address already in use")
    {
        AuthFailureReason::CallbackPortUnavailable
    } else if lower.contains("couldn't open a browser")
        || lower.contains("failed to open browser")
        || lower.contains("no browser on this machine")
        || lower.contains("browser didn't open")
    {
        AuthFailureReason::BrowserOpenFailed
    } else if lower.contains("interactive terminal") || lower.contains("stdin") {
        AuthFailureReason::NonInteractiveTerminal
    } else if lower.contains("no authorization code entered")
        || lower.contains("no callback url entered")
        || lower.contains("api key cannot be empty")
        || lower.contains("no api key provided")
    {
        AuthFailureReason::ManualInputMissing
    } else if lower.contains("failed to save") || lower.contains("permission denied") {
        AuthFailureReason::SaveFailed
    } else if lower.contains("post-login validation failed")
        || lower.contains("could not verify runtime readiness")
    {
        AuthFailureReason::PostLoginValidationFailed
    } else if lower.contains("no existing logins were imported") {
        AuthFailureReason::ImportUnavailable
    } else if lower.contains("device flow failed") {
        AuthFailureReason::DeviceFlowFailed
    } else if lower.contains("rate_limit_error")
        || lower.contains("rate limited")
        || lower.contains("too many requests")
        || lower.contains("http 429")
        || lower.contains("status 429")
    {
        AuthFailureReason::RateLimited
    } else if lower.contains("cli login failed")
        || lower.contains("command exited with non-zero status")
        || lower.contains("failed to start command")
    {
        AuthFailureReason::ExternalCliFailed
    } else if lower.contains("oauth")
        || lower.contains("exchange") && lower.contains("token")
        || lower.contains("authorization code") && !lower.contains("entered")
    {
        AuthFailureReason::OAuthExchangeFailed
    } else {
        AuthFailureReason::Unknown
    }
}

pub fn auth_failure_recovery_hint(provider_id: &str, reason: AuthFailureReason) -> Option<String> {
    let provider = provider_id.trim();
    if provider.is_empty() {
        return None;
    }

    let hint = match reason {
        AuthFailureReason::BrowserOpenFailed
        | AuthFailureReason::CallbackTimeout
        | AuthFailureReason::CallbackPortUnavailable
        | AuthFailureReason::NonInteractiveTerminal => format!(
            "Try a manual-safe fallback: `jcode login --provider {} --print-auth-url`, then complete with `--callback-url` or `--auth-code`.",
            provider
        ),
        AuthFailureReason::ManualInputMissing => {
            "Retry the same flow and paste the full callback URL, authorization code, or required API key when prompted.".to_string()
        }
        AuthFailureReason::SaveFailed => {
            "Check whether jcode can write its config directory, or retry inside an isolated sandbox with `bash scripts/onboarding_sandbox.sh fresh`.".to_string()
        }
        AuthFailureReason::PostLoginValidationFailed => format!(
            "Credentials were saved, but runtime verification failed. Run `jcode auth-test --provider {}` and `jcode auth doctor {}` for guided diagnosis.",
            provider, provider
        ),
        AuthFailureReason::ImportUnavailable => {
            "No reusable external login was available. Use a direct login flow instead of auto-import, or approve the detected external auth source explicitly.".to_string()
        }
        AuthFailureReason::ExternalCliFailed => {
            "The external tool login did not complete. Retry it directly, or switch to the provider's API key/manual login path.".to_string()
        }
        AuthFailureReason::DeviceFlowFailed => {
            "Retry the device-code flow, or switch to another supported auth method if available.".to_string()
        }
        AuthFailureReason::RateLimited => format!(
            "The provider accepted the browser callback but rate-limited the token exchange. Wait before retrying, avoid repeated immediate attempts, and keep using existing credentials if they still validate with `jcode auth doctor {} --validate`.",
            provider
        ),
        AuthFailureReason::OAuthExchangeFailed => format!(
            "Retry the OAuth flow, and if it keeps failing use `jcode login --provider {} --print-auth-url` so the callback can be completed manually.",
            provider
        ),
        AuthFailureReason::Unknown => {
            "Run `jcode auth status`, then `jcode auth doctor` for a structured diagnosis.".to_string()
        }
    };

    Some(hint)
}

pub fn augment_auth_error_message(provider_id: &str, message: impl AsRef<str>) -> String {
    let message = message.as_ref().trim();
    let reason = classify_auth_failure_message(message);
    if let Some(hint) = auth_failure_recovery_hint(provider_id, reason) {
        format!("{}\n\nNext step: {}", message, hint)
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_callback_timeout() {
        assert_eq!(
            classify_auth_failure_message("Timed out waiting for callback. Falling back."),
            AuthFailureReason::CallbackTimeout
        );
    }

    #[test]
    fn classifies_validation_failure() {
        assert_eq!(
            classify_auth_failure_message("Post-login validation failed for OpenAI."),
            AuthFailureReason::PostLoginValidationFailed
        );
    }

    #[test]
    fn classifies_oauth_rate_limit() {
        assert_eq!(
            classify_auth_failure_message(
                r#"Token exchange failed: {"error":{"type":"rate_limit_error","message":"Rate limited. Please try again later."}}"#,
            ),
            AuthFailureReason::RateLimited
        );
    }

    #[test]
    fn augments_message_with_next_step() {
        let message =
            augment_auth_error_message("openai", "Couldn't open a browser on this machine.");
        assert!(message.contains("Next step:"));
        assert!(message.contains("--print-auth-url"));
    }
}
