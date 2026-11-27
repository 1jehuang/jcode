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

    pub(super) fn kv_cache_request_signature(
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
    ) -> KvCacheRequestSignature {
        let dynamic_trimmed = system_dynamic.trim();
        // The dynamic block precedes the transcript, so it is part of the
        // cached prefix. Hash it with the static block so a change is
        // attributed to the harness rather than reported as a plain low read.
        let system_hash = if dynamic_trimmed.is_empty() {
            stable_hash_str(system_static)
        } else {
            stable_hash_str(&format!("{system_static}\n\n{dynamic_trimmed}"))
        };
        KvCacheRequestSignature {
            system_static_hash: system_hash,
            tools_hash: stable_hash_json(tools),
            messages_hash: stable_hash_json(&cache_relevant_messages(messages)),
            message_hashes: message_hashes(messages),
            message_count: messages.len(),
            tool_count: tools.len(),
            system_static_chars: system_static.chars().count(),
            tools_json_chars: stable_json_len(tools),
            messages_json_chars: stable_json_len(messages),
            ephemeral_hash: if dynamic_trimmed.is_empty() {
                None
            } else {
                Some(stable_hash_str(dynamic_trimmed))
            },
            ephemeral_chars: dynamic_trimmed.chars().count(),
            ephemeral_message_count: usize::from(!dynamic_trimmed.is_empty()),
        }
    }

    pub(super) fn kv_cache_signatures_prefix_match(
        current: &KvCacheRequestSignature,
        previous: &KvCacheRequestSignature,
    ) -> bool {
        if previous.message_count > current.message_count {
            return false;
        }
        if !previous.message_hashes.is_empty() && !current.message_hashes.is_empty() {
            return current.message_hashes.len() >= previous.message_hashes.len()
                && current.message_hashes[..previous.message_hashes.len()]
                    == previous.message_hashes;
        }
        if previous.message_count == current.message_count {
            current.messages_hash == previous.messages_hash
        } else {
            false
        }
    }

    pub(super) fn kv_cache_common_prefix_messages(
        current: &KvCacheRequestSignature,
        previous: &KvCacheRequestSignature,
    ) -> usize {
        current
            .message_hashes
            .iter()
            .zip(previous.message_hashes.iter())
            .take_while(|(current, previous)| current == previous)
            .count()
    }
}
