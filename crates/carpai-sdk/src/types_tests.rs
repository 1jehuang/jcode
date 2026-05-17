//! Unit tests for types module

use crate::error::CarpAiError;
use crate::types::CompletionRequest;

#[test]
fn test_validate_prompt_length() {
    // Valid prompt
    let request = CompletionRequest {
        prompt: "Short prompt".to_string(),
        session_id: None,
        model: None,
        max_tokens: None,
        temperature: None,
        stop_sequences: vec![],
        top_p: None,
        context: Default::default(),
    };
    assert!(request.validate().is_ok());

    // Prompt too long (>100K characters)
    let long_request = CompletionRequest {
        prompt: "x".repeat(100_001),
        ..request.clone()
    };
    let result = long_request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { message, field, .. }) = result {
        assert_eq!(field, Some("prompt".to_string()));
        assert!(message.contains("100,000"));
    } else {
        panic!("Expected Validation error");
    }
}

#[test]
fn test_validate_temperature_range() {
    // Valid temperature
    let mut request = CompletionRequest {
        prompt: "Test".to_string(),
        session_id: None,
        model: None,
        max_tokens: None,
        temperature: Some(0.7),
        stop_sequences: vec![],
        top_p: None,
        context: Default::default(),
    };
    assert!(request.validate().is_ok());

    // Temperature at boundary (valid)
    request.temperature = Some(0.0);
    assert!(request.validate().is_ok());

    request.temperature = Some(2.0);
    assert!(request.validate().is_ok());

    // Temperature out of range (invalid)
    request.temperature = Some(-0.1);
    let result = request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { message, field, .. }) = result {
        assert_eq!(field, Some("temperature".to_string()));
        assert!(message.contains("-0.1"));
    } else {
        panic!("Expected Validation error");
    }

    request.temperature = Some(2.1);
    let result = request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { message, field, .. }) = result {
        assert_eq!(field, Some("temperature".to_string()));
        assert!(message.contains("2.1"));
    } else {
        panic!("Expected Validation error");
    }
}

#[test]
fn test_validate_max_tokens() {
    let mut request = CompletionRequest {
        prompt: "Test".to_string(),
        session_id: None,
        model: None,
        max_tokens: Some(100),
        temperature: None,
        stop_sequences: vec![],
        top_p: None,
        context: Default::default(),
    };

    // Valid tokens
    assert!(request.validate().is_ok());

    // Zero tokens (invalid)
    request.max_tokens = Some(0);
    let result = request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { field, .. }) = result {
        assert_eq!(field, Some("max_tokens".to_string()));
    }

    // Too many tokens (invalid)
    request.max_tokens = Some(100_001);
    let result = request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { field, .. }) = result {
        assert_eq!(field, Some("max_tokens".to_string()));
    }

    // Boundary valid
    request.max_tokens = Some(1);
    assert!(request.validate().is_ok());

    request.max_tokens = Some(100_000);
    assert!(request.validate().is_ok());
}

#[test]
fn test_validate_top_p_range() {
    let mut request = CompletionRequest {
        prompt: "Test".to_string(),
        session_id: None,
        model: None,
        max_tokens: None,
        temperature: None,
        stop_sequences: vec![],
        top_p: Some(0.9),
        context: Default::default(),
    };

    // Valid top-p
    assert!(request.validate().is_ok());

    // Boundary valid
    request.top_p = Some(0.0);
    assert!(request.validate().is_ok());

    request.top_p = Some(1.0);
    assert!(request.validate().is_ok());

    // Out of range (invalid)
    request.top_p = Some(-0.1);
    let result = request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { field, .. }) = result {
        assert_eq!(field, Some("top_p".to_string()));
    }

    request.top_p = Some(1.1);
    let result = request.validate();
    assert!(result.is_err());
    if let Err(CarpAiError::Validation { field, .. }) = result {
        assert_eq!(field, Some("top_p".to_string()));
    }
}

#[test]
fn test_validate_none_values() {
    // All optional fields None should be valid
    let request = CompletionRequest {
        prompt: "Test".to_string(),
        session_id: None,
        model: None,
        max_tokens: None,
        temperature: None,
        stop_sequences: vec![],
        top_p: None,
        context: Default::default(),
    };
    assert!(request.validate().is_ok());
}

#[test]
fn test_validate_combined_errors() {
    // Multiple invalid parameters - should catch first one
    let request = CompletionRequest {
        prompt: "Test".to_string(),
        session_id: None,
        model: None,
        max_tokens: Some(0),
        temperature: Some(3.0),
        stop_sequences: vec![],
        top_p: Some(1.5),
        context: Default::default(),
    };
    let result = request.validate();
    assert!(result.is_err());
    // Should fail on one of the validations (order depends on implementation)
}

#[test]
fn test_request_id_generation() {
    use crate::types::RequestId;

    let id1 = RequestId::new();
    let id2 = RequestId::new();

    // Each ID should be unique
    assert_ne!(id1, id2);

    // Should not be empty
    assert!(!id1.0.is_empty());
    assert!(!id2.0.is_empty());
}

#[test]
fn test_session_id_generation() {
    use crate::types::SessionId;

    let id1 = SessionId::new();
    let id2 = SessionId::new();

    // Each ID should be unique
    assert_ne!(id1, id2);

    // Should not be empty
    assert!(!id1.0.is_empty());
    assert!(!id2.0.is_empty());
}

#[test]
fn test_completion_context() {
    use crate::types::CompletionContext;

    let context = CompletionContext {
        file_path: Some("src/main.rs".to_string()),
        language: Some("rust".to_string()),
        cursor_position: Some((10, 5)),
        surrounding_code: Some("fn main() {}".to_string()),
        project_root: Some("/home/user/project".to_string()),
        metadata: std::collections::HashMap::new(),
    };

    assert_eq!(context.file_path, Some("src/main.rs".to_string()));
    assert_eq!(context.language, Some("rust".to_string()));
    assert_eq!(context.cursor_position, Some((10, 5)));
}
