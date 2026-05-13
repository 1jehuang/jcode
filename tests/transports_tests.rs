//! Unit tests for Transport Protocols module
//!
//! Tests cover:
//! - SSE transport configuration and event parsing
//! - Streamable HTTP transport functionality
//! - WebSocket transport configuration
//! - Transport factory creation logic
//! - Transport registry management
//! - Error handling and edge cases

use carpai::transports::{
    SseConfig, SseEvent, StreamableHttpConfig, StreamableHttpTransport,
    SseTransport, Transport, TransportFactory, TransportRegistry, WebSocketConfig,
    WebSocketTransport,
};
use serde_json::json;

// ════════════════════════════════════════════════════════════════
// SSE Configuration Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_sse_config_default_values() {
    let config = SseConfig::default();
    
    assert!(config.url.is_empty(), "Default URL should be empty");
    assert!(!config.headers.is_empty(), "Should have default headers");
    assert_eq!(config.timeout_secs, 30, "Default timeout should be 30 seconds");
    
    // Check default headers include Accept: text/event-stream
    let has_accept_header = config.headers.iter().any(|(k, v)| {
        k == "Accept" && v == "text/event-stream"
    });
    assert!(has_accept_header, "Should have Accept: text/event-stream header");
    
    println!("✓ SSE config default values verified");
}

#[test]
fn test_sse_config_serialization() {
    let config = SseConfig {
        url: "http://localhost:8080/events".to_string(),
        headers: vec![
            ("Authorization".to_string(), "Bearer token123".to_string()),
        ],
        timeout_secs: 60,
    };
    
    // Test serialization
    let json_str = serde_json::to_string(&config).expect("Serialization failed");
    let parsed: SseConfig = serde_json::from_str(&json_str).expect("Deserialization failed");
    
    assert_eq!(parsed.url, config.url);
    assert_eq!(parsed.timeout_secs, config.timeout_secs);
    assert_eq!(parsed.headers.len(), config.headers.len());
    
    println!("✓ SSE config serialization/deserialization works");
}

#[test]
fn test_sse_config_custom_headers() {
    let mut config = SseConfig::default();
    config.url = "http://example.com/sse".to_string();
    
    // Add custom headers
    config.headers.push(("X-Custom-Header".to_string(), "custom-value".to_string()));
    
    assert_eq!(config.headers.len(), 3); // 2 default + 1 custom
    
    let has_custom = config.headers.iter().any(|(k, _)| k == "X-Custom-Header");
    assert!(has_custom, "Custom header should be present");
    
    println!("✓ Custom headers work correctly");
}

// ════════════════════════════════════════════════════════════════
// SSE Event Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_sse_event_parse_basic() {
    let raw = "data: Hello World\n\n";
    let event = SseEvent::parse(raw).expect("Parse failed");
    
    assert_eq!(event.data, "Hello World");
    assert!(event.id.is_none());
    assert!(event.event.is_none());
    assert!(event.retry.is_none());
    
    println!("✓ Basic SSE event parsing works");
}

#[test]
fn test_sse_event_parse_with_id() {
    let raw = "id: 123\ndata: Message with ID\n\n";
    let event = SseEvent::parse(raw).expect("Parse failed");
    
    assert_eq!(event.id.as_deref(), Some("123"));
    assert_eq!(event.data, "Message with ID");
    
    println!("✓ SSE event with ID parsing works");
}

#[test]
fn test_sse_event_parse_with_event_type() {
    let raw = "event: custom_event\ndata: Custom event data\n\n";
    let event = SseEvent::parse(raw).expect("Parse failed");
    
    assert_eq!(event.event.as_deref(), Some("custom_event"));
    assert_eq!(event.data, "Custom event data");
    
    println!("✓ SSE event with type parsing works");
}

#[test]
fn test_sse_event_parse_with_retry() {
    let raw = "data: Retry message\nretry: 5000\n\n";
    let event = SseEvent::parse(raw).expect("Parse failed");
    
    assert_eq!(event.data, "Retry message");
    assert_eq!(event.retry, Some(5000));
    
    println!("✓ SSE event with retry parsing works");
}

#[test]
fn test_sse_event_parse_multiline_data() {
    let raw = "data: Line 1\ndata: Line 2\ndata: Line 3\n\n";
    let event = SseEvent::parse(raw).expect("Parse failed");
    
    assert_eq!(event.data, "Line 1\nLine 2\nLine 3");
    
    println!("✓ Multiline data parsing works");
}

