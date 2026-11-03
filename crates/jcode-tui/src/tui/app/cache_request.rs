use super::*;

impl App {
    pub(super) fn begin_kv_cache_request(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
    ) {
        let turn_number = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
            .max(1);
        if self.kv_cache.kv_cache_turn_number == Some(turn_number) {
            self.kv_cache.kv_cache_turn_call_index = self
                .kv_cache
                .kv_cache_turn_call_index
                .saturating_add(1)
                .max(1);
        } else {
            self.kv_cache.kv_cache_turn_number = Some(turn_number);
            self.kv_cache.kv_cache_turn_call_index = 1;
        }

        let baseline = self.kv_cache_baseline_for_current_session();
        let signature =
            Self::kv_cache_request_signature(messages, tools, system_static, system_dynamic);
        let baseline_messages_prefix_matches = baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref())
            .map(|previous| Self::kv_cache_signatures_prefix_match(&signature, previous));

        self.maybe_push_cold_cache_warning(
            turn_number,
            self.kv_cache.kv_cache_turn_call_index,
            baseline.as_ref(),
        );
        self.pause_streaming_tps(false);
        self.kv_cache.current_api_usage_recorded = false;
        self.mark_stream_usage_call_boundary();

        self.kv_cache.pending_kv_cache_request = Some(PendingKvCacheRequest {
            started_at: Some(Instant::now()),
            utc_day: crate::kv_cache_monitor::utc_day_now(),
            turn_number,
            call_index: self.kv_cache.kv_cache_turn_call_index,
            cache_ttl_secs: crate::tui::cache_ttl_for_provider_model(
                &self.kv_cache_provider_name(),
                Some(&self.kv_cache_provider_model()),
            ),
            provider: self.kv_cache_provider_name(),
            model: self.kv_cache_provider_model(),
            upstream_provider: self.upstream_provider.clone(),
            account: self.kv_cache_account(),
            signature: Some(signature),
            baseline_messages_prefix_matches,
            baseline,
            cache_generation: self.kv_cache.cache_generation,
        });
    }
}
