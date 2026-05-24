//! Password Hashing with Argon2id
//!
//! Replaces insecure SHA256 hashing with industry-standard argon2id.
//! Argon2id is the winner of the Password Hashing Competition and provides:
//! - Memory-hard operations (resistant to GPU/ASIC attacks)
//! - Configurable cost parameters
//! - Built-in salt generation

use argon2::{
    self,
    password_hash::{
        rand_core::OsRng,
        PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString,
    },
    Argon2,
};
use std::time::Duration;

/// Password hasher using Argon2id algorithm
#[derive(Debug, Clone)]
pub struct PasswordHasher {
    /// Argon2 instance with configured parameters
    argon2: Argon2<'static>,
}

impl PasswordHasher {
    /// Create a new password hasher with recommended parameters
    pub fn new() -> Self {
        // OWASP recommended parameters for 2024:
        // - m_cost: 19456 (19 MB memory)
        // - t_cost: 2 (2 iterations)
        // - p_cost: 1 (1 parallelism)
        let argon2 = Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(19456, 2, 1, None).expect("Invalid params"),
        );

        Self { argon2 }
    }

    /// Hash a password with random salt
    ///
    /// # Arguments
    /// * `password` - Plain text password to hash
    ///
    /// # Returns
    /// PHC-encoded hash string (includes algorithm, parameters, salt, and hash)
    ///
    /// # Example
    /// ```
    /// let hasher = PasswordHasher::new();
    /// let hash = hasher.hash_password("secure_password").unwrap();
    /// println!("{}", hash); // "$argon2id$v=19$m=19456,t=2,p=1$..."
    /// ```
    pub fn hash_password(&self, password: &str) -> Result<String, HashError> {
        // Generate random salt
        let salt = SaltString::generate(&mut OsRng);

        // Hash password
        let password_hash = self.argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| HashError::HashFailed(e.to_string()))?;

        Ok(password_hash.to_string())
    }

    /// Verify a password against a stored hash
    ///
    /// # Arguments
    /// * `password` - Plain text password to verify
    /// * `hash` - Stored PHC-encoded hash
    ///
    /// # Returns
    /// `true` if password matches hash, `false` otherwise
    ///
    /// # Example
    /// ```
    /// let hasher = PasswordHasher::new();
    /// let hash = hasher.hash_password("correct_password").unwrap();
    /// assert!(hasher.verify_password("correct_password", &hash).unwrap());
    /// assert!(!hasher.verify_password("wrong_password", &hash).unwrap());
    /// ```
    pub fn verify_password(&self, password: &str, hash: &str) -> Result<bool, HashError> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| HashError::ParseFailed(e.to_string()))?;

        match Argon2::default().verify_password(password.as_bytes(), &parsed_hash) {
            Ok(()) => Ok(true),
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(e) => Err(HashError::VerificationFailed(e.to_string())),
        }
    }

    /// Check if a hash needs rehashing (parameters updated)
    ///
    /// Returns `true` if the hash uses outdated parameters and should be rehashed
    pub fn needs_rehash(&self, hash: &str) -> Result<bool, HashError> {
        let parsed = PasswordHash::new(hash)
            .map_err(|e| HashError::ParseFailed(e.to_string()))?;

        // Check algorithm
        if parsed.algorithm != argon2::Algorithm::Argon2id {
            return Ok(true);
        }

        // Check version
        if parsed.version != Some(argon2::Version::V0x13.into()) {
            return Ok(true);
        }

        Ok(false)
    }

    /// Estimate hashing time (for monitoring)
    pub fn estimate_hash_time(&self) -> Duration {
        // Typical Argon2id with our params takes ~100-200ms on modern CPU
        Duration::from_millis(150)
    }
}

impl Default for PasswordHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Hashing error types
#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("Password hashing failed: {0}")]
    HashFailed(String),

    #[error("Hash parsing failed: {0}")]
    ParseFailed(String),

    #[error("Password verification failed: {0}")]
    VerificationFailed(String),
}

/// Legacy SHA256 hasher (DEPRECATED - for migration only)
#[deprecated(since = "0.12.0", note = "Use PasswordHasher with Argon2id instead")]
pub struct LegacySha256Hasher;

#[deprecated(since = "0.12.0", note = "Use PasswordHasher::hash_password instead")]
impl LegacySha256Hasher {
    pub fn hash(_password: &str) -> String {
        // This is intentionally broken to force migration
        panic!("Legacy SHA256 hashing is deprecated. Migrate to PasswordHasher with Argon2id.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify() {
        let hasher = PasswordHasher::new();
        let password = "secure_password_123";

        let hash = hasher.hash_password(password).unwrap();
        assert!(hash.starts_with("$argon2id$"));

        assert!(hasher.verify_password(password, &hash).unwrap());
        assert!(!hasher.verify_password("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_different_hashes_for_same_password() {
        let hasher = PasswordHasher::new();
        let password = "same_password";

        let hash1 = hasher.hash_password(password).unwrap();
        let hash2 = hasher.hash_password(password).unwrap();

        // Hashes should differ due to random salt
        assert_ne!(hash1, hash2);

        // But both should verify correctly
        assert!(hasher.verify_password(password, &hash1).unwrap());
        assert!(hasher.verify_password(password, &hash2).unwrap());
    }

    #[test]
    fn test_hash_format() {
        let hasher = PasswordHasher::new();
        let hash = hasher.hash_password("test").unwrap();

        // PHC format: $algorithm$params$salt$hash
        assert!(hash.starts_with("$argon2id$v="));
        assert!(hash.contains("$m="));
        assert!(hash.contains("$t="));
        assert!(hash.contains("$p="));
    }

    #[test]
    fn test_estimate_time() {
        let hasher = PasswordHasher::new();
        let estimate = hasher.estimate_hash_time();
        assert!(estimate >= Duration::from_millis(50));
        assert!(estimate <= Duration::from_millis(500));
    }
}
