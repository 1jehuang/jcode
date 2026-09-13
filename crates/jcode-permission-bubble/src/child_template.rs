//! Child message template with 10 strict rules.
//!
//! Mirrors Claude Code's `buildChildMessage` from `forkSubagent.ts`:
//! wraps a parent directive in a boilerplate envelope that constrains the
//! child agent to safe, scoped, non-recursive behaviour.

/// XML tag wrapping the boilerplate block — used for detection in history.
pub const FORK_BOILERPLATE_TAG: &str = "fork-boilerplate";

/// Prefix before the directive text, stripped by the renderer.
pub const FORK_DIRECTIVE_PREFIX: &str = "Your directive: ";

/// The 10 non-negotiable rules enforced on every child.
const RULES: [&str; 10] = [
    "Do NOT spawn sub-agents; execute directly.",
    "Do NOT converse, ask questions, or suggest next steps.",
    "USE your tools directly: Bash, Read, Write, etc.",
    "If you modify files, commit your changes before reporting.",
    "Keep your report under 500 words.",
    "REPORT structured facts, then stop.",
    "Stay strictly within your directive's scope.",
    "Do NOT emit text between tool calls.",
    "Your response MUST begin with \"Scope:\".",
    "Do NOT fork; you ARE the fork.",
];
/// Structured output format appended after the rules.
const OUTPUT_FORMAT: &str = "\
Output format (plain text labels, not markdown headers):
  Scope: <echo back your assigned scope in one sentence>
  Result: <the answer or key findings, limited to the scope above>
  Key files: <relevant file paths — include for research tasks>
  Files changed: <list with commit hash — include only if you modified files>
  Issues: <list — include only if there are issues to flag>";

/// Builder for constructing child templates via a fluent API.
#[derive(Debug, Clone)]
pub struct ChildTemplate {
    system_message: String,
    task_description: String,
}

impl ChildTemplate {
    /// Create a builder from a parent directive string.
    pub fn builder(directive: &str) -> Self {
        Self {
            system_message: String::new(),
            task_description: directive.to_string(),
        }
    }

    /// Add a custom preamble before the standard rules.
    pub fn preamble(mut self, text: &str) -> Self {
        self.system_message.push_str(text);
        self.system_message.push_str("\n\n");
        self
    }

    /// Consume the builder and produce the final `ChildMessage`.
    pub fn build(self) -> ChildMessage {
        let mut body = String::from(
            "STOP. READ THIS FIRST.\n\n\
             You are a forked worker process. You are NOT the main agent.\n\n\
             RULES (non-negotiable):\n",
        );
        for (i, rule) in RULES.iter().enumerate() {
            body.push_str(&format!("{}. {}\n", i + 1, rule));
        }
        body.push('\n');
        body.push_str(OUTPUT_FORMAT);

        let system_message = if self.system_message.is_empty() {
            body
        } else {
            format!("{}\n\n{}", self.system_message.trim_end(), body)
        };

        ChildMessage { system_message, task_description: self.task_description }
    }
}
/// A fully-formed child message ready for a provider API call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildMessage {
    pub system_message: String,
    pub task_description: String,
}

/// Build the complete child message string matching the Claude Code envelope.
///
/// Output: `<fork-boilerplate>\n...\n</fork-boilerplate>\nYour directive: <directive>`
pub fn build_child_message(directive: &str) -> String {
    let child = ChildTemplate::builder(directive).build();
    format!(
        "<{tag}>\n{system}\n</{tag}>\n{prefix}{directive}",
        tag = FORK_BOILERPLATE_TAG,
        system = child.system_message,
        prefix = FORK_DIRECTIVE_PREFIX,
        directive = child.task_description,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_child_message_wraps_with_tag() {
        let msg = build_child_message("fix the bug");
        assert!(msg.starts_with("<fork-boilerplate>"));
        assert!(msg.ends_with("Your directive: fix the bug"));
        assert!(msg.contains("</fork-boilerplate>"));
    }

    #[test]
    fn all_ten_rules_present_and_numbered() {
        let msg = build_child_message("test");
        for (i, rule) in RULES.iter().enumerate() {
            assert!(msg.contains(&format!("{}. {}", i + 1, rule)), "Rule {}: {rule}", i + 1);
        }
    }

    #[test]
    fn output_format_section_present() {
        let msg = build_child_message("test");
        for label in ["Scope:", "Result:", "Key files:", "Files changed:", "Issues:"] {
            assert!(msg.contains(label), "Missing label: {label}");
        }
    }

    #[test]
    fn builder_with_preamble() {
        let child = ChildTemplate::builder("do work")
            .preamble("Custom preamble here.")
            .build();
        assert!(child.system_message.starts_with("Custom preamble here."));
        assert!(child.system_message.contains("RULES (non-negotiable):"));
    }

    #[test]
    fn child_message_struct_fields() {
        let child = ChildTemplate::builder("my task").build();
        assert_eq!(child.task_description, "my task");
        assert!(child.system_message.contains("forked worker process"));
        assert_eq!(FORK_BOILERPLATE_TAG, "fork-boilerplate");
        assert_eq!(FORK_DIRECTIVE_PREFIX, "Your directive: ");
    }
}
