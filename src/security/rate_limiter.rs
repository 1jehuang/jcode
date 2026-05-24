//! Rate Limiting Middleware for Axum
//!
//! Uses tower_governor for token bucket rate limiting

use axum::{
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::Response,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_governor::{
    governor::GovernorConfigBuilder,
    key_extractor::SmartIpKeyExtractor,
    GovernorLayer, GovernorService,
};
use tracing::warn;

/// Rate limit configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Requests per second
    pub rps: u64,

    /// Burst size (max instantaneous requests)
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            rps: 10,      // 10 requests/second
            burst_size: 20, // Allow burst of 20
        }
    }
}

/// Create rate limiting layer for Axum
pub fn create_rate_limit_layer(config: RateLimitConfig) -> GovernorLayer {
    let conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(config.rps)
            .burst_size(config.burst_size)
            .finish()
            .expect("Failed to build rate limit config"),
    );

    GovernorLayer::new(conf)
}

/// Per-endpoint rate limiter with different limits
pub struct EndpointRateLimiter {
    /// General API endpoints
    pub general: GovernorLayer,

    /// Authentication endpoints (stricter)
    pub auth: GovernorLayer,

    /// Completion endpoints (moderate)
    pub completion: GovernorLayer,

    /// Chat endpoints (moderate)
    pub chat: GovernorLayer,
}

impl EndpointRateLimiter {
    pub fn new() -> Self {
        Self {
            general: create_rate_limit_layer(RateLimitConfig {
                rps: 10,
                burst_size: 20,
            }),
            auth: create_rate_limit_layer(RateLimitConfig {
                rps: 2,   // Stricter: 2 req/s
                burst_size: 5,
            }),
            completion: create_rate_limit_layer(RateLimitConfig {
                rps: 5,   // Moderate: 5 req/s
                burst_size: 10,
            }),
            chat: create_rate_limit_layer(RateLimitConfig {
                rps: 3,   // Moderate: 3 req/s
                burst_size: 8,
            }),
        }
    }
}

/// Custom rate limit error handler
pub fn handle_rate_limit_error(key: SmartIpKeyExtractor, addr: ConnectInfo<SocketAddr>) -> Response {
    warn!(
        "Rate limit exceeded for IP: {} (key: {:?})",
        addr.0, key
    );

    (
        StatusCode::TOO_MANY_REQUESTS,
        serde_json::json!({
            "error": {
                "code": 429,
                "message": "Rate limit exceeded. Please try again later.",
                "retry_after_secs": 60,
            }
        })
        .to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_config_defaults() {
        let config = RateLimitConfig::default();
        assert_eq!(config.rps, 10);
        assert_eq!(config.burst_size, 20);
    }

    #[test]
    fn test_endpoint_rate_limiter_creation() {
        let limiter = EndpointRateLimiter::new();
        // Just verify it can be created without panic
        assert!(true);
    }
}
