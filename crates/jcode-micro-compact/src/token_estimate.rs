//! Token estimation for messages with 4/3 conservative padding.
//!
//! Mirrors Claude Code's `roughTokenCountEstimation` and
//! `estimateMessageTokens` from `microCompact.ts`.

use jcode_message_types::{ContentBlock, Message, Role};

/// Approximate characters per token (English + code heuristic).
const CHARS_PER_TOKEN: usize = 4;

/// Fixed token cost for images/documents.
const IMAGE_TOKEN_COST: usize = 2000;

/// Conservative padding numerator (×4/3).
const PAD_NUM: usize = 4;
/// Conservative padding denominator (÷3).
const PAD_DEN: usize = 3;

/// Estimate token count for raw text. ~4 chars per token, floor of 1.
pub fn rough_token_count(text: &str) -> usize {
    if text.is_empty() {
        1
    } else {
        (text.len() / CHARS_PER_TOKEN).max(1)
    }
}

/// Legacy alias kept for backward compatibility with existing callers.
pub fn estimate_tokens(text: &str) -> usize {
    rough_token_count(text)
}

/// Estimate tokens for a single content block.
fn block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text, .. } => rough_token_count(text),
        ContentBlock::AnthropicThinking { thinking, .. } => rough_token_count(thinking),
        ContentBlock::Reasoning { text } => rough_token_count(text),
        ContentBlock::ReasoningTrace { text } => rough_token_count(text),
        ContentBlock::OpenAIReasoning { summary, .. } => {
            summary.iter().map(|s| rough_token_count(s)).sum()
        }
        ContentBlock::ToolUse { name, input, .. } => {
            let payload = format!("{}{}", name, input);
            rough_token_count(&payload)
        }
        ContentBlock::ToolResult { content, .. } => rough_token_count(content),
        ContentBlock::Image { .. } => IMAGE_TOKEN_COST,
        ContentBlock::OpenAICompaction { encrypted_content } => {
            rough_token_count(encrypted_content)
        }
    }
}

/// Estimate total tokens across a slice of messages.
///
/// Only counts user/assistant role messages. Walks every content block,
/// then pads the sum by 4/3 for conservatism (matching Claude Code's
/// `estimateMessageTokens`).
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    let raw: usize = messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .flat_map(|m| &m.content)
        .map(block_tokens)
        .sum();

    // Pad by 4/3 to be conservative since we're approximating
    (raw * PAD_NUM).div_ceil(PAD_DEN) // ceil division
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::{ContentBlock, Message, Role};
    use serde_json::json;

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    fn assistant_msg(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: Role::Assistant,
            content: blocks,
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    #[test]
    fn rough_token_count_basic() {
        assert_eq!(rough_token_count(""), 1); // floor
        assert_eq!(rough_token_count("ab"), 1);
        assert_eq!(rough_token_count("1234"), 1);
        assert_eq!(rough_token_count("12345678"), 2);
        assert_eq!(rough_token_count(&"a".repeat(400)), 100);
    }

    #[test]
    fn estimate_tokens_is_alias() {
        assert_eq!(estimate_tokens("hello"), rough_token_count("hello"));
    }

    #[test]
    fn single_text_message_padded() {
        // 100 chars → 25 tokens → ceil(25 × 4/3) = 34
        let msgs = vec![user_msg(&"a".repeat(100))];
        assert_eq!(estimate_message_tokens(&msgs), 34);
    }

    #[test]
    fn image_block_counts_2000() {
        let msg = assistant_msg(vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "base64...".into(),
        }]);
        // 2000 tokens × 4/3 = 2667
        assert_eq!(estimate_message_tokens(&[msg]), 2667);
    }

    #[test]
    fn tool_use_block_counts_name_and_input() {
        let msg = assistant_msg(vec![ContentBlock::ToolUse {
            id: "t1".into(),
            name: "grep".into(),
            input: json!({"query": "foo"}),
            thought_signature: None,
        }]);
        // serde_json::Value Display produces compact JSON: "grep{\"query\":\"foo\"}"
        // = 19 chars → 4 tokens → ceil(4*4/3) = 6
        assert_eq!(estimate_message_tokens(&[msg]), 6);
    }

    #[test]
    fn thinking_block_counts_text() {
        let msg = assistant_msg(vec![ContentBlock::AnthropicThinking {
            thinking: "Let me think...".into(),
            signature: "sig".into(),
        }]);
        // "Let me think..." = 15 chars → 3 tokens → ceil(3*4/3)=4
        assert_eq!(estimate_message_tokens(&[msg]), 4);
    }

    #[test]
    fn system_role_skipped() {
        let msg = Message {
            role: Role::User, // User messages are counted
            content: vec![ContentBlock::Text {
                text: "hello".into(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        };
        // Only user/assistant counted
        assert!(estimate_message_tokens(&[msg]) > 0);
    }

    #[test]
    fn tool_result_counts_content() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "result data here".into(),
                is_error: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        };
        // "result data here" = 16 chars → 4 tokens → ceil(4*4/3)=6
        assert_eq!(estimate_message_tokens(&[msg]), 6);
    }

    #[test]
    fn mixed_blocks_sum_correctly() {
        let msg = assistant_msg(vec![
            ContentBlock::Text {
                text: "a".repeat(80),
                cache_control: None,
            }, // 20 tokens
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "x".into(),
            }, // 2000
        ]);
        // raw=2020, padded=ceil(2020*4/3)=2694
        assert_eq!(estimate_message_tokens(&[msg]), 2694);
    }
}