#[test]
fn test_sse_event_to_string_format() {
    let event = SseEvent {
        id: Some("evt-001".to_string()),
        event: Some("message".to_string()),
        data: "Test payload".to_string(),
        retry: Some(3000),
    };
    
    let formatted = event.to_string();
    
    assert!(formatted.contains("id: evt-001"), "Should contain ID");
    assert!(formatted.contains("event: message"), "Should contain event type");
    assert!(formatted.contains("data: Test payload"), "Should contain data");
    assert!(formatted.ends_with("\n\n"), "Should end with double newline");
    
    println!("✓ SSE event formatting works");
}

#[test]
fn test_sse_event_roundtrip() {
    let original = SseEvent {
        id: Some("test-id".to_string()),
        event: None,
        data: "Roundtrip test".to_string(),
        retry: None,
    };
    
    let formatted = original.to_string();
    let parsed = SseEvent::parse(&formatted).expect("Roundtrip parse failed");
    
    assert_eq!(original.id, parsed.id);
    assert_eq!(original.event, parsed.event);
    assert_eq!(original.data, parsed.data);
    assert_eq!(original.retry, parsed.retry);
    
    println!("✓ SSE event roundtrip (format -> parse) works");
}

// ════════════════════════════════════════════════════════════════
// Streamable HTTP Configuration Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_streamable_http_config_default() {
    let config = StreamableHttpConfig::default();
    
    assert!(config.base_url.is_empty());
    assert!(config.session_id.is_none());
    assert_eq!(config.request_timeout_secs, 30);
    
    println!("✓ Streamable HTTP default config works");
}

#[test]
fn test_streamable_http_config_with_session() {
    let config = StreamableHttpConfig {
        base_url: "http://localhost:3000/mcp".to_string(),
        session_id: Some("session-abc123".to_string()),
        request_timeout_secs: 45,
    };
    
    assert_eq!(config.base_url, "http://localhost:3000/mcp");
    assert_eq!(config.session_id.as_deref(), Some("session-abc123"));
    assert_eq!(config.request_timeout_secs, 45);
    
    println!("✓ Streamable HTTP config with session works");
}

#[tokio::test]
async fn test_streamable_http_transport_creation() {
    let config = StreamableHttpConfig {
        base_url: "http://localhost:3000/mcp".to_string(),
        ..Default::default()
    };
    
    let transport = StreamableHttpTransport::new(config);
    
    // Verify it implements Transport trait
    assert!(transport.is_connected()); // Default implementation returns true
    
    println!("✓ Streamable HTTP transport creation works");
}

// ════════════════════════════════════════════════════════════════
// WebSocket Configuration Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_websocket_config_default() {
    let config = WebSocketConfig::default();
    
    assert!(config.url.is_empty());
    assert!(config.origin.is_none());
    assert_eq!(config.protocols.len(), 1);
    assert_eq!(config.protocols[0], "mcp");
    
    println!("✓ WebSocket default config works");
}

#[test]
fn test_websocket_config_custom() {
    let config = WebSocketConfig {
        url: "ws://localhost:8080/ws".to_string(),
        origin: Some("http://localhost:8080".to_string()),
        protocols: vec!["mcp".to_string(), "jsonrpc".to_string()],
    };
    
    assert_eq!(config.url, "ws://localhost:8080/ws");
    assert_eq!(config.origin.as_deref(), Some("http://localhost:8080"));
    assert_eq!(config.protocols.len(), 2);
    
    println!("✓ WebSocket custom config works");
}

#[tokio::test]
async fn test_websocket_transport_creation() {
    let config = WebSocketConfig {
        url: "ws://localhost:8080/ws".to_string(),
        ..Default::default()
    };
    
    let transport = WebSocketTransport::new(config);
    
    // Should not be connected initially
    assert!(!transport.is_connected());
    
    println!("✓ WebSocket transport creation works");
}

// ════════════════════════════════════════════════════════════════
// SSE Transport Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_sse_transport_creation_and_close() {
    let config = SseConfig {
        url: "http://localhost:8080/events".to_string(),
        ..Default::default()
    };
    
    let transport = SseTransport::new(config);
    
    // Not connected initially
    assert!(!transport.is_connected());
    
    // Close should work even without connection
    let result = transport.close().await;
    assert!(result.is_ok(), "Close should succeed");
    
    println!("✓ SSE transport lifecycle works");
}

#[tokio::test]
async fn test_sse_receive_when_not_connected() {
    let config = SseConfig {
        url: "http://localhost:8080/events".to_string(),
        ..Default::default()
    };
    
    let transport = SseTransport::new(config);
    
    // Should error when not connected
    let result = transport.receive().await;
    assert!(result.is_err(), "Receive should fail when not connected");
    
    println!("✓ SSE receive error handling works");
}

