//! Out-of-credit (HTTP 402) failover between OpenAI-compatible profiles.
//!
//! Every OpenAI-compatible profile (`[providers.<name>]` and catalog profiles
//! such as `opencode-*`) runs behind the single `ActiveProvider::OpenRouter`
//! slot, and its runtime reports HTTP errors *inside* the returned stream. So
//! the provider-level failover loop never saw a 402 and never considered other
//! profiles that serve the same model. This module peeks at the start of the
//! stream, and when the active profile is out of credit it resends the turn to
//! another configured profile serving the same model id.

use super::*;
use crate::message::{ConnectionPhase, StreamEvent};
use futures::StreamExt;

const COMPATIBLE_API_METHOD_PREFIX: &str = "openai-compatible:";

/// Key under which a profile's out-of-credit state is recorded (5 minute TTL,
/// see `PROVIDER_RUNTIME_UNAVAILABLE_TTL`).
pub(super) fn compatible_profile_unavailability_key(profile_id: &str) -> String {
    format!("{COMPATIBLE_API_METHOD_PREFIX}{profile_id}")
}

fn compatible_profile_unavailable_detail(profile_id: &str) -> Option<String> {
    provider_unavailability_detail_for_account(&compatible_profile_unavailability_key(profile_id))
}

enum Peeked {
    Stream(EventStream),
    OutOfCredit(anyhow::Error),
}

/// Read the stream up to the HTTP response (connection bookkeeping events
/// only). A billing error at that point means the request never reached the
/// model, so it is safe to resend elsewhere.
async fn peek_for_out_of_credit(mut stream: EventStream) -> Peeked {
    let mut buffered: Vec<Result<StreamEvent>> = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => {
                let still_connecting = matches!(
                    event,
                    StreamEvent::ConnectionType { .. }
                        | StreamEvent::StatusDetail { .. }
                        | StreamEvent::ConnectionPhase {
                            phase: ConnectionPhase::Authenticating
                                | ConnectionPhase::Connecting
                                | ConnectionPhase::SendingRequest
                                | ConnectionPhase::Retrying { .. }
                        }
                );
                buffered.push(Ok(event));
                if !still_connecting {
                    break;
                }
            }
            Err(err) => {
                if jcode_provider_core::is_billing_exhausted_error_message(&format!("{err:#}")) {
                    return Peeked::OutOfCredit(err);
                }
                buffered.push(Err(err));
                break;
            }
        }
    }
    Peeked::Stream(Box::pin(futures::stream::iter(buffered).chain(stream)))
}

/// Short plain-words reason, e.g. `out of credit (HTTP 402: Insufficient credits)`.
fn out_of_credit_reason(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
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

    /// Complete on `candidate`; when it is the active OpenAI-compatible
    /// profile and that profile is out of credit, resend on a sibling profile
    /// serving the same model (or, with `cross_provider_failover = "manual"`,
    /// return the failover prompt instead of resending).
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
                match peek_for_out_of_credit(stream).await {
                    Peeked::Stream(stream) => return Ok(stream),
                    Peeked::OutOfCredit(err) => {
                        let reason = out_of_credit_reason(&err);
                        record_provider_unavailable_for_account(
                            &compatible_profile_unavailability_key(&from_profile),
                            &reason,
                        );
                        crate::logging::warn(&format!(
                            "OpenAI-compatible profile {} is {}; failing over to another profile serving {}",
                            from_profile, reason, model
                        ));
                        reason
                    }
                }
            }
        };

        if crate::config::config().provider.cross_provider_failover
            == crate::config::CrossProviderFailoverMode::Manual
        {
            let (chars, tokens) = Self::estimate_request_input(messages, tools, mode);
            let to_profile = &siblings[0];
            return Err(anyhow::anyhow!(
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
            ));
        }

        let mut last_error: Option<anyhow::Error> = None;
        for to_profile in &siblings {
            if let Err(err) = self.set_model(&format!("{to_profile}:{model}")) {
                crate::logging::warn(&format!(
                    "Out-of-credit failover: could not switch to {}: {}",
                    to_profile, err
                ));
                continue;
            }
            let attempt = self
                .complete_candidate(candidate, messages, tools, mode, None)
                .await;
            let stream = match attempt {
                Ok(stream) => stream,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };
            match peek_for_out_of_credit(stream).await {
                Peeked::Stream(stream) => {
                    let notice = format!(
                        "{from_profile} is {reason}. Switched this session to {to_profile} (same model {model}) and resent the turn. Use /model to pick another route."
                    );
                    crate::logging::info(&format!("Out-of-credit failover: {notice}"));
                    self.startup_notices
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(format!("⚡ {notice}"));
                    let status = futures::stream::once(async move {
                        Ok(StreamEvent::StatusDetail { detail: notice })
                    });
                    return Ok(Box::pin(status.chain(stream)));
                }
                Peeked::OutOfCredit(err) => {
                    record_provider_unavailable_for_account(
                        &compatible_profile_unavailability_key(to_profile),
                        &out_of_credit_reason(&err),
                    );
                    last_error = Some(err);
                }
            }
        }

        // Every sibling failed too: go back to the original profile and
        // surface the last error the way the runtime would have.
        let _ = self.set_model(&format!("{from_profile}:{model}"));
        let err = last_error.unwrap_or_else(|| anyhow::anyhow!("{from_profile} is {reason}"));
        Ok(Box::pin(futures::stream::once(async move { Err(err) })))
    }
}
