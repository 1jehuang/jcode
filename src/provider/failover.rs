use super::*;
use serde::{Deserialize, Serialize};

const PROVIDER_FAILOVER_PROMPT_PREFIX: &str = "[jcode-provider-failover]";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ProviderFailoverPrompt {
    pub from_provider: String,
    pub from_label: String,
    pub to_provider: String,
    pub to_label: String,
    pub reason: String,
    pub estimated_input_chars: usize,
    pub estimated_input_tokens: usize,
}

impl ProviderFailoverPrompt {
    pub(super) fn to_error_message(&self) -> String {
        let payload = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        format!(
            "{PROVIDER_FAILOVER_PROMPT_PREFIX}{payload}\n{} is unavailable; switching to {} would resend about {} input tokens (~{} chars).",
            self.from_label, self.to_label, self.estimated_input_tokens, self.estimated_input_chars,
        )
    }
}

pub(crate) fn parse_failover_prompt_message(message: &str) -> Option<ProviderFailoverPrompt> {
    let line = message.lines().next()?.trim();
    let json = line.strip_prefix(PROVIDER_FAILOVER_PROMPT_PREFIX)?;
    serde_json::from_str(json).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailoverDecision {
    None,
    RetryNextProvider,
    RetryAndMarkUnavailable,
}

impl FailoverDecision {
    pub(super) fn should_failover(self) -> bool {
        !matches!(self, Self::None)
    }

    pub(super) fn should_mark_provider_unavailable(self) -> bool {
        matches!(self, Self::RetryAndMarkUnavailable)
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RetryNextProvider => "retry-next-provider",
            Self::RetryAndMarkUnavailable => "retry-and-mark-unavailable",
        }
    }
}

impl MultiProvider {
    pub(super) fn provider_is_configured(&self, provider: ActiveProvider) -> bool {
        match provider {
            ActiveProvider::Claude => self.has_claude_runtime(),
            ActiveProvider::OpenAI => self.openai_provider().is_some(),
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            ActiveProvider::Antigravity => self.antigravity_provider().is_some(),
            ActiveProvider::Gemini => self.gemini_provider().is_some(),
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            ActiveProvider::OpenRouter => self
                .openrouter
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
        }
    }

    pub(super) fn provider_precheck_unavailable_reason(
        &self,
        provider: ActiveProvider,
    ) -> Option<String> {
        match provider {
            ActiveProvider::Claude if self.is_claude_usage_exhausted() => Some(
                crate::provider::account_failover::usage_exhausted_reason(provider),
            ),
            _ => None,
        }
    }

    pub(super) fn fallback_sequence_for(
        active: ActiveProvider,
        forced_provider: Option<ActiveProvider>,
    ) -> Vec<ActiveProvider> {
        if let Some(forced) = forced_provider {
            vec![forced]
        } else {
            Self::fallback_sequence(active)
        }
    }

    pub(super) fn build_failover_prompt(
        &self,
        from: ActiveProvider,
        to: ActiveProvider,
        reason: String,
        estimated_input_chars: usize,
        estimated_input_tokens: usize,
    ) -> ProviderFailoverPrompt {
        ProviderFailoverPrompt {
            from_provider: Self::provider_key(from).to_string(),
            from_label: Self::provider_label(from).to_string(),
            to_provider: Self::provider_key(to).to_string(),
            to_label: Self::provider_label(to).to_string(),
            reason,
            estimated_input_chars,
            estimated_input_tokens,
        }
    }

