//! KV-cache usage diagnostics.

use super::*;

impl App {
    pub(super) fn log_kv_cache_usage_summary(
        &self,
        request: &PendingKvCacheRequest,
        optimal_input_tokens: Option<u64>,
        prompt_tokens: u64,
    ) {
        let input_tokens = self.streaming.streaming_input_tokens;
        let read_tokens = self.streaming.streaming_cache_read_tokens.unwrap_or(0);
        let creation_tokens = self.streaming.streaming_cache_creation_tokens.unwrap_or(0);
        let read_pct = ratio_pct(read_tokens, prompt_tokens);
        let creation_pct = ratio_pct(creation_tokens, prompt_tokens);
        let optimal_read_pct = optimal_input_tokens.map(|optimal| ratio_pct(read_tokens, optimal));
        let session_read_pct = ratio_pct(
            self.token_accounting.total_cache_read_tokens,
            self.token_accounting.total_cache_prompt_tokens,
        );
        let session_optimal_read_pct = if self.token_accounting.total_cache_optimal_input_tokens > 0
        {
            Some(ratio_pct(
                self.token_accounting.total_cache_read_tokens,
                self.token_accounting.total_cache_optimal_input_tokens,
            ))
        } else {
            None
        };
        let miss = self
            .kv_cache
            .kv_cache_miss_samples
            .last()
            .filter(|sample| {
                sample.turn_number == request.turn_number && sample.call_index == request.call_index
            })
            .map(|sample| {
                format!(
                    "{}:{}",
                    sample.missed_tokens,
                    sample.reason.label().replace(' ', "_")
                )
            })
            .unwrap_or_else(|| {
                if request.baseline.is_none() {
                    "warmup:no_baseline".to_string()
                } else {
                    "none".to_string()
                }
            });
        let baseline_age_secs = request
            .baseline
            .as_ref()
            .map(|baseline| baseline.completed_at.elapsed().as_secs());
        let baseline_input_tokens = request
            .baseline
            .as_ref()
            .map(|baseline| baseline.input_tokens);
        let missed_tokens =
            baseline_input_tokens.map(|baseline| baseline.saturating_sub(read_tokens));
        let ttl_secs = request
            .baseline
            .as_ref()
            .and_then(|baseline| baseline.cache_ttl_secs);
        let ttl_remaining_secs = ttl_secs
            .zip(baseline_age_secs)
            .map(|(ttl, age)| ttl.saturating_sub(age));
        let current_signature = request.signature.as_ref();
        let baseline_signature = request
            .baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref());
        let system_static_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.system_static_hash != baseline.system_static_hash);
        let tools_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.tools_hash != baseline.tools_hash);
        let message_full_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.messages_hash != baseline.messages_hash);
        let message_prefix_changed = request
            .baseline_messages_prefix_matches
            .map(|matches| !matches);
        let common_prefix_messages = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| Self::kv_cache_common_prefix_messages(current, baseline));
        let first_changed_message_index = common_prefix_messages
            .zip(baseline_signature.map(|signature| signature.message_count))
            .and_then(|(common, baseline_count)| (common < baseline_count).then_some(common));
        let dynamic_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.ephemeral_hash != baseline.ephemeral_hash);
        let current_message_count = current_signature.map(|signature| signature.message_count);
        let baseline_message_count = baseline_signature.map(|signature| signature.message_count);
        let current_tool_count = current_signature.map(|signature| signature.tool_count);
        let baseline_tool_count = baseline_signature.map(|signature| signature.tool_count);
        let current_system_chars = current_signature.map(|signature| signature.system_static_chars);
        let baseline_system_chars =
            baseline_signature.map(|signature| signature.system_static_chars);
        let current_tools_json_chars =
            current_signature.map(|signature| signature.tools_json_chars);
        let baseline_tools_json_chars =
            baseline_signature.map(|signature| signature.tools_json_chars);
        let current_messages_json_chars =
            current_signature.map(|signature| signature.messages_json_chars);
        let baseline_messages_json_chars =
            baseline_signature.map(|signature| signature.messages_json_chars);
        let current_ephemeral_chars = current_signature.map(|signature| signature.ephemeral_chars);
        let baseline_ephemeral_chars =
            baseline_signature.map(|signature| signature.ephemeral_chars);
        let current_ephemeral_message_count =
            current_signature.map(|signature| signature.ephemeral_message_count);
        let baseline_ephemeral_message_count =
            baseline_signature.map(|signature| signature.ephemeral_message_count);
        let current_hashes_present = current_signature
            .map(|signature| !signature.message_hashes.is_empty())
            .unwrap_or(false);
        let baseline_hashes_present = baseline_signature
            .map(|signature| !signature.message_hashes.is_empty())
            .unwrap_or(false);

        crate::logging::info(&format!(
            "KV_CACHE_USAGE: turn={} call={} provider={} label={} upstream={:?} model={} \
             input={} cache_read={} cache_write={} read_pct={} write_pct={} \
             optimal_input={:?} optimal_read_pct={:?} missed_tokens={:?} miss={} \
             session_input={} session_read={} session_write={} session_read_pct={} \
             session_optimal_input={} session_optimal_read_pct={:?} \
             baseline_input={:?} baseline_age_secs={:?} ttl_secs={:?} ttl_remaining_secs={:?} ttl_is_estimate={} \
             prefix_matches={:?} common_prefix_messages={:?} first_changed_message_index={:?} \
             system_changed={:?} tools_changed={:?} message_prefix_changed={:?} message_full_hash_changed={:?} dynamic_changed={:?} \
             message_count={:?} baseline_message_count={:?} tool_count={:?} baseline_tool_count={:?} \
             system_chars={:?} baseline_system_chars={:?} tools_json_chars={:?} baseline_tools_json_chars={:?} \
             messages_json_chars={:?} baseline_messages_json_chars={:?} ephemeral_chars={:?} baseline_ephemeral_chars={:?} \
             ephemeral_message_count={:?} baseline_ephemeral_message_count={:?} message_hashes_present={} baseline_message_hashes_present={} \
             connection={:?} status_detail={:?} {}",
            request.turn_number,
            request.call_index,
            request.provider,
            self.kv_cache_provider_label(),
            request.upstream_provider,
            request.model,
            input_tokens,
            read_tokens,
            creation_tokens,
            read_pct,
            creation_pct,
            optimal_input_tokens,
            optimal_read_pct,
            missed_tokens,
            miss,
            self.token_accounting.total_cache_reported_input_tokens,
            self.token_accounting.total_cache_read_tokens,
            self.token_accounting.total_cache_creation_tokens,
            session_read_pct,
            self.token_accounting.total_cache_optimal_input_tokens,
            session_optimal_read_pct,
            baseline_input_tokens,
            baseline_age_secs,
            ttl_secs,
            ttl_remaining_secs,
            request
                .baseline
                .as_ref()
                .is_some_and(|baseline| crate::provider::cache_ttl_is_estimate(&baseline.provider)),
            request.baseline_messages_prefix_matches,
            common_prefix_messages,
            first_changed_message_index,
            system_static_hash_changed,
            tools_hash_changed,
            message_prefix_changed,
            message_full_hash_changed,
            dynamic_hash_changed,
            current_message_count,
            baseline_message_count,
            current_tool_count,
            baseline_tool_count,
            current_system_chars,
            baseline_system_chars,
            current_tools_json_chars,
            baseline_tools_json_chars,
            current_messages_json_chars,
            baseline_messages_json_chars,
            current_ephemeral_chars,
            baseline_ephemeral_chars,
            current_ephemeral_message_count,
            baseline_ephemeral_message_count,
            current_hashes_present,
            baseline_hashes_present,
            self.connection_type.as_deref(),
            self.status_detail.as_deref(),
            self.kv_cache_diagnostic_fields(request, prompt_tokens),
        ));
    }
}
