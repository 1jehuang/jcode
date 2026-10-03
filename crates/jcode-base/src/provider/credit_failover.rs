//! Out-of-credit (HTTP 402) failover between OpenAI-compatible profiles.
//!
//! Every OpenAI-compatible profile (`[providers.<name>]` and catalog profiles
//! such as `opencode-*`) runs behind the single `ActiveProvider::OpenRouter`
//! slot, and its runtime reports HTTP errors *inside* the returned stream. So
//! the provider-level failover loop never saw a 402 and never considered other
//! profiles that serve the same model. This module peeks at the start of the
//! stream, and when the active profile is out of credit it offers another
//! configured profile serving the same model id.
//!
//! The provider never resends on its own. It returns a
//! `ProviderFailoverPrompt` in both `cross_provider_failover` modes: the TUI
//! runs the cancelable countdown (countdown mode) or shows the manual hint
//! (manual mode), and only the TUI switches profile and resends. That keeps
//! the conversation and tools away from another endpoint until the user had
//! the chance to press Esc.

use super::stream_peek::{Peeked, peek_before_output};
use super::*;

const COMPATIBLE_API_METHOD_PREFIX: &str = "openai-compatible:";

/// Key under which a profile's out-of-credit state is recorded (5 minute TTL,
/// see `PROVIDER_RUNTIME_UNAVAILABLE_TTL`).
pub(super) fn compatible_profile_unavailability_key(profile_id: &str) -> String {
    format!("{COMPATIBLE_API_METHOD_PREFIX}{profile_id}")
}

fn compatible_profile_unavailable_detail(profile_id: &str) -> Option<String> {
    provider_unavailability_detail_for_account(&compatible_profile_unavailability_key(profile_id))
}

/// Short plain-words reason, e.g. `out of credit (HTTP 402: Insufficient credits)`.
fn out_of_credit_reason(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    if !text.contains("response:") && !text.trim().is_empty() && text.lines().count() == 1 {
        // SSE error event: the message is the provider's own wording.
        return format!("out of credit ({})", text.trim());
    }
    let detail = serde_json::from_str::<serde_json::Value>(
        text.lines()
            .find_map(|line| line.trim().strip_prefix("response:"))
            .unwrap_or("")
            .trim(),
    )
    .ok()
    .and_then(|body| {
        body.pointer("/error/message")
            .or_else(|| body.get("error"))
            .or_else(|| body.get("message"))
            .and_then(|value| value.as_str().map(str::to_string))
    });
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            format!("out of credit (HTTP 402: {})", detail.trim())
        }
        _ => "out of credit (HTTP 402 Payment Required)".to_string(),
    }
}

impl MultiProvider {
    pub(super) async fn complete_candidate(
        &self,
        candidate: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        match mode {
            CompletionMode::Unified { system } => {
                self.complete_on_provider(candidate, messages, tools, system, resume_session_id)
                    .await
            }
            CompletionMode::Split {
                system_static,
                system_dynamic,
            } => {
                self.complete_split_on_provider(
                    candidate,
                    messages,
                    tools,
                    system_static,
                    system_dynamic,
                    resume_session_id,
                )
                .await
            }
        }
    }

    /// Profile id of the active direct OpenAI-compatible runtime, if any.
    fn active_compatible_profile_id_for_failover(&self) -> Option<String> {
        if self.active_provider() != ActiveProvider::OpenRouter {
            return None;
        }
        let (_, api_method, _) = self
            .active_openrouter_execution_provider()?
            .direct_openai_compatible_route_parts()?;
        api_method
            .strip_prefix(COMPATIBLE_API_METHOD_PREFIX)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    /// Other configured OpenAI-compatible profiles that serve exactly `model`
    /// and are not marked out of credit, sorted by profile id.
    fn compatible_profile_siblings(&self, from_profile: &str, model: &str) -> Vec<String> {
        let mut siblings: Vec<String> = self
            .fresh_routes_memo_entry()
            .routes
            .iter()
            .filter(|route| route.available && route.model == model)
            .filter_map(|route| {
                route
                    .api_method
                    .strip_prefix(COMPATIBLE_API_METHOD_PREFIX)
                    .map(str::trim)
                    .map(str::to_string)
            })
            .filter(|id| !id.is_empty() && id != from_profile)
            .filter(|id| compatible_profile_unavailable_detail(id).is_none())
            .collect();
        siblings.sort();
        siblings.dedup();
        siblings
    }

    /// Short reason for a sibling that failed with something other than
    /// billing exhaustion, e.g. `unavailable (429 Too Many Requests)`.
    fn failed_profile_reason(err: &anyhow::Error) -> String {
        let text = format!("{err:#}");
        let status = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("status:"))
            .map(str::trim)
            .filter(|status| !status.is_empty());
        match status {
            Some(status) => format!("unavailable (HTTP {status})"),
            None => format!(
                "unavailable ({})",
                text.lines().next().unwrap_or("request failed").trim()
            ),
        }
    }

