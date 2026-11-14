//! Variable substitution for session memory templates.

use std::collections::HashMap;

/// Substitutes `{{variable}}` placeholders in template text.
#[derive(Debug, Clone)]
pub struct VariableSubstitutor {
    /// Variable name -> value mapping.
    variables: HashMap<String, String>,
}

impl VariableSubstitutor {
    /// Create a new substitutor with the given variables.
    pub fn new(variables: HashMap<String, String>) -> Self {
        Self { variables }
    }

    /// Create an empty substitutor.
    pub fn empty() -> Self {
        Self {
            variables: HashMap::new(),
        }
    }

    /// Add a variable.
    pub fn with_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(name.into(), value.into());
        self
    }

    /// Get a variable value.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(String::as_str)
    }

    /// List all variable names.
    pub fn keys(&self) -> Vec<&str> {
        self.variables.keys().map(String::as_str).collect()
    }

    /// Substitute all `{{var}}` placeholders in `text`.
    ///
    /// Unrecognized variables are left as-is.
    pub fn substitute(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (name, value) in &self.variables {
            let placeholder = format!("{{{{{}}}}}", name);
            result = result.replace(&placeholder, value);
        }
        result
    }
}

impl Default for VariableSubstitutor {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_substitution() {
        let vars = VariableSubstitutor::new(HashMap::from([
            ("project".into(), "jcode".into()),
            ("feature".into(), "session-memory".into()),
        ]));
        let result = vars.substitute("Working on {{project}} {{feature}}.");
        assert_eq!(result, "Working on jcode session-memory.");
    }

    #[test]
    fn unknown_var_left_as_is() {
        let vars = VariableSubstitutor::empty();
        let result = vars.substitute("{{unknown}} stays.");
        assert_eq!(result, "{{unknown}} stays.");
    }

    #[test]
    fn builder_pattern() {
        let vars = VariableSubstitutor::empty()
            .with_var("a", "1")
            .with_var("b", "2");
        assert_eq!(vars.get("a"), Some("1"));
        assert_eq!(vars.get("b"), Some("2"));
        assert_eq!(vars.keys().len(), 2);
    }
}
