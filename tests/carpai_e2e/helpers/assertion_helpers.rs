//! Assertion helpers for E2E tests
//!
//! Provides reusable assertion macros and functions for validating
//! test outcomes, HTTP responses, JSON payloads, and timing constraints.

use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

/// Assert that output contains expected text
///
/// # Arguments
/// * `output` - The actual output string
/// * `expected` - Substring that should be present in output
pub fn assert_output_contains(output: &str, expected: &str) {
    assert!(
        output.contains(expected),
        "Expected output to contain '{}'\n\nActual output:\n{}",
        expected,
        truncate_output(output, 500)
    );
}

/// Assert that output does NOT contain text
pub fn assert_output_not_contains(output: &str, unexpected: &str) {
    assert!(
        !output.contains(unexpected),
        "Expected output NOT to contain '{}', but it did\n\nOutput:\n{}",
        unexpected,
        truncate_output(output, 500)
    );
}

/// Assert HTTP response status code matches expected value
///
/// # Arguments
/// * `response` - HTTP response (reqwest::Response)
/// * `expected` - Expected status code (e.g., 200, 404)
#[cfg(feature = "server")]
pub fn assert_status_code(response: &reqwest::Response, expected: u16) {
    let actual = response.status().as_u16();
    assert_eq!(
        actual, expected,
        "Expected status code {}, got {}\nResponse: {:?}",
        actual, expected, response
    );
}

/// Assert response time is within acceptable bounds
///
/// # Arguments
/// * `duration` - Actual response duration
/// * `max_ms` - Maximum acceptable duration in milliseconds
pub fn assert_response_time(duration: Duration, max_ms: u64) {
    let actual_ms = duration.as_millis() as u64;
    assert!(
        actual_ms <= max_ms,
        "Response time {}ms exceeded maximum allowed {}ms",
        actual_ms,
        max_ms
    );
}

/// Assert JSON field exists and has expected value
///
/// # Arguments
/// * `json` - Parsed JSON value
/// * `field_path` - Dot-separated path to field (e.g., "data.messages.0.content")
/// * `expected` - Expected value
pub fn assert_json_field(json: &Value, field_path: &str, expected: &Value) -> Result<()> {
    let actual = get_json_field(json, field_path)?;

    assert_eq!(
        *actual, *expected,
        "JSON field '{}' mismatch:\nExpected: {}\nActual:   {}",
        field_path,
        serde_json::to_string_pretty(expected)?,
        serde_json::to_string_pretty(actual)?
    );

    Ok(())
}

/// Assert JSON field exists (regardless of value)
pub fn assert_json_field_exists(json: &Value, field_path: &str) -> Result<()> {
    match get_json_field(json, field_path) {
        Ok(_) => Ok(()),
        Err(e) => anyhow::bail!("JSON field '{}' should exist but doesn't: {}", field_path, e),
    }
}

/// Assert JSON array has expected length
pub fn assert_json_array_length(json: &Value, field_path: &str, expected_len: usize) -> Result<()> {
    let arr = get_json_field(json, field_path)?;

    if let Value::Array(arr) = arr {
        assert_eq!(
            arr.len(),
            expected_len,
            "Array at '{}' has length {}, expected {}",
            field_path,
            arr.len(),
            expected_len
        );
        Ok(())
    } else {
        anyhow::bail!("Field '{}' is not an array", field_path)
    }
}

/// Get a nested JSON field by dot-notation path
fn get_json_field<'a>(json: &'a Value, path: &str) -> Result<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        // Handle array indexing (e.g., "messages.0")
        if let Ok(index) = part.parse::<usize>() {
            if let Value::Array(arr) = current {
                current = arr.get(index).with_context(|| {
                    format!(
                        "Index {} out of bounds at path segment '{}' (array length: {})",
                        index,
                        part,
                        arr.len()
                    )
                })?;
            } else {
                anyhow::bail!(
                    "Cannot index non-array value at path segment {} (full path: {}.{} )",
                    i,
                    parts[..i].join("."),
                    part
                );
            }
        } else {
            // Regular object field access
            current = current.get(part).with_context(|| {
                format!(
                    "Field '{}' not found at path segment {} (full path: {}.{} )",
                    part,
                    i,
                    parts[..i].join("."),
                    part
                )
            })?;
        }
    }

    Ok(current)
}

/// Assert string matches a regex pattern
#[cfg(feature = "regex")]
pub fn assert_matches_regex(text: &str, pattern: &str) -> Result<()> {
    use regex::Regex;

    let re = Regex::new(pattern)
        .with_context(|| format!("Invalid regex pattern: {}", pattern))?;

    assert!(
        re.is_match(text),
        "Text did not match pattern '{}'\nText:\n{}",
        pattern,
        truncate_output(text, 500)
    );

    Ok(())
}

