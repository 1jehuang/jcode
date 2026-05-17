//! Unit tests for error module

use crate::error::CarpAiError;

#[test]
fn test_error_is_recoverable_connection() {
    let error = CarpAiError::Connection {
        message: "Connection refused".to_string(),
        endpoint: "http://localhost:8080".to_string(),
        source: None,
    };

    // Connection errors are typically recoverable
    assert!(error.is_recoverable());
}

#[test]
fn test_error_is_recoverable_timeout() {
    let error = CarpAiError::Timeout {
        operation: "completion".to_string(),
        timeout_secs: 30,
    };

    // Timeouts are recoverable
    assert!(error.is_recoverable());
}

#[test]
fn test_error_is_recoverable_rate_limit() {
    let error = CarpAiError::RateLimit {
        retry_after_secs: Some(60),
        limit: 100,
    };

    // Rate limits are recoverable (with wait)
    assert!(error.is_recoverable());
}

#[test]
fn test_error_is_not_recoverable_auth() {
    let error = CarpAiError::Authentication {
        message: "Invalid API key".to_string(),
        provider: "openai".to_string(),
    };

    // Auth errors are NOT recoverable without user intervention
    assert!(!error.is_recoverable());
}

#[test]
fn test_error_is_not_recoverable_validation() {
    let error = CarpAiError::Validation {
        message: "Invalid input".to_string(),
        field: Some("prompt".to_string()),
        suggestion: Some("Check input format".to_string()),
    };

    // Validation errors are NOT recoverable (need fix)
    assert!(!error.is_recoverable());
}

#[test]
fn test_error_is_not_recoverable_server_4xx() {
    let error = CarpAiError::Server {
        status: 400,
        message: "Bad request".to_string(),
        code: None,
        request_id: None,
    };

    // 4xx client errors are NOT recoverable
    assert!(!error.is_recoverable());
}

#[test]
fn test_error_is_recoverable_server_5xx() {
    let error = CarpAiError::Server {
        status: 503,
        message: "Service unavailable".to_string(),
        code: None,
        request_id: None,
    };

    // 5xx server errors ARE recoverable (retry might work)
    assert!(error.is_recoverable());
}

#[test]
fn test_error_is_recoverable_offline_queued() {
    let error = CarpAiError::Offline {
        message: "No cached response".to_string(),
        queued: true,
        suggestion: Some("Will process when online".to_string()),
    };

    // Offline with queue is recoverable
    assert!(error.is_recoverable());
}

#[test]
fn test_error_is_not_recoverable_offline_not_queued() {
    let error = CarpAiError::Offline {
        message: "No cached response".to_string(),
        queued: false,
        suggestion: None,
    };

    // Offline without queue is NOT recoverable
    assert!(!error.is_recoverable());
}

#[test]
fn test_error_recovery_suggestion_connection() {
    let error = CarpAiError::Connection {
        message: "Connection refused".to_string(),
        endpoint: "http://localhost:8080".to_string(),
        source: None,
    };

    let suggestion = error.recovery_suggestion();
    assert!(suggestion.is_some());
    let s = suggestion.unwrap();
    assert!(s.contains("check") || s.contains("network") || s.contains("server"));
}

#[test]
fn test_error_recovery_suggestion_rate_limit() {
    let error = CarpAiError::RateLimit {
        retry_after_secs: Some(60),
        limit: 100,
    };

    let suggestion = error.recovery_suggestion();
    assert!(suggestion.is_some());
    let s = suggestion.unwrap();
    assert!(s.contains("wait") || s.contains("60"));
}

#[test]
fn test_error_recovery_suggestion_validation() {
    let error = CarpAiError::Validation {
        message: "Prompt too long".to_string(),
        field: Some("prompt".to_string()),
        suggestion: Some("Shorten your prompt".to_string()),
    };

    let suggestion = error.recovery_suggestion();
    assert_eq!(suggestion, Some("Shorten your prompt".to_string()));
}

#[test]
fn test_error_display_connection() {
    let error = CarpAiError::Connection {
        message: "Connection refused".to_string(),
        endpoint: "http://localhost:8080".to_string(),
        source: None,
    };

    let display = format!("{}", error);
    assert!(display.contains("Connection"));
    assert!(display.contains("localhost:8080"));
}

#[test]
fn test_error_display_timeout() {
    let error = CarpAiError::Timeout {
        operation: "completion".to_string(),
        timeout_secs: 30,
    };

    let display = format!("{}", error);
    assert!(display.contains("Timeout"));
    assert!(display.contains("30"));
}

#[test]
fn test_error_display_rate_limit() {
    let error = CarpAiError::RateLimit {
        retry_after_secs: Some(60),
        limit: 100,
    };

    let display = format!("{}", error);
    assert!(display.contains("Rate limit"));
    assert!(display.contains("100"));
}

#[test]
fn test_error_from_reqwest() {
    // Test that we can convert reqwest errors
    let error = CarpAiError::from_reqwest_error(
        &reqwest::Error::new(reqwest::StatusCode::BAD_GATEWAY, http::Error::from(std::io::Error::new(std::io::ErrorKind::Other, "test")))
    );

    match error {
        CarpAiError::Connection { .. } => {},
        _ => panic!("Expected Connection error"),
    }
}

#[test]
fn test_error_debug_format() {
    let error = CarpAiError::Timeout {
        operation: "test".to_string(),
        timeout_secs: 10,
    };

    let debug = format!("{:?}", error);
    assert!(debug.contains("Timeout"));
}

#[test]
fn test_error_clone() {
    let error = CarpAiError::Timeout {
        operation: "test".to_string(),
        timeout_secs: 10,
    };

    let cloned = error.clone();
    match cloned {
        CarpAiError::Timeout { operation, timeout_secs } => {
            assert_eq!(operation, "test");
            assert_eq!(timeout_secs, 10);
        },
        _ => panic!("Wrong error type"),
    }
}

#[test]
fn test_error_serde_roundtrip() {
    use serde_json;

    let error = CarpAiError::Timeout {
        operation: "completion".to_string(),
        timeout_secs: 30,
    };

    // Serialize
    let json = serde_json::to_string(&error).unwrap();

    // Deserialize
    let deserialized: CarpAiError = serde_json::from_str(&json).unwrap();

    match deserialized {
        CarpAiError::Timeout { operation, timeout_secs } => {
            assert_eq!(operation, "completion");
            assert_eq!(timeout_secs, 30);
        },
        _ => panic!("Wrong error type after deserialization"),
    }
}

#[test]
fn test_error_source_chain() {
    let io_error = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
    let error = CarpAiError::Connection {
        message: "Failed to connect".to_string(),
        endpoint: "http://localhost".to_string(),
        source: Some(Box::new(io_error)),
    };

    // Should have source
    assert!(error.source().is_some());
}
