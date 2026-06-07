use super::*;
use jcode_provider_core::{FailoverDecision, ProviderFailoverPrompt};

impl MultiProvider {
    pub(super) fn provider_is_configured(&self, provider: ActiveProvider) -> bool {
        self.reconcile_auth_if_provider_missing(provider)
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
        jcode_provider_core::fallback_sequence(active)
    }

    pub(super) fn summarize_error(err: &anyhow::Error) -> String {
        err.to_string()
            .lines()
            .next()
            .unwrap_or("unknown error")
            .trim()
            .to_string()
    }

    pub(super) fn classify_failover_error(err: &anyhow::Error) -> FailoverDecision {
        jcode_provider_core::classify_failover_error_message(&err.to_string())
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
