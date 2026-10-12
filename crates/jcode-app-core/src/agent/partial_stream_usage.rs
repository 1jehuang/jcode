/// Report whatever usage a provider stream reported before it failed.
///
/// The normal `usage_report` is emitted after the stream completes. A stream
/// that errors (or is retried after compaction) still consumed the tokens the
/// provider already reported, so count them. `[input, output, cache_read,
/// cache_creation]`; a no-op when nothing was reported.
pub(super) fn record_partial_stream_usage(
    session_id: &str,
    provider: &str,
    model: &str,
    [input, output, cache_read, cache_creation]: [Option<u64>; 4],
) {
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_creation.is_none() {
        return;
    }
    crate::telemetry::record_provider_usage(
        Some(session_id),
        provider,
        model,
        crate::telemetry::UsageSource::Agent,
        crate::telemetry::ProviderUsage {
            input_tokens: input.unwrap_or(0),
            output_tokens: output.unwrap_or(0),
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_creation,
        },
    );
}
