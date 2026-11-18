use super::{Provider, SimpleCompletionError, SimpleCompletionUsage, nonempty_response_model};
use anyhow::Result;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};

/// Drive `Provider::complete` for a single user prompt and collect the text and
/// provider-reported usage.
pub async fn collect_simple_completion<P: Provider + ?Sized>(
    provider: &P,
    prompt: &str,
    system: &str,
) -> Result<(String, SimpleCompletionUsage)> {
    use futures::StreamExt;

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt.to_string(),
            cache_control: None,
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    }];

    // Cache counter semantics belong to the API transport, not the reseller
    // or compute backend named by an UpstreamProvider event.
    let provider_name = provider.name().to_string();
    let model = provider.model();
    let response = provider.complete(&messages, &[], system, None).await?;
    let mut result = String::new();
    let mut usage = SimpleCompletionUsage {
        requested_model: Some(model.clone()),
        ..Default::default()
    };
    tokio::pin!(response);

    while let Some(event) = response.next().await {
        match event {
            Ok(StreamEvent::TextDelta(text)) => result.push_str(&text),
            Ok(StreamEvent::TextSuffixReplace { old_text, new_text }) => {
                jcode_message_types::replace_text_suffix(&mut result, &old_text, &new_text);
            }
            Ok(StreamEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            }) => {
                usage.observe(
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                );
            }
            Ok(StreamEvent::ResolvedModel { model }) => {
                if let Some(model) = nonempty_response_model(Some(&model)) {
                    usage.resolved_model = Some(model.to_owned());
                }
            }
            Ok(StreamEvent::RetryRollback { .. }) => {
                result.clear();
                usage = SimpleCompletionUsage {
                    requested_model: Some(model.clone()),
                    ..Default::default()
                };
            }
            Ok(StreamEvent::UpstreamProvider { .. }) => {}
            Ok(StreamEvent::MessageEnd { stop_reason })
                if super::is_guardrail_stop_reason(stop_reason.as_deref()) =>
            {
                // A refusal or content-filter stop is not a usable answer. Callers
                // such as compaction would otherwise persist the refusal text as
                // if it were real output (issue #2089).
                return Err(SimpleCompletionError {
                    usage,
                    provider: provider_name,
                    model,
                    source: anyhow::anyhow!(
                        "simple completion stopped by provider guardrail (stop_reason={})",
                        stop_reason.as_deref().unwrap_or("")
                    ),
                }
                .into());
            }
            Ok(StreamEvent::Error { message, .. }) => {
                return Err(SimpleCompletionError {
                    usage,
                    provider: provider_name,
                    model,
                    source: anyhow::anyhow!(message),
                }
                .into());
            }
            Ok(_) => {}
            Err(source) => {
                return Err(SimpleCompletionError {
                    usage,
                    provider: provider_name,
                    model,
                    source,
                }
                .into());
            }
        }
    }

    Ok((result, usage))
}
