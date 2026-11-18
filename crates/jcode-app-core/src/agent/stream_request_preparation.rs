//! Provider transport prewarming and post-compaction request metadata.

use super::*;

impl Agent {
    pub(super) async fn prepare_stream_request(
        &mut self,
        event_tx: &crate::stream_batching::BatchingSender,
    ) -> (
        Vec<ToolDefinition>,
        crate::prompt::SplitSystemPrompt,
        Vec<Message>,
    ) {
        // Start provider transport setup before deriving and potentially
        // compacting the request history. This is the first point where the
        // stable request settings are available.
        let mut tools = self.tool_definitions().await;
        let mut split_prompt = self.build_system_prompt_split(None);
        self.provider
            .prewarm(&tools, &split_prompt.static_part)
            .await;
        let (messages, compaction_event) = self.messages_for_provider();
        if let Some(event) = compaction_event {
            // Reset cache tracker and tool lock on compaction since the message history changes
            self.cache_tracker.reset();
            self.kv_cache_monitor.reset();
            self.locked_tools = None;
            logging::info(&format!(
                "Context compacted ({}{})",
                event.trigger,
                event
                    .pre_tokens
                    .map(|t| format!(" {} tokens", t))
                    .unwrap_or_default()
            ));
            let _ = event_tx.send(ServerEvent::Compaction {
                trigger: event.trigger.clone(),
                pre_tokens: event.pre_tokens,
                post_tokens: event.post_tokens,
                tokens_saved: event.tokens_saved,
                duration_ms: event.duration_ms,
                messages_dropped: event.messages_dropped,
                messages_compacted: event.messages_compacted,
                summary_chars: event.summary_chars,
                active_messages: event.active_messages,
            });
            // Compaction clears the tool lock, so rebuild the foreground
            // request metadata rather than relying on the pre-compaction snapshot.
            tools = self.tool_definitions().await;
            split_prompt = self.build_system_prompt_split(None);
        }

        (tools, split_prompt, messages)
    }
}
