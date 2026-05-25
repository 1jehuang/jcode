//! Security Middleware and Utilities
//!
//! Provides:
//! - Rate limiting middleware (Axum)
//! - API key validation with prefix check
//! - Password hashing with argon2id
//! - SQL injection prevention helpers

pub mod rate_limiter;
pub mod api_key_validator;
pub mod password_hasher;
pub mod sql_safety;

pub use rate_limiter::{GovernorLayer as RateLimitLayer, EndpointRateLimiter};
pub use api_key_validator::ApiKeyValidator;
pub use password_hasher::PasswordHasher;
pub use sql_safety::ParameterizedQuery;
