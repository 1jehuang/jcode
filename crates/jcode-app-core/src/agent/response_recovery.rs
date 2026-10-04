use super::*;

/// A tool call that failed schema validation, kept so the correction and
/// terminal error can repeat exactly what the model did wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MalformedToolCallInfo {
    /// Tool name exactly as the model sent it.
    pub(crate) name: String,
    /// The validation error that was returned to the model as the tool result.
    pub(crate) error: String,
}

impl Agent {
    fn parse_text_wrapped_tool_call(
        text: &str,
    ) -> Option<(String, String, serde_json::Value, String)> {
        let marker = "to=functions.";
        let marker_idx = text.find(marker)?;
        let after_marker = &text[marker_idx + marker.len()..];

        let mut tool_name_end = 0usize;
        for (idx, ch) in after_marker.char_indices() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                tool_name_end = idx + ch.len_utf8();
            } else {
                break;
            }
        }
        if tool_name_end == 0 {
            return None;
        }

        let tool_name = after_marker[..tool_name_end].to_string();
        let remaining = &after_marker[tool_name_end..];
        let mut fallback: Option<(String, String, serde_json::Value, String)> = None;

        for (brace_idx, ch) in remaining.char_indices() {
            if ch != '{' {
                continue;
            }
            let slice = &remaining[brace_idx..];
            let mut stream =
                serde_json::Deserializer::from_str(slice).into_iter::<serde_json::Value>();
            let parsed = match stream.next() {
                Some(Ok(value)) => value,
                Some(Err(_)) | None => continue,
            };
            let consumed = stream.byte_offset();
            if !parsed.is_object() {
                continue;
            }

            let prefix = text[..marker_idx].trim_end().to_string();
            let suffix = remaining[brace_idx + consumed..].trim().to_string();
            if suffix.is_empty() {
                return Some((prefix, tool_name.clone(), parsed, suffix));
            }
            if fallback.is_none() {
                fallback = Some((prefix, tool_name.clone(), parsed, suffix));
            }
        }

        fallback
    }

    pub(super) fn recover_text_wrapped_tool_call(
        &self,
        text_content: &mut String,
        tool_calls: &mut Vec<ToolCall>,
    ) -> bool {
        if !tool_calls.is_empty() || text_content.trim().is_empty() {
            return false;
        }

        let Some((prefix, tool_name, arguments, suffix)) =
            Self::parse_text_wrapped_tool_call(text_content)
        else {
            return false;
        };

        let mut sanitized = String::new();
        if !prefix.is_empty() {
            sanitized.push_str(&prefix);
        }
        if !suffix.is_empty() {
            if !sanitized.is_empty() {
                sanitized.push('\n');
            }
            sanitized.push_str(&suffix);
        }
        *text_content = sanitized;

        let call_id = format!("fallback_text_call_{}", id::new_id("call"));
        let recovered_total = RECOVERED_TEXT_WRAPPED_TOOL_CALLS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        logging::warn(&format!(
            "[agent] Recovered text-wrapped tool call for '{}' ({}, total={})",
            tool_name, call_id, recovered_total
        ));
        let intent = ToolCall::intent_from_input(&arguments);
        tool_calls.push(ToolCall {
            id: call_id,
            name: tool_name,
            input: arguments,
            intent,
            thought_signature: None,
        });

        true
    }

    pub(crate) fn should_continue_after_stop_reason(stop_reason: &str) -> bool {
        let reason = stop_reason.trim().to_ascii_lowercase();
        if reason.is_empty() {
            return false;
        }

        if matches!(reason.as_str(), "stop" | "end_turn" | "tool_use") {
            return false;
        }

        // Anthropic pauses long server-tool turns (web search) with
        // `pause_turn`; resending the conversation resumes them.
        Self::is_pause_turn_stop_reason(&reason)
            || reason.contains("incomplete")
            || reason.contains("max_output_tokens")
            || reason.contains("max_tokens")
            || reason.contains("length")
            || reason.contains("trunc")
            || reason.contains("commentary")
    }

    /// Anthropic `pause_turn`: the provider paused a long server-tool turn.
    pub(crate) fn is_pause_turn_stop_reason(stop_reason: &str) -> bool {
        stop_reason.trim().eq_ignore_ascii_case("pause_turn")
    }

    /// True when the provider's stop reason indicates a model-side
    /// guardrail/safety stop (e.g. Anthropic `refusal`), as opposed to a
    /// normal end-of-turn or truncation.
    pub(crate) fn is_guardrail_stop_reason(stop_reason: Option<&str>) -> bool {
        let Some(reason) = stop_reason else {
            return false;
        };
        let reason = reason.trim().to_ascii_lowercase();
        matches!(reason.as_str(), "refusal" | "content_filter" | "safety")
            || reason.contains("guardrail")
            || reason.contains("policy_violation")
    }

    pub(crate) const FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS: [&'static str; 3] = [
        "Reassess the user's request using its full context and likely intent rather than isolated keywords. Determine whether answering would enable a concrete harmful action. If no specific safety concern applies, answer normally. Preserve the refusal only when it is genuinely warranted, and briefly identify the specific concern.",
        "Review the request again and distinguish legitimate coding, debugging, defensive, educational, or analytical work from instructions that facilitate harm. If only part of the request is unsafe, help with the safe portions and narrowly limit the rest instead of issuing a blanket refusal.",
        "Perform one final, independent policy check. Prefer a safe and useful constrained response when possible. Refuse only the specific content that creates a concrete safety risk; otherwise continue with the user's actual task. Do not weaken a refusal that remains genuinely necessary.",
    ];

    /// Try a small sequence of differently framed policy checks after Fable
    /// guardrails a response. Every prompt preserves warranted refusals, and the
    /// fixed suite size prevents an unbounded refusal/retry loop.
    pub(crate) fn maybe_reconsider_fable_guardrail(
        &mut self,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        let model = self.provider.model();
        if !Self::should_reconsider_fable_guardrail(
            &model,
            stop_reason,
            *attempts,
            Self::FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS.len() as u32,
        ) {
            return Ok(false);
        }

        let prompt = Self::FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS[*attempts as usize];
        *attempts += 1;
        logging::warn(&format!(
            "Fable 5 guardrail stopped the response (stop_reason={:?}); trying reconsideration prompt {}/{}",
            stop_reason,
            attempts,
            Self::FABLE_GUARDRAIL_RECONSIDERATION_PROMPTS.len(),
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    pub(crate) fn should_reconsider_fable_guardrail(
        model: &str,
        stop_reason: Option<&str>,
        attempts: u32,
        max_attempts: u32,
    ) -> bool {
        Self::is_guardrail_stop_reason(stop_reason)
            && model.to_ascii_lowercase().contains("fable-5")
            && attempts < max_attempts
    }

    /// Builds the user-facing notice for a turn that ended with no visible
    /// assistant output (no text, no tool calls). Returns `None` when the turn
    /// looks normal and no notice should be surfaced.
    pub(crate) fn provider_guardrail_notice(
        stop_reason: Option<&str>,
        visible_text_empty: bool,
        had_reasoning: bool,
    ) -> Option<String> {
        let guardrail = Self::is_guardrail_stop_reason(stop_reason);
        if !guardrail && !visible_text_empty {
            return None;
        }
        let reason_label = stop_reason
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .unwrap_or("unknown");
        if guardrail {
            return Some(format!(
                "Provider guardrail stopped the response (stop_reason: {}). The model declined to answer this request. Rephrasing, narrowing the request, or providing more context may help.",
                reason_label
            ));
        }
        // Empty visible output with a non-guardrail stop reason: still surface,
        // since the user otherwise sees nothing at all. Do not assert a content
        // filter here: in practice this is usually a transient upstream failure
        // (a dropped or empty stream), not a provider guardrail (issue #672).
        let reasoning_hint = if had_reasoning {
            " after producing only internal reasoning"
        } else {
            ""
        };
        Some(format!(
            "The model ended its turn without any visible output{} (stop_reason: {}). The provider returned an empty response; this is usually a transient upstream failure rather than a content filter. Retrying the request may help.",
            reasoning_hint, reason_label
        ))
    }

    /// Log-event label for an empty final turn: real guardrail stops keep the
    /// `PROVIDER_GUARDRAIL` name, transient empty responses get their own so
    /// the two are separable in logs (issue #672).
    pub(crate) fn empty_turn_log_event(stop_reason: Option<&str>) -> &'static str {
        if Self::is_guardrail_stop_reason(stop_reason) {
            "PROVIDER_GUARDRAIL"
        } else {
            "PROVIDER_EMPTY_RESPONSE"
        }
    }

    /// Stable opening of the continuation injected by
    /// [`Self::maybe_continue_empty_post_tool_response`].
    ///
    /// `messages_end_with_tool_result` treats any User-role text opening with
    /// `<system-reminder>` as evidence that tool results are in play. Without
    /// excluding this one specifically, the injected continuation keeps that
    /// signal true on the following turn by itself, so each whitespace-only
    /// response appends another one and spends another API call.
    pub(crate) const EMPTY_POST_TOOL_CONTINUATION_PREFIX: &str =
        "<system-reminder>The previous provider response was empty after tool results.";

    /// Retry a whitespace-only final response that arrived right after tool
    /// results, by asking the model to produce the final answer. Shared by the
    /// non-streaming and streaming (mpsc) turn loops so their recovery
    /// behavior cannot drift (issue #672). Returns true when a continuation
    /// message was injected and the caller should re-issue the request.
    pub(crate) fn maybe_continue_empty_post_tool_response(
        &mut self,
        visible_text_empty: bool,
        prompt_has_recent_tool_result: bool,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        if !visible_text_empty || !prompt_has_recent_tool_result {
            return Ok(false);
        }
        // A model-side refusal is deliberate; retrying it just burns tokens.
        if Self::is_guardrail_stop_reason(stop_reason) {
            return Ok(false);
        }
        // A paused server-tool turn is resumed by the incomplete-response path,
        // which must not see an injected user message first.
        if stop_reason.is_some_and(Self::is_pause_turn_stop_reason) {
            return Ok(false);
        }
        if *attempts >= Self::MAX_EMPTY_POST_TOOL_CONTINUATION_ATTEMPTS {
            return Ok(false);
        }
        *attempts += 1;
        logging::warn(&format!(
            "Provider returned whitespace-only final response after tool results (stop_reason={:?}); requesting final answer continuation (attempt {}/{})",
            stop_reason,
            attempts,
            Self::MAX_EMPTY_POST_TOOL_CONTINUATION_ATTEMPTS
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                // Keep this as a user-role message for provider compatibility,
                // but mark it as internal so transcript renderers never present
                // the synthetic recovery instruction as a prompt from the user.
                // Built from the shared prefix so the predicate that must exclude
                // this channel cannot drift from the text that produces it.
                text: format!(
                    "{} Provide the final answer to the user's last request using the tool results above. Do not call more tools unless absolutely necessary.</system-reminder>",
                    Self::EMPTY_POST_TOOL_CONTINUATION_PREFIX
                ),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    fn continuation_prompt_for_stop_reason(stop_reason: &str) -> String {
        format!(
            "[System reminder: your previous response ended before completion (stop_reason: {}). Continue exactly where you left off, do not repeat completed content, and if the next step is a tool call, emit the tool call now.]",
            stop_reason.trim()
        )
    }

    pub(crate) fn maybe_continue_incomplete_response(
        &mut self,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        let Some(stop_reason) = stop_reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
        else {
            return Ok(false);
        };

        if !Self::should_continue_after_stop_reason(stop_reason) {
            return Ok(false);
        }

        if *attempts >= Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS {
            logging::warn(&format!(
                "Response ended with stop_reason='{}' after {} continuation attempts; returning partial output",
                stop_reason, attempts
            ));
            return Ok(false);
        }

        *attempts += 1;

        if Self::is_pause_turn_stop_reason(stop_reason) {
            // Resend the paused assistant turn unchanged: no synthetic user
            // message, or the API cannot resume the server tool loop.
            logging::info(&format!(
                "Response paused with stop_reason='pause_turn'; resuming (attempt {}/{})",
                attempts,
                Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS
            ));
            return Ok(true);
        }

        logging::warn(&format!(
            "Response ended with stop_reason='{}'; requesting continuation (attempt {}/{})",
            stop_reason,
            attempts,
            Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS
        ));

        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: Self::continuation_prompt_for_stop_reason(stop_reason),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    /// True when the provider said it stopped to call a tool but no tool call
    /// survived parsing.
    ///
    /// `stop_reason: tool_use` with zero tool calls is a contradiction: the
    /// model intended to act and the harness has nothing to run. Breaking out
    /// of the turn there strands the agent mid-task, which on a benchmark run
    /// looks like an ordinary "the agent stopped early" failure and silently
    /// discards all of its uncommitted work. Treat it like any other
    /// incomplete response and ask for a continuation instead.
    pub(crate) fn is_stranded_tool_use_stop(stop_reason: Option<&str>) -> bool {
        stop_reason
            .map(str::trim)
            .map(|reason| reason.eq_ignore_ascii_case("tool_use"))
            .unwrap_or(false)
    }

    pub(crate) fn maybe_continue_stranded_tool_use(
        &mut self,
        stop_reason: Option<&str>,
        attempts: &mut u32,
    ) -> Result<bool> {
        if !Self::is_stranded_tool_use_stop(stop_reason) {
            return Ok(false);
        }
        if *attempts >= Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS {
            logging::warn(&format!(
                "Provider reported stop_reason='tool_use' with no parsed tool call after {} continuation attempts; ending turn",
                attempts
            ));
            return Ok(false);
        }
        *attempts += 1;
        logging::warn(&format!(
            "Provider reported stop_reason='tool_use' but no tool call was parsed; requesting continuation (attempt {}/{})",
            attempts,
            Self::MAX_INCOMPLETE_CONTINUATION_ATTEMPTS
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "[System reminder: your previous response ended with stop_reason \"tool_use\" but no tool call arrived. Nothing was executed. Re-issue the tool call you intended, do not repeat completed work, and continue the task.]"
                    .to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    pub(super) fn filter_truncated_tool_calls(
        &mut self,
        stop_reason: Option<&str>,
        tool_calls: &mut Vec<ToolCall>,
        assistant_message_id: Option<&String>,
    ) {
        let stop_reason = stop_reason.unwrap_or("");
        if !Self::should_continue_after_stop_reason(stop_reason) {
            return;
        }

        let before = tool_calls.len();
        tool_calls.retain(|tc| !tc.input.is_null());
        let discarded = before - tool_calls.len();
        if discarded > 0 && tool_calls.is_empty() {
            logging::warn(&format!(
                "Discarded {} tool call(s) with null input (truncated by {}); requesting continuation",
                discarded,
                if stop_reason.is_empty() {
                    "unknown"
                } else {
                    stop_reason
                }
            ));
            if let Some(msg_id) = assistant_message_id {
                self.session.remove_tool_use_blocks(msg_id);
                self.persist_session_best_effort("truncated tool-call repair");
            }
        }
    }

    /// Cap on consecutive tool rounds in which *every* call failed schema
    /// validation (e.g. a model repeatedly emitting `null` arguments, as
    /// reported for GLM-5.3 calling `write`/`bash`, apologizing, then doing it
    /// again). Rounds below the cap get a schema-guided correction; the round
    /// at the cap ends the turn with an actionable error instead of spending
    /// forever on the same malformed output.
    pub(crate) const MAX_MALFORMED_TOOL_CALL_ROUNDS: u32 = 3;

    /// Stable prefix for the terminal malformed-tool-call failure. The TUI
    /// keys its auto-poke disable and resume affordances off this exact
    /// prefix, so it must not change without coordinating the client side.
    /// Public so the TUI can reference `crate::agent::Agent::MALFORMED_TOOL_CALL_ERROR_PREFIX`
    /// instead of hardcoding the string.
    pub const MALFORMED_TOOL_CALL_ERROR_PREFIX: &str = "Invalid tool calls:";

    /// Bounded recovery for tool rounds in which every call failed schema
    /// validation. Shared by the blocking and streaming mpsc turn loops so
    /// the behavior cannot drift.
    ///
    /// Semantics:
    /// - A round with at least one valid (executable) tool call resets the
    ///   consecutive-round counter: the model is not stuck in a
    ///   validation-only loop, and the per-call error results already carry
    ///   the feedback for any malformed sibling call.
    /// - A validation-only round (no valid calls) increments the counter and,
    ///   below the cap, injects a user-role system reminder repeating each
    ///   validation error plus the failing tool's expected input schema. The
    ///   invalid tool_use blocks and error tool results stay in history and
    ///   null arguments are never executed or fabricated.
    /// - On the second consecutive validation-only round the provider session
    ///   state is reset once per streak, so the next request resends full
    ///   context instead of continuing a possibly corrupted server-side
    ///   session.
    /// - At [`Agent::MAX_MALFORMED_TOOL_CALL_ROUNDS`] consecutive rounds the
    ///   turn fails with a terminal error starting with
    ///   [`Agent::MALFORMED_TOOL_CALL_ERROR_PREFIX`] and actionable guidance.
    pub(crate) fn handle_malformed_tool_round(
        &mut self,
        invalid: &[MalformedToolCallInfo],
        executed_valid_call: bool,
        consecutive_rounds: &mut u32,
        tools: &[ToolDefinition],
    ) -> Result<()> {
        if invalid.is_empty() {
            if executed_valid_call {
                *consecutive_rounds = 0;
            }
            return Ok(());
        }
        if executed_valid_call {
            *consecutive_rounds = 0;
            return Ok(());
        }

        *consecutive_rounds = consecutive_rounds.saturating_add(1);
        let last_error = invalid
            .last()
            .map(|call| call.error.as_str())
            .unwrap_or("malformed tool call");

        if *consecutive_rounds >= Self::MAX_MALFORMED_TOOL_CALL_ROUNDS {
            logging::warn(&format!(
                "Malformed tool-call round {}/{} ({} invalid call(s), last: {}); ending turn",
                consecutive_rounds,
                Self::MAX_MALFORMED_TOOL_CALL_ROUNDS,
                invalid.len(),
                last_error
            ));
            return Err(anyhow::anyhow!(
                "{}",
                Self::malformed_tool_call_terminal_error(
                    self.provider.model(),
                    *consecutive_rounds,
                    last_error
                )
            ));
        }

        // The first correction did not help; reset provider session state
        // once per streak so the next request resends full context.
        if *consecutive_rounds == 2 && self.provider_session_id.is_some() {
            logging::warn(
                "Malformed tool calls repeated despite correction; resetting provider session state",
            );
            self.reset_provider_session();
        }

        logging::warn(&format!(
            "Tool round had {} malformed call(s) (validation-only round {}/{}); injecting schema-guided correction",
            invalid.len(),
            consecutive_rounds,
            Self::MAX_MALFORMED_TOOL_CALL_ROUNDS
        ));
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                // User-role system reminder so transcript renderers hide the
                // synthetic recovery instruction, matching the empty-response
                // recovery message style.
                text: Self::malformed_tool_call_correction(invalid, tools),
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(())
    }

    /// System reminder correcting malformed tool calls: repeats each
    /// validation error and the failing tool's expected input schema so the
    /// model can re-issue the call without guessing.
    pub(crate) fn malformed_tool_call_correction(
        invalid: &[MalformedToolCallInfo],
        tools: &[ToolDefinition],
    ) -> String {
        let mut lines = String::from(
            "<system-reminder>Your previous tool call(s) were rejected as malformed and nothing was executed:\n",
        );
        let mut seen: Vec<(String, String)> = Vec::new();
        for call in invalid {
            let key = (call.name.clone(), call.error.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            lines.push_str(&format!("- {}\n", call.error));
            match tools.iter().find(|tool| tool.name == call.name) {
                Some(definition) => lines.push_str(&format!(
                    "  Re-issue '{}' with arguments that are a single JSON object matching this input schema: {}\n",
                    call.name,
                    malformed_tool_schema_excerpt(&definition.input_schema)
                )),
                None => lines.push_str(&format!(
                    "  '{}' is not an available tool; check the tool name against the available tools.\n",
                    call.name
                )),
            }
        }
        lines.push_str("Do not send null arguments and do not repeat the same malformed call. Emit each tool call with its complete JSON object of arguments.</system-reminder>");
        lines
    }

    /// Actionable terminal error once the malformed-round cap is reached.
    /// The message starts with [`Agent::MALFORMED_TOOL_CALL_ERROR_PREFIX`].
    pub(crate) fn malformed_tool_call_terminal_error(
        model: impl std::fmt::Display,
        consecutive_rounds: u32,
        last_error: &str,
    ) -> String {
        format!(
            "{prefix} model '{model}' repeatedly emitted malformed tool calls (null or non-object arguments) for {rounds} consecutive rounds (last error: {last_error}). Nothing was executed in those rounds and the turn was stopped to avoid endless retries. Re-send your message to try again, or switch models with /model.",
            prefix = Self::MALFORMED_TOOL_CALL_ERROR_PREFIX,
            model = model,
            rounds = consecutive_rounds,
            last_error = last_error,
        )
    }
}

/// Compact, correction-focused schema excerpt for the malformed-call reminder.
///
/// Leads with the `required` field names and a compact `property:type` list.
/// Serializing the raw schema and truncating it would cut off `required`,
/// which usually sits at the end and is exactly the part a model emitting
/// null arguments needs to see to fix the call.
fn malformed_tool_schema_excerpt(schema: &serde_json::Value) -> String {
    const MAX_PROPERTY_CHARS: usize = 400;

    let object = schema.as_object();
    let required = object
        .and_then(|map| map.get("required"))
        .and_then(|value| value.as_array())
        .map(|names| {
            names
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let properties = object
        .and_then(|map| map.get("properties"))
        .and_then(|value| value.as_object());

    let property_types = |budget: usize| -> String {
        let mut listed = Vec::new();
        let mut used = 0usize;
        let mut overflow = 0usize;
        for (name, definition) in properties.into_iter().flatten() {
            let kind = definition
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let entry = format!("{name}:{kind}");
            if used + entry.len() > budget && !listed.is_empty() {
                overflow += 1;
                continue;
            }
            used += entry.len();
            listed.push(entry);
        }
        let mut out = listed.join(", ");
        if overflow > 0 {
            out.push_str(&format!(", ...({overflow} more)"));
        }
        out
    };

    let mut parts = Vec::new();
    if !required.is_empty() {
        parts.push(format!("required: {}", required.join(", ")));
    }
    let property_list = property_types(MAX_PROPERTY_CHARS);
    if !property_list.is_empty() {
        parts.push(format!("properties: {property_list}"));
    }
    if parts.is_empty() {
        // Nothing recognizable (or a bare/non-object schema): fall back to
        // the serialized schema with a hard cap.
        let full = schema.to_string();
        const MAX_FULL_CHARS: usize = 600;
        if full.chars().count() > MAX_FULL_CHARS {
            let truncated: String = full.chars().take(MAX_FULL_CHARS).collect();
            format!("{truncated}...(truncated)")
        } else {
            full
        }
    } else {
        parts.join("; ")
    }
}

#[cfg(test)]
mod malformed_tool_call_recovery_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::Mutex;

    type ExecLog = Arc<Mutex<Vec<Value>>>;
    type SessionLog = Arc<Mutex<Vec<Option<String>>>>;

    /// Scripted provider: each `complete` call pops one scripted round from
    /// the queue, recording the `resume_session_id` it was handed. Running out
    /// of rounds panics, which makes unbounded-loop regressions fail loudly.
    #[derive(Clone)]
    struct MalformedScriptProvider {
        rounds: Arc<Mutex<Vec<Vec<StreamEvent>>>>,
        session_ids: SessionLog,
    }

    #[async_trait]
    impl Provider for MalformedScriptProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            self.session_ids
                .lock()
                .unwrap()
                .push(resume_session_id.map(str::to_string));
            let mut queue = self.rounds.lock().unwrap();
            let events = queue
                .first()
                .expect("scripted rounds exhausted: turn made an unbounded request")
                .clone();
            queue.remove(0);
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
        }
        fn name(&self) -> &str {
            "malformed-script-test"
        }
        fn model(&self) -> String {
            "glm-test".into()
        }
        fn supports_compaction(&self) -> bool {
            false
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(self.clone())
        }
    }

    struct CaptureTool(ExecLog);
    #[async_trait]
    impl crate::tool::Tool for CaptureTool {
        fn name(&self) -> &str {
            "capture"
        }
        fn description(&self) -> &str {
            "Capture test input"
        }
        fn parameters_schema(&self) -> Value {
            json!({"type":"object","properties":{"value":{"type":"boolean"}},"required":["value"]})
        }
        async fn execute(
            &self,
            input: Value,
            _: crate::tool::ToolContext,
        ) -> Result<crate::tool::ToolOutput> {
            self.0.lock().unwrap().push(input);
            Ok(crate::tool::ToolOutput::new("ok"))
        }
    }

    /// A round whose single tool call has null arguments: the streamed
    /// fragments fail JSON parsing, which is exactly how the reported
    /// GLM-5.3 null-argument `write`/`bash` calls reach the agent.
    fn malformed_round(id: &str, session: Option<&str>) -> Vec<StreamEvent> {
        let mut events = vec![
            StreamEvent::ToolUseStart {
                id: id.into(),
                name: "capture".into(),
            },
            StreamEvent::ToolInputDeltaFor {
                id: id.into(),
                delta: "{definitely not json".into(),
            },
            StreamEvent::ToolUseEndFor { id: id.into() },
        ];
        if let Some(session) = session {
            events.push(StreamEvent::SessionId(session.into()));
        }
        events.push(StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".into()),
        });
        events
    }

    fn valid_round(id: &str, value: bool, session: Option<&str>) -> Vec<StreamEvent> {
        let mut events = vec![
            StreamEvent::ToolUseStart {
                id: id.into(),
                name: "capture".into(),
            },
            StreamEvent::ToolInputDeltaFor {
                id: id.into(),
                delta: format!("{{\"value\":{value}}}"),
            },
            StreamEvent::ToolUseEndFor { id: id.into() },
        ];
        if let Some(session) = session {
            events.push(StreamEvent::SessionId(session.into()));
        }
        events.push(StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".into()),
        });
        events
    }

    /// One malformed plus one valid capture call in a single round.
    fn mixed_round(bad_id: &str, good_id: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::ToolUseStart {
                id: bad_id.into(),
                name: "capture".into(),
            },
            StreamEvent::ToolInputDeltaFor {
                id: bad_id.into(),
                delta: "{broken".into(),
            },
            StreamEvent::ToolUseEndFor { id: bad_id.into() },
            StreamEvent::ToolUseStart {
                id: good_id.into(),
                name: "capture".into(),
            },
            StreamEvent::ToolInputDeltaFor {
                id: good_id.into(),
                delta: "{\"value\":true}".into(),
            },
            StreamEvent::ToolUseEndFor { id: good_id.into() },
            StreamEvent::MessageEnd {
                stop_reason: Some("tool_use".into()),
            },
        ]
    }

    fn final_text_round() -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta("done".into()),
            StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            },
        ]
    }

    async fn scripted_agent(rounds: Vec<Vec<StreamEvent>>) -> (Agent, SessionLog, ExecLog) {
        let session_ids: SessionLog = Arc::new(Mutex::new(Vec::new()));
        let exec_log: ExecLog = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Provider> = Arc::new(MalformedScriptProvider {
            rounds: Arc::new(Mutex::new(rounds)),
            session_ids: Arc::clone(&session_ids),
        });
        let registry = Registry::empty();
        registry
            .register(
                "capture".into(),
                Arc::new(CaptureTool(Arc::clone(&exec_log))),
            )
            .await;
        let mut agent = Agent::new(provider, registry);
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "test".into(),
                cache_control: None,
            }],
        );
        (agent, session_ids, exec_log)
    }

    fn correction_reminders(agent: &Agent) -> Vec<String> {
        agent
            .session
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. }
                    if text.starts_with(
                        "<system-reminder>Your previous tool call(s) were rejected as malformed",
                    ) =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn null_tool_use_count(agent: &Agent) -> usize {
        agent
            .session
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|block| matches!(block, ContentBlock::ToolUse { input, .. } if input.is_null()))
            .count()
    }

    fn error_tool_result_count(agent: &Agent) -> usize {
        agent
            .session
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult {
                        is_error: Some(true),
                        ..
                    }
                )
            })
            .count()
    }

    async fn run_scripted_turn(agent: &mut Agent, streaming: bool) -> Result<()> {
        if streaming {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            agent.run_turn_streaming_mpsc(tx).await
        } else {
            agent.run_turn(false).await.map(|_| ())
        }
    }

    #[test]
    fn schema_excerpt_leads_with_required_and_compact_types() {
        let write_like = json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "o".repeat(1000)},
                "content": {"type": "string", "description": "o".repeat(1000)}
            },
            "required": ["file_path", "content"]
        });
        let excerpt = malformed_tool_schema_excerpt(&write_like);
        assert!(
            excerpt.starts_with("required: file_path, content"),
            "required must lead the excerpt, got: {excerpt}"
        );
        assert!(excerpt.contains("file_path:string"));
        assert!(excerpt.contains("content:string"));
        assert!(
            !excerpt.contains("oooo"),
            "long descriptions must be dropped for compactness, got: {excerpt}"
        );

        // A schema with far more properties than the budget still lists the
        // first fields and marks the overflow instead of hiding everything.
        let mut properties = serde_json::Map::new();
        for index in 0..60u32 {
            properties.insert(format!("field_{index:02}"), json!({"type": "string"}));
        }
        let many_properties = json!({"type": "object", "properties": properties});
        let excerpt = malformed_tool_schema_excerpt(&many_properties);
        assert!(excerpt.starts_with("properties: "), "got: {excerpt}");
        assert!(excerpt.contains("field_00"), "got: {excerpt}");
        assert!(
            excerpt.contains("...("),
            "overflow marker expected: {excerpt}"
        );

        let bare = json!({"type": "object"});
        assert_eq!(
            malformed_tool_schema_excerpt(&bare),
            "{\"type\":\"object\"}"
        );
    }

    #[test]
    fn correction_includes_expected_schema_and_unknown_tool_note() {
        let tools = vec![ToolDefinition::new(
            "capture",
            "Capture test input",
            json!({"type":"object","properties":{"value":{"type":"boolean"}},"required":["value"]}),
        )];
        let invalid = vec![MalformedToolCallInfo {
            name: "capture".into(),
            error: "Invalid tool call for 'capture': arguments must be a JSON object, got null."
                .into(),
        }];
        let reminder = Agent::malformed_tool_call_correction(&invalid, &tools);
        assert!(reminder.starts_with("<system-reminder>"));
        assert!(reminder.ends_with("</system-reminder>"));
        assert!(reminder.contains("nothing was executed"));
        assert!(reminder.contains("arguments must be a JSON object"));
        assert!(reminder.contains("required: value"));
        assert!(reminder.contains("value:boolean"));
        assert!(reminder.contains("Do not send null arguments"));

        let unknown = vec![MalformedToolCallInfo {
            name: "ghost".into(),
            error: "Invalid tool call for 'ghost': arguments must be a JSON object, got null."
                .into(),
        }];
        let reminder = Agent::malformed_tool_call_correction(&unknown, &tools);
        assert!(reminder.contains("'ghost' is not an available tool"));
    }

    #[test]
    fn terminal_error_keeps_stable_prefix_and_actionable_guidance() {
        let message = Agent::malformed_tool_call_terminal_error(
            "glm-5.3",
            3,
            "Invalid tool call for 'write': arguments must be a JSON object, got null.",
        );
        assert!(message.starts_with(Agent::MALFORMED_TOOL_CALL_ERROR_PREFIX));
        assert!(message.starts_with("Invalid tool calls:"));
        assert!(message.contains("glm-5.3"));
        assert!(message.contains("3 consecutive rounds"));
        assert!(message.contains("arguments must be a JSON object"));
        assert!(message.contains("Re-send your message"));
        assert!(message.contains("/model"));
    }

    #[tokio::test]
    async fn consecutive_malformed_rounds_terminate_the_turn_with_stable_prefix() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        for streaming in [false, true] {
            let (mut agent, session_ids, exec_log) = scripted_agent(vec![
                malformed_round("m1", Some("sess-1")),
                malformed_round("m2", Some("sess-1")),
                malformed_round("m3", Some("sess-1")),
                // A fourth round exists in the script; requesting it would
                // prove the loop is unbounded.
                malformed_round("m4", None),
            ])
            .await;

            let error = run_scripted_turn(&mut agent, streaming)
                .await
                .expect_err("terminal failure once the malformed cap is reached");
            let message = error.to_string();
            assert!(
                message.starts_with("Invalid tool calls:"),
                "stable prefix missing (streaming={streaming}): {message}"
            );
            assert!(message.contains("glm-test"), "got: {message}");
            assert!(message.contains("3 consecutive rounds"), "got: {message}");
            assert!(
                message.contains("arguments must be a JSON object, got null."),
                "got: {message}"
            );
            assert!(message.contains("/model"), "got: {message}");

            // Bounded spend: exactly one request per malformed round, and the
            // fourth scripted round is never requested.
            assert_eq!(
                session_ids.lock().unwrap().len(),
                Agent::MAX_MALFORMED_TOOL_CALL_ROUNDS as usize,
                "streaming={streaming}"
            );
            // The provider session id carried by round 3 is None: it was
            // reset once after the second validation-only round failed.
            assert_eq!(
                *session_ids.lock().unwrap(),
                vec![None, Some("sess-1".into()), None],
                "streaming={streaming}"
            );

            // Null arguments were never executed and never fabricated.
            assert!(exec_log.lock().unwrap().is_empty(), "streaming={streaming}");
            // The invalid tool_use blocks and their error tool results stay
            // in history so the transcript and any resume stay consistent.
            assert_eq!(null_tool_use_count(&agent), 3, "streaming={streaming}");
            assert_eq!(error_tool_result_count(&agent), 3, "streaming={streaming}");
            // Two schema-guided correction attempts were injected before the
            // terminal round.
            let reminders = correction_reminders(&agent);
            assert_eq!(reminders.len(), 2, "streaming={streaming}");
            assert!(reminders[0].contains("capture"), "streaming={streaming}");
            assert!(
                reminders[0].contains("required: value"),
                "streaming={streaming}"
            );
            assert!(
                reminders[0].contains("single JSON object"),
                "streaming={streaming}"
            );
        }
    }

    #[tokio::test]
    async fn valid_tool_calls_reset_the_malformed_round_bound() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        for streaming in [false, true] {
            let (mut agent, _session_ids, exec_log) = scripted_agent(vec![
                malformed_round("m1", None),
                malformed_round("m2", None),
                valid_round("v1", true, None),
                malformed_round("m3", None),
                malformed_round("m4", None),
                valid_round("v2", false, None),
                final_text_round(),
            ])
            .await;

            run_scripted_turn(&mut agent, streaming)
                .await
                .expect("turn completes: malformed streaks never reach the cap");

            // Only the valid calls executed, with the exact arguments the
            // model sent: nothing null was executed, nothing fabricated.
            assert_eq!(
                *exec_log.lock().unwrap(),
                vec![json!({"value": true}), json!({"value": false})],
                "streaming={streaming}"
            );
            // One correction per validation-only round (two streaks of two).
            assert_eq!(
                correction_reminders(&agent).len(),
                4,
                "streaming={streaming}"
            );
            assert_eq!(null_tool_use_count(&agent), 4, "streaming={streaming}");
            assert_eq!(error_tool_result_count(&agent), 4, "streaming={streaming}");
        }
    }

    #[tokio::test]
    async fn mixed_round_counts_as_progress_and_resets_the_bound() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        // Round 2 is mixed (one malformed + one valid call): it must not
        // count toward the validation-only streak, and its valid call resets
        // the streak. If it counted, rounds 1-4 would hit the cap of 3 and
        // the turn would fail.
        let (mut agent, _session_ids, exec_log) = scripted_agent(vec![
            malformed_round("m1", None),
            mixed_round("bad", "good"),
            malformed_round("m2", None),
            malformed_round("m3", None),
            final_text_round(),
        ])
        .await;

        agent
            .run_turn(false)
            .await
            .expect("mixed rounds must not exhaust the malformed bound");

        // Only the valid call in the mixed round executed.
        assert_eq!(*exec_log.lock().unwrap(), vec![json!({"value": true})]);
        // Corrections were injected after rounds 1, 3, and 4 - none after the
        // mixed round.
        let reminders = correction_reminders(&agent);
        assert_eq!(reminders.len(), 3);
        assert!(reminders[0].contains("capture"));
        // Four malformed calls total (rounds 1, 2, 3, 4) all have their error
        // tool results preserved in history.
        assert_eq!(null_tool_use_count(&agent), 4);
        assert_eq!(error_tool_result_count(&agent), 4);
    }
}
