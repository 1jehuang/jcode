//! Pending streamed tool-call assembly.

use super::*;

impl PendingStreamingTools {
    pub(super) fn finish(&mut self, id: Option<&str>) -> Option<ToolCall> {
        let index = match id {
            Some(id) => self.calls.iter().position(|(tool, _)| tool.id == id)?,
            None => self.calls.len().checked_sub(1)?,
        };
        let (mut tool, input) = self.calls.remove(index);
        tool.input = ToolCall::parse_streamed_input_to_object(&input);
        tool.refresh_intent_from_input();
        Some(tool)
    }
}

impl PendingStreamingTools {
    pub(super) fn input(&mut self, id: Option<&str>, delta: &str) {
        let call = match id {
            Some(id) => self.calls.iter_mut().find(|(tool, _)| tool.id == id),
            None => self.calls.last_mut(),
        };
        if let Some((_, input)) = call {
            input.push_str(delta);
        }
    }
}
