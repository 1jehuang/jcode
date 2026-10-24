use crate::message::ToolCall;

/// Keep raw JSON separate for every in-flight call. Unkeyed providers address
/// the most recently started call, preserving the legacy sequential behavior.
#[derive(Default)]
pub(super) struct PendingStreamingTools {
    calls: Vec<(ToolCall, String)>,
}

impl PendingStreamingTools {
    pub(super) fn start(&mut self, tool: ToolCall) {
        self.calls.push((tool, String::new()));
    }

    pub(super) fn input(&mut self, id: Option<&str>, delta: &str) {
        let call = match id {
            Some(id) => self.calls.iter_mut().find(|(tool, _)| tool.id == id),
            None => self.calls.last_mut(),
        };
        if let Some((_, input)) = call {
            input.push_str(delta);
        }
    }

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

    pub(super) fn signature(&mut self, id: &str, signature: &str) {
        if let Some((tool, _)) = self.calls.iter_mut().find(|(tool, _)| tool.id == id) {
            tool.thought_signature = Some(signature.to_owned());
        }
    }

    pub(super) fn drain(&mut self) -> impl Iterator<Item = ToolCall> + '_ {
        self.calls.drain(..).map(|(mut tool, input)| {
            tool.input = ToolCall::parse_streamed_input_to_object(&input);
            tool.refresh_intent_from_input();
            tool
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.calls.clear();
    }
}
