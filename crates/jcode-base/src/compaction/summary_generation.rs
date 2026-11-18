use super::*;

/// Generate summary using the provider
pub(super) async fn generate_compaction_artifact(
    provider: Arc<dyn Provider>,
    messages: Vec<Message>,
    mut existing_summary: Option<Summary>,
) -> Result<CompactionResult> {
    let start = Instant::now();
    if let Some(summary) = existing_summary.as_mut()
        && let Some(encrypted_content) = summary.openai_encrypted_content.as_ref()
        && !openai_encrypted_content_is_sendable(encrypted_content)
    {
        let encrypted_content_len = encrypted_content.len();
        crate::logging::warn(&format!(
            "[compaction] Existing OpenAI native compaction payload is oversized ({} chars); falling back to text summary",
            encrypted_content_len,
        ));
        summary.openai_encrypted_content = None;
        let fallback = openai_encrypted_content_fallback_summary(encrypted_content_len);
        if summary.text.trim().is_empty() {
            summary.text = fallback;
        } else if !summary
            .text
            .contains("OpenAI native compaction state was discarded")
        {
            summary.text.push_str("\n\n");
            summary.text.push_str(&fallback);
        }
    }

    if let Ok(native) = provider
        .native_compact(
            &messages,
            existing_summary
                .as_ref()
                .map(|summary| summary.text.as_str()),
            existing_summary
                .as_ref()
                .and_then(|summary| summary.openai_encrypted_content.as_deref()),
        )
        .await
    {
        if let Some(usage) = native.usage {
            crate::telemetry::record_simple_completion_usage(
                None,
                provider.name(),
                &provider.model(),
                crate::telemetry::UsageSource::Compaction,
                usage,
            );
        }
        if let Some(encrypted_content) = native.openai_encrypted_content.as_ref()
            && !openai_encrypted_content_is_sendable(encrypted_content)
        {
            crate::logging::warn(&format!(
                "[compaction] OpenAI native compaction returned oversized encrypted_content ({} chars); falling back to text summary",
                encrypted_content.len(),
            ));
        } else if native.openai_encrypted_content.is_some() || native.summary_text.is_some() {
            return Ok(CompactionResult {
                summary_text: native.summary_text.unwrap_or_default(),
                openai_encrypted_content: native.openai_encrypted_content,
                covers_up_to_turn: messages.len(),
                duration_ms: start.elapsed().as_millis() as u64,
                summarized_messages: messages.len(),
            });
        }
    }

    let max_prompt_chars = provider.context_window().saturating_sub(4000) * CHARS_PER_TOKEN;
    let prompt = build_compaction_prompt(&messages, existing_summary.as_ref(), max_prompt_chars);

    // Generate summary using simple completion
    let (summary, usage) = provider
        .complete_simple_with_usage(
            &prompt,
            "You are a helpful assistant that summarizes conversations.",
        )
        .await
        .inspect_err(|error| {
            crate::telemetry::record_simple_completion_failure_usage(
                None,
                crate::telemetry::UsageSource::Compaction,
                error,
            );
        })?;
    crate::telemetry::record_simple_completion_usage(
        None,
        provider.name(),
        &provider.model(),
        crate::telemetry::UsageSource::Compaction,
        usage,
    );

    Ok(CompactionResult {
        summary_text: validate_compaction_summary(summary)?,
        openai_encrypted_content: None,
        covers_up_to_turn: messages.len(),
        duration_ms: start.elapsed().as_millis() as u64,
        summarized_messages: messages.len(),
    })
}

/// Reject summaries that cannot stand in for the messages they replace, such
/// as empty output or a provider refusal (issue #2089). Returning an error makes
/// compaction keep the original messages instead of dropping them.
pub(super) fn validate_compaction_summary(summary: String) -> Result<String> {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        anyhow::bail!("compaction summary rejected: provider returned empty text");
    }
    if looks_like_refusal(trimmed) {
        let preview: String = trimmed.chars().take(120).collect();
        anyhow::bail!(
            "compaction summary rejected: provider refusal: {:?}",
            preview
        );
    }
    Ok(summary)
}

pub(super) fn looks_like_refusal(text: &str) -> bool {
    // Only inspect the opening so a real summary that mentions an earlier
    // refusal later on is still accepted.
    let head: String = text.chars().take(160).collect::<String>().to_lowercase();
    let head = head.replace('\u{2019}', "'");
    const PATTERNS: [&str; 8] = [
        "i'm sorry, but i can",
        "i am sorry, but i can",
        "sorry, but i can't",
        "sorry, i can't",
        "i cannot assist with",
        "i can't assist with",
        "i can't help with",
        "i'm unable to help with",
    ];
    // Short replies are refusals if they contain a pattern anywhere. Longer
    // ones only if they open with it.
    if text.chars().count() < 300 {
        PATTERNS.iter().any(|pattern| head.contains(pattern))
    } else {
        PATTERNS.iter().any(|pattern| head.starts_with(pattern))
    }
}
