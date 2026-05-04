use serde::Deserialize;

const EXPIRY_SKEW_MS: i64 = 60_000;

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    #[serde(default)]
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: i64,
}

pub(crate) fn expires_at_from_now(expires_in_seconds: i64) -> i64 {
    chrono::Utc::now().timestamp_millis() + (expires_in_seconds * 1000)
}

pub(crate) fn token_is_expired(expires_at: i64) -> bool {
    let now_ms = chrono::Utc::now().timestamp_millis();
    expires_at <= now_ms + EXPIRY_SKEW_MS
}
