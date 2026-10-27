use super::{Provider, SimpleCompletionUsage};
use anyhow::Result;
use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};

/// Drive `Provider::complete` for a single user prompt and collect the text and
/// provider-reported usage.
pub async fn collect_simple_completion<P: Provider + ?Sized>(
    provider: &P,
    prompt: &str,
    system: &str,
) -> Result<(String, SimpleCompletionUsage)> {
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: prompt.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];

    let response = provider.complete(&messages, &[], system, None).await?;
    let mut result = String::new();
    let mut usage = SimpleCompletionUsage::default();
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
            }) => usage.observe(
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            ),
            Ok(_) => {}
            Err(err) => return Err(err),
        }
    }

    Ok((result, usage))
}
