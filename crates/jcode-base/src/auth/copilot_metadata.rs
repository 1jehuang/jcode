use super::CopilotModelInfo;

impl CopilotModelInfo {
    pub fn metadata(&self) -> jcode_provider_core::ModelMetadata {
        let limits = self
            .capabilities
            .as_ref()
            .and_then(|caps| caps.limits.as_ref());
        let supports = self
            .capabilities
            .as_ref()
            .and_then(|caps| caps.supports.as_ref());
        jcode_provider_core::ModelMetadata {
            context_window: limits
                .and_then(|limits| limits.max_context_window_tokens)
                .filter(|value| *value > 0),
            max_input_tokens: limits
                .and_then(|limits| limits.max_prompt_tokens)
                .filter(|value| *value > 0),
            max_output_tokens: limits
                .and_then(|limits| limits.max_output_tokens)
                .filter(|value| *value > 0),
            max_non_streaming_output_tokens: limits
                .and_then(|limits| limits.max_non_streaming_output_tokens)
                .filter(|value| *value > 0),
            reasoning_efforts: self.reasoning_efforts(),
            reasoning_effort_labels: Default::default(),
            supports_image_input: supports.and_then(|supports| supports.vision),
            supports_tool_calls: supports.and_then(|supports| supports.tool_calls),
            supports_parallel_tool_calls: supports
                .and_then(|supports| supports.parallel_tool_calls),
            supports_streaming: supports.and_then(|supports| supports.streaming),
            supports_structured_outputs: supports.and_then(|supports| supports.structured_outputs),
            supports_adaptive_thinking: supports.and_then(|supports| supports.adaptive_thinking),
            supported_endpoints: self.supported_endpoints.clone(),
        }
    }
}
