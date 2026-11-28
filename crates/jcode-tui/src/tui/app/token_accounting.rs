/// Live counters since the last authoritative history snapshot.
#[derive(Clone, Debug, Default)]
pub(super) struct TokenAccounting {
    // Total session token usage (accumulated across all turns)
    pub(super) total_input_tokens: u64,
    pub(super) total_output_tokens: u64,
    // Total session KV cache usage for turns where the provider reported cache telemetry.
    pub(super) total_cache_reported_input_tokens: u64,
    pub(super) total_cache_prompt_tokens: u64,
    pub(super) total_cache_read_tokens: u64,
    pub(super) total_cache_creation_tokens: u64,
    pub(super) total_cache_optimal_input_tokens: u64,
    /// Reads from calls that contributed an optimal input denominator.
    pub(super) total_cache_optimal_read_tokens: u64,
    pub(super) last_cache_reported_input_tokens: Option<u64>,
    pub(super) last_cache_prompt_tokens: Option<u64>,
    pub(super) last_cache_read_tokens: Option<u64>,
    pub(super) last_cache_creation_tokens: Option<u64>,
    pub(super) last_cache_optimal_input_tokens: Option<u64>,
    pub(super) cache_next_optimal_input_tokens: Option<u64>,
    pub(super) current_request_has_optimal: bool,
}

impl TokenAccounting {
    pub(super) fn reset_live_cache_window(&mut self) {
        self.total_cache_reported_input_tokens = 0;
        self.total_cache_prompt_tokens = 0;
        self.total_cache_read_tokens = 0;
        self.total_cache_creation_tokens = 0;
        self.total_cache_optimal_input_tokens = 0;
        self.total_cache_optimal_read_tokens = 0;
        self.cache_next_optimal_input_tokens = None;
        self.last_cache_optimal_input_tokens = None;
        self.current_request_has_optimal = false;
    }
}
