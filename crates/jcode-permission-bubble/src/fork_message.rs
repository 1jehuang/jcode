//! Fork message construction for cache-sharing prefix alignment.
use serde::{Deserialize, Serialize};

use crate::child_template::{build_child_message, FORK_BOILERPLATE_TAG};

/// Placeholder text for all tool_result blocks in the fork prefix.
pub const FORK_PLACEHOLDER_RESULT: &str = "Fork started \u{2014} processing in background";

/// Content block: text, tool_use, or tool_result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: Vec<ContentBlock> },
}

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

pub type AssistantMessage = Message;

/// A fork payload with prefix messages for cache sharing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkPayload {
    pub prefix_messages: Vec<ForkMessage>,
    pub divergence_point: usize,
    pub cache_partition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkMessage {
    pub role: String,
    pub content: String,
    pub original_hash: Option<String>,
}

/// Builder for constructing fork payloads with prefix integrity.
#[derive(Debug, Clone)]
pub struct ForkMessageBuilder {
    prefix: Vec<ForkMessage>,
    cache_partition: String,
}

impl ForkMessageBuilder {
    pub fn new(cache_partition: impl Into<String>) -> Self {
        Self { prefix: Vec::new(), cache_partition: cache_partition.into() }
    }

    pub fn push_prefix(mut self, role: impl Into<String>, content: impl Into<String>) -> Self {
        self.prefix.push(ForkMessage {
            role: role.into(), content: content.into(), original_hash: None,
        });
        self
    }

    pub fn build(self) -> ForkPayload {
        let divergence_point = self.prefix.len();
        ForkPayload { prefix_messages: self.prefix, divergence_point, cache_partition: self.cache_partition }
    }
}

impl ForkPayload {
    pub fn prefix_byte_len(&self) -> usize {
        self.prefix_messages.iter().map(|m| m.content.len()).sum()
    }

    pub fn shares_prefix_with(&self, other: &Self) -> bool {
        self.prefix_messages.len() == other.prefix_messages.len()
            && self.prefix_messages.iter().zip(&other.prefix_messages)
                .all(|(a, b)| a.role == b.role && a.content == b.content)
    }
}

/// Build forked messages for a child agent. Returns `[assistant_clone, user_msg]`
/// where user_msg has placeholder tool_results + per-child directive.
pub fn build_forked_messages(directive: &str, assistant: &AssistantMessage) -> Vec<Message> {
    let tool_uses: Vec<_> = assistant.content.iter().filter_map(|b| match b {
        ContentBlock::ToolUse { id, .. } => Some(id.clone()),
        _ => None,
    }).collect();

    if tool_uses.is_empty() {
        return vec![Message {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: build_child_message(directive) }],
        }];
    }

    let mut blocks: Vec<ContentBlock> = tool_uses.iter()
        .map(|id| ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: vec![ContentBlock::Text { text: FORK_PLACEHOLDER_RESULT.into() }],
        })
        .collect();
    blocks.push(ContentBlock::Text { text: build_child_message(directive) });

    vec![assistant.clone(), Message { role: "user".into(), content: blocks }]
}

/// Check whether a message list contains fork child boilerplate.
pub fn is_in_fork_child(messages: &[Message]) -> bool {
    messages.iter().any(|m| m.role == "user" && m.content.iter().any(|b| match b {
        ContentBlock::Text { text } => text.contains(&format!("<{FORK_BOILERPLATE_TAG}>")),
        _ => false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::child_template::FORK_DIRECTIVE_PREFIX;

    fn tu(id: &str) -> ContentBlock {
        ContentBlock::ToolUse { id: id.into(), name: "read".into(), input: serde_json::json!({}) }
    }

    fn assistant() -> AssistantMessage {
        Message { role: "assistant".into(), content: vec![tu("tu-1"), ContentBlock::Text { text: "checking".into() }, tu("tu-2")] }
    }

    #[test]
    fn builder_divergence_point() {
        let p = ForkMessageBuilder::new("alpha")
            .push_prefix("system", "sys").push_prefix("user", "hi").build();
        assert_eq!(p.divergence_point, 2);
        assert_eq!(p.cache_partition, "alpha");
    }

    #[test]
    fn shared_prefix_detected() {
        let a = ForkMessageBuilder::new("p1").push_prefix("user", "x").build();
        let b = ForkMessageBuilder::new("p2").push_prefix("user", "x").build();
        assert!(a.shares_prefix_with(&b));
    }

    #[test]
    fn differing_prefix() {
        let a = ForkMessageBuilder::new("p1").push_prefix("user", "a").build();
        let b = ForkMessageBuilder::new("p2").push_prefix("user", "b").build();
        assert!(!a.shares_prefix_with(&b));
    }

    #[test]
    fn prefix_byte_len() {
        let p = ForkMessageBuilder::new("p").push_prefix("s", "12345").push_prefix("u", "123").build();
        assert_eq!(p.prefix_byte_len(), 8);
    }

    #[test]
    fn forked_with_tool_uses() {
        let msgs = build_forked_messages("fix bug", &assistant());
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "assistant");
        assert_eq!(msgs[1].role, "user");
        // 2 tool_results + 1 text
        assert_eq!(msgs[1].content.len(), 3);
        if let ContentBlock::ToolResult { tool_use_id, content } = &msgs[1].content[0] {
            assert_eq!(tool_use_id, "tu-1");
            assert_eq!(content[0], ContentBlock::Text { text: FORK_PLACEHOLDER_RESULT.into() });
        } else { panic!("Expected ToolResult"); }
    }

    #[test]
    fn forked_no_tool_uses() {
        let a = Message { role: "assistant".into(), content: vec![ContentBlock::Text { text: "hi".into() }] };
        let msgs = build_forked_messages("do x", &a);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn child_message_has_boilerplate() {
        let msg = build_child_message("feat X");
        assert!(msg.contains(&format!("<{FORK_BOILERPLATE_TAG}>")));
        assert!(msg.contains(&format!("</{FORK_BOILERPLATE_TAG}>")));
        assert!(msg.contains("feat X"));
        assert!(msg.contains(FORK_DIRECTIVE_PREFIX));
    }

    #[test]
    fn is_in_fork_child_detection() {
        let fb = Message { role: "user".into(), content: vec![ContentBlock::Text { text: format!("<{FORK_BOILERPLATE_TAG}>x</{FORK_BOILERPLATE_TAG}>") }] };
        assert!(is_in_fork_child(&[fb]));
        let nm = Message { role: "user".into(), content: vec![ContentBlock::Text { text: "normal".into() }] };
        assert!(!is_in_fork_child(&[nm]));
    }

    #[test]
    fn cache_identical_across_directives() {
        let a = build_forked_messages("alpha", &assistant());
        let b = build_forked_messages("beta", &assistant());
        // Assistant messages identical
        assert_eq!(serde_json::to_string(&a[0]).unwrap(), serde_json::to_string(&b[0]).unwrap());
        // Tool results identical (first 2 blocks)
        for i in 0..2 {
            assert_eq!(serde_json::to_string(&a[1].content[i]).unwrap(), serde_json::to_string(&b[1].content[i]).unwrap());
        }
        // Directive text differs
        let ta = match &a[1].content[2] { ContentBlock::Text { text } => text.clone(), _ => panic!() };
        let tb = match &b[1].content[2] { ContentBlock::Text { text } => text.clone(), _ => panic!() };
        assert_ne!(ta, tb);
    }
}
