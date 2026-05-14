use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UsageSource {
    User,
    Ambient,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub timestamp: DateTime<Utc>,
    pub source: UsageSource,
    pub tokens_input: u32,
    pub tokens_output: u32,
    pub provider: String,
}

impl UsageRecord {
    pub fn total_tokens(&self) -> u64 {
        self.tokens_input as u64 + self.tokens_output as u64
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    pub limit_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub limit_requests: Option<u64>,
    pub remaining_requests: Option<u64>,
    pub reset_at: Option<DateTime<Utc>>,
}
