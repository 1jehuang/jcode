//! SQL Injection Prevention Helpers
//!
//! Provides:
//! - Parameterized query builder
//! - Query validation
//! - Safe escaping utilities

use std::collections::HashMap;

/// Parameterized query builder to prevent SQL injection
///
/// Instead of string concatenation:
/// ```rust,no_run
/// // DANGEROUS - DON'T DO THIS
/// let query = format!("SELECT * FROM users WHERE id = {}", user_id);
/// ```
///
/// Use parameterized queries:
/// ```rust
/// use carpai::security::ParameterizedQuery;
///
/// let mut query = ParameterizedQuery::new("SELECT * FROM users WHERE id = ?1 AND name = ?2");
/// query.bind(1, &user_id);
/// query.bind(2, &user_name);
/// let (sql, params) = query.build();
/// ```
#[derive(Debug, Clone)]
pub struct ParameterizedQuery {
    /// SQL template with placeholders (?1, ?2, etc.)
    template: String,

    /// Bound parameters
    params: HashMap<usize, ParamValue>,

    /// Parameter order (for positional binding)
    param_order: Vec<usize>,
}

/// Parameter value (type-safe)
#[derive(Debug, Clone)]
pub enum ParamValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    Boolean(bool),
}

impl ParameterizedQuery {
    /// Create a new parameterized query
    pub fn new(template: &str) -> Self {
        Self {
            template: template.to_string(),
            params: HashMap::new(),
            param_order: Vec::new(),
        }
    }

    /// Bind a parameter to a placeholder
    ///
    /// # Arguments
    /// * `index` - Placeholder index (1-based)
    /// * `value` - Value to bind (anything that implements Into<ParamValue>)
    ///
    /// # Example
    /// ```
    /// let mut query = ParameterizedQuery::new("SELECT * FROM users WHERE id = ?1");
    /// query.bind(1, 42);
    /// ```
    pub fn bind(&mut self, index: usize, value: impl Into<ParamValue>) -> &mut Self {
        if !self.param_order.contains(&index) {
            self.param_order.push(index);
        }
        self.params.insert(index, value.into());
        self
    }

    /// Build the query into SQL and parameters
    ///
    /// Returns the SQL template (unchanged) and ordered parameter values
    pub fn build(self) -> (String, Vec<ParamValue>) {
        let ordered_params: Vec<ParamValue> = self.param_order
            .iter()
            .filter_map(|idx| self.params.get(idx).cloned())
            .collect();

        (self.template, ordered_params)
    }

    /// Validate that all placeholders have bound parameters
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Extract all placeholder indices from template
        let mut required_indices = Vec::new();
        let mut pos = 0;
        let bytes = self.template.as_bytes();

        while pos < bytes.len() {
            if bytes[pos] == b'?' && pos + 1 < bytes.len() && bytes[pos + 1].is_ascii_digit() {
                let idx = (bytes[pos + 1] - b'0') as usize;
                required_indices.push(idx);
                pos += 2;
            } else {
                pos += 1;
            }
        }

        // Check all required params are bound
        for idx in &required_indices {
            if !self.params.contains_key(idx) {
                return Err(ValidationError::UnboundParameter {
                    index: *idx,
                    template: self.template.clone(),
                });
            }
        }

        Ok(())
    }

    /// Get SQL template (for debugging/logging)
    pub fn template(&self) -> &str {
        &self.template
    }
}

/// Convert common types to ParamValue
impl From<i32> for ParamValue {
    fn from(v: i32) -> Self {
        ParamValue::Integer(v as i64)
    }
}

impl From<i64> for ParamValue {
    fn from(v: i64) -> Self {
        ParamValue::Integer(v)
    }
}

impl From<f64> for ParamValue {
    fn from(v: f64) -> Self {
        ParamValue::Real(v)
    }
}

impl From<String> for ParamValue {
    fn from(v: String) -> Self {
        ParamValue::Text(v)
    }
}

impl From<&str> for ParamValue {
    fn from(v: &str) -> Self {
        ParamValue::Text(v.to_string())
    }
}

impl From<Vec<u8>> for ParamValue {
    fn from(v: Vec<u8>) -> Self {
        ParamValue::Blob(v)
    }
}

impl From<bool> for ParamValue {
    fn from(v: bool) -> Self {
        ParamValue::Boolean(v)
    }
}

impl From<Option<String>> for ParamValue {
    fn from(v: Option<String>) -> Self {
        match v {
            Some(s) => ParamValue::Text(s),
            None => ParamValue::Null,
        }
    }
}

/// Validation error
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Unbound parameter ?{index} in query: {template}")]
    UnboundParameter {
        index: usize,
        template: String,
    },
}

/// Escape utility for LIKE clauses (when parameterized queries can't be used)
pub fn escape_like_pattern(pattern: &str) -> String {
    pattern
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Validate identifier (table/column names) to prevent injection
pub fn validate_identifier(name: &str) -> Result<(), IdentifierError> {
    if name.is_empty() {
        return Err(IdentifierError::Empty);
    }

    if name.len() > 128 {
        return Err(IdentifierError::TooLong(name.len()));
    }

    // Only allow alphanumeric and underscore
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(IdentifierError::InvalidCharacters(name.to_string()));
    }

    // Must not start with digit
    if name.chars().next().unwrap().is_ascii_digit() {
        return Err(IdentifierError::StartsWithDigit(name.to_string()));
    }

    Ok(())
}

/// Identifier validation error
#[derive(Debug, thiserror::Error)]
pub enum IdentifierError {
    #[error("Identifier cannot be empty")]
    Empty,

    #[error("Identifier too long: {0} characters (max 128)")]
    TooLong(usize),

    #[error("Invalid characters in identifier: {0} (only alphanumeric and underscore allowed)")]
    InvalidCharacters(String),

    #[error("Identifier cannot start with digit: {0}")]
    StartsWithDigit(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameterized_query_basic() {
        let mut query = ParameterizedQuery::new("SELECT * FROM users WHERE id = ?1 AND name = ?2");
        query.bind(1, 42i64);
        query.bind(2, "Alice");

        let (sql, params) = query.build();
        assert_eq!(sql, "SELECT * FROM users WHERE id = ?1 AND name = ?2");
        assert_eq!(params.len(), 2);
        assert!(matches!(params[0], ParamValue::Integer(42)));
        assert!(matches!(params[1], ParamValue::Text(s) if s == "Alice"));
    }

    #[test]
    fn test_parameterized_query_validation_success() {
        let mut query = ParameterizedQuery::new("SELECT * FROM users WHERE id = ?1");
        query.bind(1, 123);
        assert!(query.validate().is_ok());
    }

    #[test]
    fn test_parameterized_query_validation_failure() {
        let query = ParameterizedQuery::new("SELECT * FROM users WHERE id = ?1 AND name = ?2");
        assert!(query.validate().is_err());
    }

    #[test]
    fn test_escape_like_pattern() {
        assert_eq!(escape_like_pattern("100%"), "100\\%");
        assert_eq!(escape_like_pattern("foo_bar"), "foo\\_bar");
        assert_eq!(escape_like_pattern("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_validate_identifier_valid() {
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("user_id").is_ok());
        assert!(validate_identifier("_private").is_ok());
    }

    #[test]
    fn test_validate_identifier_invalid() {
        assert!(validate_identifier("").is_err());
        assert!(validate_identifier("123abc").is_err());
        assert!(validate_identifier("user; DROP TABLE").is_err());
        assert!(validate_identifier("user name").is_err());
    }
}