    pub(super) fn fallback_sequence(active: ActiveProvider) -> Vec<ActiveProvider> {
        match active {
            ActiveProvider::Claude => {
                vec![
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                    ActiveProvider::Gemini,
                    ActiveProvider::Cursor,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::OpenAI => {
                vec![
                    ActiveProvider::OpenAI,
                    ActiveProvider::Claude,
                    ActiveProvider::Copilot,
                    ActiveProvider::Gemini,
                    ActiveProvider::Cursor,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::Copilot => {
                vec![
                    ActiveProvider::Copilot,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Antigravity,
                    ActiveProvider::Gemini,
                    ActiveProvider::Cursor,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::Antigravity => {
                vec![
                    ActiveProvider::Antigravity,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                    ActiveProvider::Gemini,
                    ActiveProvider::Cursor,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::Gemini => {
                vec![
                    ActiveProvider::Gemini,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Antigravity,
                    ActiveProvider::Copilot,
                    ActiveProvider::Cursor,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::Cursor => {
                vec![
                    ActiveProvider::Cursor,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                    ActiveProvider::Antigravity,
                    ActiveProvider::Gemini,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::OpenRouter => {
                vec![
                    ActiveProvider::OpenRouter,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                    ActiveProvider::Antigravity,
                    ActiveProvider::Gemini,
                    ActiveProvider::Cursor,
                ]
            }
        }
    }

    pub(super) fn summarize_error(err: &anyhow::Error) -> String {
        err.to_string()
            .lines()
            .next()
            .unwrap_or("unknown error")
            .trim()
            .to_string()
    }

    fn contains_standalone_status_code(haystack: &str, code: &str) -> bool {
        let haystack_bytes = haystack.as_bytes();
        let code_len = code.len();

        haystack.match_indices(code).any(|(start, _)| {
            let before_ok = start == 0 || !haystack_bytes[start - 1].is_ascii_digit();
            let end = start + code_len;
            let after_ok = end == haystack_bytes.len() || !haystack_bytes[end].is_ascii_digit();
            before_ok && after_ok
        })
    }

    pub(super) fn classify_failover_error(err: &anyhow::Error) -> FailoverDecision {
        let lower = err.to_string().to_ascii_lowercase();

        let request_size_or_context = [
            "context length",
            "context_length",
            "context window",
            "maximum context",
            "prompt is too long",
            "input is too long",
            "too many tokens",
            "max tokens",
            "token limit",
            "token_limit",
            "413 payload too large",
            "413 request entity too large",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || Self::contains_standalone_status_code(&lower, "413");
        if request_size_or_context {
            return FailoverDecision::RetryNextProvider;
        }

        let rate_or_quota = [
            "rate limit",
            "rate-limited",
            "too many requests",
            "quota",
            "credit balance",
            "credits have run out",
            "insufficient credit",
            "billing",
            "payment required",
            "usage tier",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || Self::contains_standalone_status_code(&lower, "429")
            || Self::contains_standalone_status_code(&lower, "402");
        if rate_or_quota {
            return FailoverDecision::RetryAndMarkUnavailable;
        }

        let auth_or_access = [
            "access denied",
            "not accessible by integration",
            "provider unavailable",
            "provider not available",
            "provider is unavailable",
            "provider currently unavailable",
            "provider not configured",
            "credentials are not configured",
            "no credentials",
            "token exchange failed",
            "authentication failed",
            "unauthorized",
            "forbidden",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || Self::contains_standalone_status_code(&lower, "401")
            || Self::contains_standalone_status_code(&lower, "403");
        if auth_or_access {
            return FailoverDecision::RetryAndMarkUnavailable;
        }

        FailoverDecision::None
    }

    pub(super) fn additional_no_provider_guidance(&self) -> Vec<String> {
        [ActiveProvider::Claude, ActiveProvider::OpenAI]
            .into_iter()
            .filter_map(crate::provider::account_failover::account_switch_guidance)
            .collect()
    }

    pub(super) fn no_provider_available_error(&self, notes: &[String]) -> anyhow::Error {
        let mut msg = "No tokens/providers left: no usable provider right now. Anthropic/OpenAI usage may be exhausted and GitHub Copilot is not authenticated or currently unavailable.".to_string();
        if !notes.is_empty() {
            msg.push_str(" Details: ");
            msg.push_str(&notes.join(" | "));
        }
        let extra_guidance = self.additional_no_provider_guidance();
        if !extra_guidance.is_empty() {
            msg.push(' ');
            msg.push_str(&extra_guidance.join(" "));
        }
        msg.push_str(" Use `/usage` to check limits and `/login <provider>` to re-authenticate.");
        anyhow::anyhow!(msg)
    }
}