/// Assert two values are approximately equal (for floating point comparisons)
pub fn assert_approx_equal(actual: f64, expected: f64, tolerance: f64) {
    let diff = (actual - expected).abs();
    assert!(
        diff <= tolerance,
        "Values differ by {:.6}, which exceeds tolerance {:.6}\nExpected: {:.6}\nActual:   {:.6}",
        diff,
        tolerance,
        expected,
        actual
    );
}

/// Truncate long output for display purposes
fn truncate_output(output: &str, max_chars: usize) -> String {
    if output.len() <= max_chars {
        output.to_string()
    } else {
        format!("{}... [truncated, total {} chars]",
               &output[..max_chars],
               output.len())
    }
}

/// Custom test result type with detailed context
#[derive(Debug)]
pub struct TestAssertion {
    pub passed: bool,
    pub message: String,
    pub details: Option<String>,
}

impl TestAssertion {
    pub fn passed(message: impl Into<String>) -> Self {
        Self {
            passed: true,
            message: message.into(),
            details: None,
        }
    }

    pub fn failed(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            passed: false,
            message: message.into(),
            details: Some(details.into()),
        }
    }
}

/// Collection of assertions for reporting
pub struct AssertionReport {
    pub test_name: String,
    pub assertions: Vec<TestAssertion>,
    start_time: std::time::Instant,
}

impl AssertionReport {
    pub fn new(test_name: impl Into<String>) -> Self {
        Self {
            test_name: test_name.into(),
            assertions: Vec::new(),
            start_time: std::time::Instant::now(),
        }
    }

    pub fn add(&mut self, assertion: TestAssertion) {
        self.assertions.push(assertion);
    }

    /// Check all assertions passed; panic with details if any failed
    pub fn finalize(self) {
        let failed: Vec<&TestAssertion> = self.assertions.iter()
            .filter(|a| !a.passed)
            .collect();

        let elapsed = self.start_time.elapsed();
        let total = self.assertions.len();
        let passed = total - failed.len();

        tracing::info!(
            "Test '{}' completed in {:?}: {}/{} assertions passed",
            self.test_name,
            elapsed,
            passed,
            total
        );

        if !failed.is_empty() {
            let failure_details: Vec<String> = failed.iter()
                .map(|f| format!("- {}: {}", f.message, f.details.as_deref().unwrap_or("")))
                .collect();

            panic!(
                "Test '{}' FAILED ({}/{} assertions passed):\n{}\n\nCompleted in {:?}",
                self.test_name,
                passed,
                total,
                failure_details.join("\n"),
                elapsed
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_output_contains_passes_on_match() {
        assert_output_contains("hello world", "world");
    }

    #[test]
    #[should_panic(expected = "Expected output to contain")]
    fn test_assert_output_contains_fails_on_no_match() {
        assert_output_contains("hello world", "foo");
    }

    #[test]
    fn test_get_json_field_simple_path() {
        let json = serde_json::json!({"name": "test", "value": 42});
        let result = get_json_field(&json, "name").unwrap();
        assert_eq!(result, &serde_json::json!("test"));
    }

    #[test]
    fn test_get_json_field_nested_path() {
        let json = serde_json::json!({
            "data": {"messages": [{"content": "hello"}]}
        });
        let result = get_json_field(&json, "data.messages.0.content").unwrap();
        assert_eq!(result, &serde_json::json!("hello"));
    }

    #[test]
    fn test_assert_response_time_within_bounds() {
        assert_response_time(Duration::from_millis(100), 200);
    }

    #[test]
    #[should_panic(expected = "exceeded maximum")]
    fn test_assert_response_time_exceeds_bounds() {
        assert_response_time(Duration::from_millis(300), 200);
    }

    #[test]
    fn test_assertion_report_collects_and_finalizes() {
        let mut report = AssertionReport::new("test_report");
        report.add(TestAssertion::passed("assertion 1"));
        report.add(TestAssertion::passed("assertion 2"));

        // Should not panic
        report.finalize();
    }

    #[test]
    #[should_panic(expected = "FAILED")]
    fn test_assertion_report_detects_failures() {
        let mut report = AssertionReport::new("test_report_failed");
        report.add(TestAssertion::passed("good assertion"));
        report.add(TestAssertion::failed("bad assertion", "details here"));

        report.finalize();  // Should panic
    }
}
