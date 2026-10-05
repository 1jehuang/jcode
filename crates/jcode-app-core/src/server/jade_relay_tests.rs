//! Tests for `#[path]`-attributed module `jade_relay_tests` of `jade_relay.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::*;

#[test]
fn relay_listener_config_is_opt_in_and_requires_credentials() {
    let cfg = SafetyConfig::default();
    assert!(RelayListenerConfig::from_safety(&cfg).is_none());

    let cfg = SafetyConfig {
        jade_relay_enabled: true,
        jade_relay_reply_enabled: true,
        ..SafetyConfig::default()
    };
    assert!(RelayListenerConfig::from_safety(&cfg).is_none());
}

#[test]
fn relay_listener_config_accepts_complete_opt_in_config() {
    let cfg = SafetyConfig {
        jade_relay_enabled: true,
        jade_relay_reply_enabled: true,
        jade_relay_api_base: Some("https://example.com/api".to_string()),
        jade_relay_token: Some("tok".to_string()),
        jade_relay_token_id: Some("alice-token".to_string()),
        jade_relay_user_id: Some("alice".to_string()),
        jade_relay_session_id: Some("sess-1".to_string()),
        ..SafetyConfig::default()
    };
    let parsed = RelayListenerConfig::from_safety(&cfg).expect("complete config");
    assert_eq!(parsed.api.api_base, "https://example.com/api/");
    assert_eq!(parsed.api.token, "tok");
    assert_eq!(parsed.api.token_id.as_deref(), Some("alice-token"));
    assert_eq!(parsed.api.user_id.as_deref(), Some("alice"));
    assert_eq!(parsed.session_id, "sess-1");
}

#[test]
fn relay_launch_config_is_separately_opt_in() {
    let cfg = SafetyConfig {
        jade_relay_enabled: true,
        jade_relay_api_base: Some("https://example.com".to_string()),
        jade_relay_token: Some("tok".to_string()),
        ..SafetyConfig::default()
    };
    assert!(RelayLaunchConfig::from_safety(&cfg).is_none());

    let cfg = SafetyConfig {
        jade_relay_launch_enabled: true,
        jade_relay_launch_working_dir: Some("/tmp/project".to_string()),
        ..cfg
    };
    let parsed = RelayLaunchConfig::from_safety(&cfg).expect("launch opt-in config");
    assert_eq!(parsed.api.api_base, "https://example.com/");
    assert_eq!(parsed.default_working_dir.as_deref(), Some("/tmp/project"));
}

#[test]
fn launch_request_reads_structured_data() {
    let event = RelayEvent {
        seq: 7,
        event_type: "launch".to_string(),
        text: Some("hello from web".to_string()),
        data: Some(serde_json::json!({
            "working_dir": "/tmp/repo",
            "model": "openai:gpt-test",
            "provider": "openai",
            "selfdev": true,
        })),
    };
    let parsed = LaunchRequest::from_event(&event, Some("/fallback")).expect("launch request");
    assert_eq!(parsed.text, "hello from web");
    assert_eq!(parsed.working_dir.as_deref(), Some("/tmp/repo"));
    assert_eq!(parsed.model.as_deref(), Some("openai:gpt-test"));
    assert_eq!(parsed.provider_key.as_deref(), Some("openai"));
    assert!(parsed.selfdev);
}

#[test]
fn relay_session_listener_polls_prompt_and_cancel_commands() {
    assert_eq!(session_command_event_types_param(), "types=prompt,cancel");
}

#[test]
fn relay_event_type_defaults_to_prompt_for_legacy_events() {
    let legacy = RelayEvent {
        seq: 1,
        event_type: String::new(),
        text: Some("hello".to_string()),
        data: None,
    };
    assert_eq!(legacy.event_type(), "prompt");

    let cancel = RelayEvent {
        event_type: "cancel".to_string(),
        ..legacy
    };
    assert_eq!(cancel.event_type(), "cancel");
}

#[test]
fn relay_url_encoding_matches_jade_api_expectations() {
    assert_eq!(urlencoding_encode("sess-relay-test"), "sess-relay-test");
    assert_eq!(urlencoding_encode("a/b c"), "a%2Fb%20c");
    assert_eq!(urlencoding_encode("user.name~1_2"), "user.name~1_2");
}

#[test]
fn truncation_preserves_short_text_and_marks_long_text() {
    assert_eq!(truncate_chars("hello", 10), "hello");
    assert_eq!(truncate_chars("abcdef", 4), "abc…");
}
