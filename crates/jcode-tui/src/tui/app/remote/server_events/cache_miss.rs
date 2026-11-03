use super::*;

pub(super) fn handle_kv_cache_miss(
    app: &mut App,
    reason: &str,
    missed_tokens: u64,
    message: String,
    notices_enabled: bool,
) -> bool {
    // TokenUsage normally records the local classification first. Do not
    // duplicate that notice, but do not require a baseline after reconnect.
    if let Some(turn_number) = app.kv_cache.kv_cache_turn_number {
        let call_index = app.kv_cache.kv_cache_turn_call_index;
        if let Some(sample) = app
            .kv_cache
            .kv_cache_miss_samples
            .iter_mut()
            .find(|sample| sample.turn_number == turn_number && sample.call_index == call_index)
        {
            // The daemon can have complete request fingerprints after a
            // reconnect while the client only has provisional usage telemetry.
            // Upgrade evidence in place without losing local turn/hit counters.
            if notices_enabled
                && matches!(reason, "low_read" | "zero_read")
                && matches!(
                    sample.reason,
                    KvCacheMissReason::LowRead | KvCacheMissReason::ZeroRead
                )
                && message.contains("] [prefix unchanged]")
            {
                let marker = format!("turn {turn_number} call {call_index}:");
                if let Some(idx) = app.display_messages.iter().rposition(|entry| {
                    entry.role == "system"
                        && entry.content.contains("KV cache miss [")
                        && entry.content.contains(&marker)
                        && !entry.content.contains("] [prefix unchanged]")
                }) {
                    let tagged = app.display_messages[idx].content.replacen(
                        "] ",
                        "] [prefix unchanged] ",
                        1,
                    );
                    return app.replace_display_message_content(idx, tagged);
                }
            }
            // TokenUsage can arrive before the daemon's native-compaction
            // attribution. Correct that provisional local notice in place,
            // rather than deduplicating away the more authoritative cause.
            // Daily rollover is attributed by the daemon from its own request
            // clock, so it likewise corrects a provisional local low-read.
            let authoritative = match reason {
                "compaction" => Some(KvCacheMissReason::Compaction),
                "daily_rollover" => Some(KvCacheMissReason::DailyRollover),
                // The daemon knows the account actually used after a failover.
                "account_switch" => Some(KvCacheMissReason::AccountSwitch),
                // Never retain a provisional unchanged-prefix tag when the
                // daemon detected a real change in its request fingerprints.
                "system_changed" => Some(KvCacheMissReason::HarnessSystemChanged),
                "tools_changed" => Some(KvCacheMissReason::HarnessToolsChanged),
                "prefix_changed" => Some(KvCacheMissReason::HarnessPrefixChanged),
                _ => None,
            };
            if let Some(authoritative) = authoritative
                && sample.reason != authoritative
            {
                sample.reason = authoritative;
                sample.missed_tokens = missed_tokens;
                if notices_enabled {
                    let marker = format!("turn {turn_number} call {call_index}:");
                    if let Some(idx) = app.display_messages.iter().rposition(|entry| {
                        entry.role == "system"
                            && entry.content.contains("KV cache miss [")
                            && entry.content.contains(&marker)
                    }) {
                        return app.replace_display_message_content(idx, message);
                    }
                    app.push_kv_cache_notice(message);
                    return true;
                }
            }
            return false;
        }
        let reason = match reason {
            "compaction" => KvCacheMissReason::Compaction,
            "provider_switch" => KvCacheMissReason::ProviderSwitch,
            "account_switch" => KvCacheMissReason::AccountSwitch,
            "model_switch" => KvCacheMissReason::ModelSwitch,
            "upstream_switch" => KvCacheMissReason::UpstreamSwitch,
            "expired" => KvCacheMissReason::Expired,
            "daily_rollover" => KvCacheMissReason::DailyRollover,
            "system_changed" => KvCacheMissReason::HarnessSystemChanged,
            "tools_changed" => KvCacheMissReason::HarnessToolsChanged,
            "prefix_changed" => KvCacheMissReason::HarnessPrefixChanged,
            "zero_read" => KvCacheMissReason::ZeroRead,
            "low_read" => KvCacheMissReason::LowRead,
            _ => KvCacheMissReason::Unknown,
        };
        app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
            turn_number,
            call_index,
            missed_tokens,
            reason,
        });
        let overflow = app
            .kv_cache
            .kv_cache_miss_samples
            .len()
            .saturating_sub(App::KV_CACHE_MAX_MISS_SAMPLES);
        app.kv_cache.kv_cache_miss_samples.drain(..overflow);
    }
    if notices_enabled {
        // Use the normal transcript insertion path for chronological insertion.
        app.push_kv_cache_notice(message);
        return true;
    }
    false
}
