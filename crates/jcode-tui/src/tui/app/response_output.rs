/// Output of the current provider response, independent of the render buffer.
/// Compaction may commit that buffer before the turn's Done event arrives.
#[derive(Clone, Debug, Default)]
pub(super) struct ResponseOutput {
    has_text: bool,
    has_tools: bool,
    ended_normally: bool,
}

impl ResponseOutput {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn text(&mut self, text: &str) {
        self.has_text |= !text.trim().is_empty();
    }

    pub(super) fn replace_text(&mut self, text: &str) {
        self.has_text = !text.trim().is_empty();
    }

    pub(super) fn tool(&mut self) {
        self.has_tools = true;
    }

    pub(super) fn finish(&mut self, stop_reason: Option<&str>) {
        // Unknown, truncated, tool-use, and guardrail endings are not evidence
        // that a final answer was delivered. None is normal for some providers.
        self.ended_normally = matches!(
            stop_reason,
            None | Some("end_turn" | "stop" | "stop_sequence" | "completed")
        );
    }

    pub(super) fn has_final_response(&self) -> bool {
        self.has_text && !self.has_tools && self.ended_normally
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_response_requires_text_without_tools_and_a_normal_end() {
        for reason in [None, Some("end_turn"), Some("stop"), Some("completed")] {
            let mut output = ResponseOutput::default();
            output.text(" \n");
            output.finish(reason);
            assert!(!output.has_final_response());
            output.text("Done.");
            assert!(output.has_final_response());
            output.tool();
            assert!(!output.has_final_response());
            output.text("Commentary after a tool call");
            assert!(!output.has_final_response());
            output.reset();
            assert!(!output.has_final_response());
        }
        for reason in [
            "max_tokens",
            "length",
            "refusal",
            "tool_use",
            "pause_turn",
            "unknown",
        ] {
            let mut output = ResponseOutput::default();
            output.text("Partial answer");
            output.finish(Some(reason));
            assert!(!output.has_final_response(), "{reason}");
        }
    }

    #[test]
    fn replacement_can_remove_an_answer() {
        let mut output = ResponseOutput::default();
        output.text("Tool markup, not an answer");
        output.finish(None);
        output.replace_text("");
        assert!(!output.has_final_response());
    }
}