    /// True when another profile serving `model` is marked out of credit, which
    /// means this turn is part of an out-of-credit failover chain.
    fn credit_failover_chain_active(&self, from_profile: &str, model: &str) -> bool {
        self.fresh_routes_memo_entry()
            .routes
            .iter()
            .filter(|route| route.model == model)
            .filter_map(|route| route.api_method.strip_prefix(COMPATIBLE_API_METHOD_PREFIX))
            .map(str::trim)
            .filter(|id| !id.is_empty() && *id != from_profile)
            .any(|id| {
                compatible_profile_unavailable_detail(id)
                    .is_some_and(|detail| detail.contains("out of credit"))
            })
    }

    /// Complete on `candidate`. When it is the active OpenAI-compatible
    /// profile and that profile is out of credit (or, during a failover chain,
    /// fails in any other way before output), return a failover prompt that
    /// offers the next sibling profile serving the same model.
    ///
    /// Nothing is sent to the sibling here. The TUI countdown (or the user,
    /// in manual mode) switches to `to_provider` and resends; the next call
    /// then tries that sibling and, if it fails too, offers the one after it.
    pub(super) async fn complete_candidate_with_credit_failover(
        &self,
        candidate: ActiveProvider,
        active: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let from_profile = (candidate == ActiveProvider::OpenRouter && candidate == active)
            .then(|| self.active_compatible_profile_id_for_failover())
            .flatten();
        let Some(from_profile) = from_profile else {
            return self
                .complete_candidate(candidate, messages, tools, mode, resume_session_id)
                .await;
        };
        let model = self.model();
        let siblings = self.compatible_profile_siblings(&from_profile, &model);
        if siblings.is_empty() {
            // Nobody else serves this model: keep today's behaviour.
            return self
                .complete_candidate(candidate, messages, tools, mode, resume_session_id)
                .await;
        }

        let reason = match compatible_profile_unavailable_detail(&from_profile) {
            Some(detail) => detail,
            None => {
                let stream = self
                    .complete_candidate(candidate, messages, tools, mode, resume_session_id)
                    .await?;
                match peek_before_output(stream).await {
                    Peeked::Stream(stream) => return Ok(stream),
                    Peeked::Failed {
                        error,
                        out_of_credit,
                        replay,
                        ..
                    } => {
                        let reason = if out_of_credit {
                            out_of_credit_reason(&error)
                        } else if self.credit_failover_chain_active(&from_profile, &model) {
                            Self::failed_profile_reason(&error)
                        } else {
                            // Not billing and no failover in progress: this is
                            // an ordinary error for the normal error paths.
                            return Ok(replay);
                        };
                        record_provider_unavailable_for_account(
                            &compatible_profile_unavailability_key(&from_profile),
                            &reason,
                        );
                        crate::logging::warn(&format!(
                            "OpenAI-compatible profile {} is {}; offering {} (same model {})",
                            from_profile, reason, siblings[0], model
                        ));
                        reason
                    }
                }
            }
        };

        let (chars, tokens) = Self::estimate_request_input(messages, tools, mode);
        let to_profile = &siblings[0];
        Err(anyhow::anyhow!(
            ProviderFailoverPrompt {
                from_provider: format!("{from_profile}:{model}"),
                from_label: from_profile.clone(),
                to_provider: format!("{to_profile}:{model}"),
                to_label: format!("{to_profile} ({model})"),
                reason,
                estimated_input_chars: chars,
                estimated_input_tokens: tokens,
            }
            .to_error_message()
        ))
    }
}
