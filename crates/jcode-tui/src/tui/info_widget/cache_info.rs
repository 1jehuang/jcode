use super::CacheMissAttribution;

/// Session-level KV cache telemetry for providers that report cache usage.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CacheHitInfo {
    /// Sum of per-request full prompt sizes, never inferred from aggregate counters.
    pub prompt_tokens: Option<u64>,
    pub last_prompt_tokens: Option<u64>,
    /// Input tokens from completed API requests that included explicit cache telemetry.
    pub reported_input_tokens: u64,
    /// Tokens read from provider KV/prefix cache across this session.
    pub read_tokens: u64,
    /// Tokens written/created in provider cache across this session, when reported.
    pub creation_tokens: u64,
    /// Approximate reusable prefix tokens expected to be cache-readable.
    pub optimal_input_tokens: u64,
    /// Reads paired with the optimal denominator; None uses the session reads.
    pub optimal_read_tokens: Option<u64>,
    /// Input tokens from the latest completed request with cache telemetry.
    pub last_reported_input_tokens: Option<u64>,
    /// Cached input tokens read on the latest completed request with cache telemetry.
    pub last_read_tokens: Option<u64>,
    /// Tokens written/created in provider cache on the latest completed request.
    pub last_creation_tokens: Option<u64>,
    /// Approximate reusable prefix tokens expected on the latest completed request.
    pub last_optimal_input_tokens: Option<u64>,
    /// Recent attributed misses with estimated cacheable tokens not read.
    pub miss_attributions: Vec<CacheMissAttribution>,
}

impl CacheHitInfo {
    /// Fraction of the session's prompt tokens that were served from cache.
    pub fn hit_ratio(&self) -> Option<f32> {
        let denominator = self.prompt_tokens?;
        if denominator == 0 {
            None
        } else {
            Some((self.read_tokens as f32 / denominator as f32).clamp(0.0, 1.0))
        }
    }

    /// Fraction of previously cacheable input reused, using reads from the
    /// same accounting window as the optimal input.
    pub fn optimal_ratio(&self) -> Option<f32> {
        if self.optimal_input_tokens == 0 {
            None
        } else {
            let read = self.optimal_read_tokens.unwrap_or(self.read_tokens);
            Some((read as f32 / self.optimal_input_tokens as f32).clamp(0.0, 1.0))
        }
    }

    pub fn last_ratio(&self) -> Option<f32> {
        let denominator = self.last_prompt_tokens?;
        if denominator == 0 {
            None
        } else {
            Some((self.last_read_tokens? as f32 / denominator as f32).clamp(0.0, 1.0))
        }
    }

    pub fn last_optimal_ratio(&self) -> Option<f32> {
        let optimal = self.last_optimal_input_tokens?;
        if optimal == 0 {
            None
        } else {
            Some((self.last_read_tokens? as f32 / optimal as f32).clamp(0.0, 1.0))
        }
    }
}
