//! API Key Validator with Prefix Check
//!
//! Validates that API keys:
//! - Start with expected prefix (e.g., "carpai_")
//! - Meet minimum length requirements
//! - Contain only valid characters

use std::sync::Arc;

/// API Key validator configuration and validation logic
#[derive(Debug, Clone)]
pub struct ApiKeyValidator {
    /// Expected prefix (e.g., "carpai_")
    pub expected_prefix: Arc<String>,

    /// Minimum key length (excluding prefix)
    pub min_length: usize,

    /// Maximum key length (excluding prefix)
    pub max_length: usize,
}

impl ApiKeyValidator {
    /// Create a new validator
    ///
    /// # Arguments
    /// * `prefix` - Expected prefix (e.g., "carpai_")
    /// * `min_length` - Minimum key length after prefix
    /// * `max_length` - Maximum key length after prefix
    pub fn new(prefix: &str, min_length: usize, max_length: usize) -> Self {
        Self {
            expected_prefix: Arc::new(prefix.to_string()),
            min_length,
            max_length,
        }
    }

    /// Validate API key format
    ///
    /// # Validation Rules
    /// 1. Must start with expected prefix
    /// 2. Key part (after prefix) must be within length bounds
    /// 3. Key part must contain only alphanumeric, underscore, or hyphen
    ///
    /// # Examples
    /// ```
    /// let validator = ApiKeyValidator::new("carpai_", 32, 64);
    /// assert!(validator.validate("carpai_abc123def456ghi789jkl012mno345pq"));
    /// assert!(!validator.validate("other_abc123")); // Wrong prefix
    /// assert!(!validator.validate("carpai_short"));  // Too short
    /// ```
    pub fn validate(&self, api_key: &str) -> ValidationResult {
        // Check prefix
        if !api_key.starts_with(&*self.expected_prefix) {
            return ValidationResult::Invalid(ValidationError::WrongPrefix {
                expected: self.expected_prefix.to_string(),
                found: api_key[..std::cmp::min(api_key.len(), self.expected_prefix.len())].to_string(),
            });
        }

        // Extract key part after prefix
        let key_part = &api_key[self.expected_prefix.len()..];

        // Check minimum length
        if key_part.len() < self.min_length {
            return ValidationResult::Invalid(ValidationError::TooShort {
                min: self.min_length,
                actual: key_part.len(),
            });
        }

        // Check maximum length
        if key_part.len() > self.max_length {
            return ValidationResult::Invalid(ValidationError::TooLong {
                max: self.max_length,
                actual: key_part.len(),
            });
        }

        // Check valid characters (alphanumeric, underscore, hyphen)
        if !key_part.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return ValidationResult::Invalid(ValidationError::InvalidCharacters);
        }

        ValidationResult::Valid
    }

    /// Extract the key part (without prefix)
    pub fn extract_key(&self, api_key: &str) -> Option<&str> {
        if api_key.starts_with(&*self.expected_prefix) {
            Some(&api_key[self.expected_prefix.len()..])
        } else {
            None
        }
    }

    /// Mask API key for logging (show first 8 and last 4 chars)
    pub fn mask_key(&self, api_key: &str) -> String {
        if api_key.len() <= 12 {
            "****".to_string()
        } else {
            let prefix_len = std::cmp::min(8, api_key.len());
            let suffix_len = std::cmp::min(4, api_key.len() - prefix_len);
            let visible_start = &api_key[..prefix_len];
            let visible_end = &api_key[api_key.len() - suffix_len..];
            format!("{}****{}", visible_start, visible_end)
        }
    }
}

/// Validation result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Valid,
    Invalid(ValidationError),
}

/// Validation error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// API key has wrong prefix
    WrongPrefix {
        expected: String,
        found: String,
    },

    /// Key is too short
    TooShort {
        min: usize,
        actual: usize,
    },

    /// Key is too long
    TooLong {
        max: usize,
        actual: usize,
    },

    /// Key contains invalid characters
    InvalidCharacters,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::WrongPrefix { expected, found } => {
                write!(f, "Wrong prefix: expected '{}', found '{}'", expected, found)
            }
            ValidationError::TooShort { min, actual } => {
                write!(f, "Key too short: minimum {} chars, got {}", min, actual)
            }
            ValidationError::TooLong { max, actual } => {
                write!(f, "Key too long: maximum {} chars, got {}", max, actual)
            }
            ValidationError::InvalidCharacters => {
                write!(f, "Key contains invalid characters (only alphanumeric, '_', '-' allowed)")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_api_key() {
        let validator = ApiKeyValidator::new("carpai_", 32, 64);

        // Valid key
        assert_eq!(
            validator.validate("carpai_abc123def456ghi789jkl012mno345pq"),
            ValidationResult::Valid
        );

        // Valid key with underscores and hyphens
        assert_eq!(
            validator.validate("carpai_abc-123_def-456_ghi-789_jkl-012_mno"),
            ValidationResult::Valid
        );
    }

    #[test]
    fn test_wrong_prefix() {
        let validator = ApiKeyValidator::new("carpai_", 32, 64);

        let result = validator.validate("other_abc123def456ghi789jkl012mno345pq");
        assert!(matches!(result, ValidationResult::Invalid(ValidationError::WrongPrefix { .. })));
    }

    #[test]
    fn test_too_short() {
        let validator = ApiKeyValidator::new("carpai_", 32, 64);

        let result = validator.validate("carpai_short");
        assert!(matches!(result, ValidationResult::Invalid(ValidationError::TooShort { .. })));
    }

    #[test]
    fn test_invalid_characters() {
        let validator = ApiKeyValidator::new("carpai_", 32, 64);

        let result = validator.validate("carpai_abc@123!def456#ghi789$jkl012mno345");
        assert!(matches!(result, ValidationResult::Invalid(ValidationError::InvalidCharacters)));
    }

    #[test]
    fn test_mask_key() {
        let validator = ApiKeyValidator::new("carpai_", 32, 64);

        let masked = validator.mask_key("carpai_abc123def456ghi789jkl012mno345pq");
        assert!(masked.starts_with("carpai_a"));
        assert!(masked.ends_with("5pq"));
        assert!(masked.contains("****"));
    }

    #[test]
    fn test_extract_key() {
        let validator = ApiKeyValidator::new("carpai_", 32, 64);

        let key = validator.extract_key("carpai_abc123def456");
        assert_eq!(key, Some("abc123def456"));

        let invalid = validator.extract_key("other_abc123");
        assert_eq!(invalid, None);
    }
}