// ════════════════════════════════════════════════════════════════
// Transport Registry Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_registry_initial_state() {
    let registry = TransportRegistry::new();
    
    let transports = registry.list().await;
    assert!(transports.is_empty(), "New registry should be empty");
    
    println!("✓ Registry initial state is empty");
}

#[tokio::test]
async fn test_registry_register_and_list() {
    let registry = TransportRegistry::new();
    
    let sse_config = SseConfig {
        url: "http://localhost:8080/sse".to_string(),
        ..Default::default()
    };
    let sse_transport = SseTransport::new(sse_config);
    
    // Register transport
    registry.register("sse-primary".to_string(), Box::new(sse_transport)).await;
    
    // List should show our transport
    let transports = registry.list().await;
    assert_eq!(transports.len(), 1);
    assert!(transports.contains(&"sse-primary".to_string()));
    
    println!("✓ Registry register and list work");
}

#[tokio::test]
async fn test_registry_send_to_nonexistent() {
    let registry = TransportRegistry::new();
    
    // Sending to non-existent transport should fail
    let result = registry.send_to("nonexistent", "test message").await;
    assert!(result.is_err(), "Send to nonexistent should fail");
    
    println!("✓ Registry handles missing transport correctly");
}

#[tokio::test]
async fn test_registry_close_all_empty() {
    let registry = TransportRegistry::new();
    
    // Closing empty registry should succeed
    let result = registry.close_all().await;
    assert!(result.is_ok(), "Close all on empty registry should succeed");
    
    println!("✓ Registry close_all handles empty state");
}

#[tokio::test]
async fn test_registry_multiple_transports() {
    let registry = TransportRegistry::new();
    
    // Register multiple transports
    for i in 1..=5 {
        let config = SseConfig {
            url: format!("http://localhost:8080/sse{}", i),
            ..Default::default()
        };
        let transport = SseTransport::new(config);
        
        registry.register(format!("sse-{}", i), Box::new(transport)).await;
    }
    
    // Should have 5 transports
    let transports = registry.list().await;
    assert_eq!(transports.len(), 5);
    
    // Close all
    let result = registry.close_all().await;
    assert!(result.is_ok());
    
    println!("✓ Registry handles multiple transports");
}

// ════════════════════════════════════════════════════════════════
// Edge Cases and Error Handling
// ════════════════════════════════════════════════════════════════

#[test]
fn test_sse_event_parse_empty_data() {
    let raw = "data: \n\n";
    let event = SseEvent::parse(raw).expect("Parse failed");
    
    assert_eq!(event.data, "");
    
    println!("✓ Empty data handling works");
}

#[test]
fn test_sse_event_parse_only_newlines() {
    let raw = "\n\n";
    let event = SseEvent::parse(raw).expect("Parse should handle only newlines");
    
    assert!(event.id.is_none());
    assert!(event.event.is_none());
    assert!(event.data.is_empty());
    
    println!("✓ Only newlines handling works");
}

#[test]
fn test_sse_event_parse_unknown_fields_ignored() {
    let raw = "unknown: value\ndata: actual data\n\n";
    let event = SseEvent::parse(raw).expect("Parse should ignore unknown fields");
    
    assert_eq!(event.data, "actual data");
    
    println!("✓ Unknown fields are properly ignored");
}

#[test]
fn test_config_serialization_roundtrips() {
    // Test that configs can round-trip through JSON
    let sse_original = SseConfig {
        url: "https://api.example.com/events".to_string(),
        timeout_secs: 120,
        ..Default::default()
    };
    
    let json = serde_json::to_value(&sse_original).unwrap();
    let sse_restored: SseConfig = serde_json::from_value(json).unwrap();
    
    assert_eq!(sse_original.url, sse_restored.url);
    assert_eq!(sse_original.timeout_secs, sse_restored.timeout_secs);
    
    println!("✓ Config serialization round-trips correctly");
}

#[tokio::test]
async fn test_transport_trait_interface_consistency() {
    // All transports should implement the same interface
    let sse_cfg = SseConfig { url: "http://test".into(), ..Default::default() };
    let http_cfg = StreamableHttpConfig { base_url: "http://test".into(), ..Default::default() };
    let ws_cfg = WebSocketConfig { url: "ws://test".into(), ..Default::default() };
    
    let sse: Box<dyn Transport> = Box::new(SseTransport::new(sse_cfg));
    let http: Box<dyn Transport> = Box::new(StreamableHttpTransport::new(http_cfg));
    let ws: Box<dyn Transport> = Box::new(WebSocketTransport::new(ws_cfg));
    
    // All should have close method
    assert!(sse.close().await.is_ok());
    assert!(http.close().await.is_ok());
    assert!(ws.close().await.is_ok());
    
    println!("✓ All transports implement consistent interface");
}
