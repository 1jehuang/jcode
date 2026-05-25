pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 60000,
            jitter_factor: 0.5,
        }
    }
}

pub const MAX_PARALLEL_TOOLS: usize = 5;
pub const ABORT_GRACE_PERIOD_MS: u64 = 5000;
pub const BACKOFF_INITIAL_MS: u64 = 1000;
pub const BACKOFF_JITTER_FACTOR: f64 = 0.5;
pub const BACKOFF_MAX_MS: u64 = 60000;
pub const MAX_RETRY_ATTEMPTS: u32 = 3;
pub const MAX_FALLBACK_DEPTH: u32 = 3;
