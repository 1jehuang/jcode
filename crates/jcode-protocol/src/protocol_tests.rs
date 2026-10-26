use super::*;
use anyhow::{Result, anyhow};

fn parse_request_json(json: &str) -> Result<Request> {
    serde_json::from_str(json).map_err(Into::into)
}

fn parse_event_json(json: &str) -> Result<ServerEvent> {
    serde_json::from_str(json).map_err(Into::into)
}

include!("protocol_tests/core_events.rs");
include!("protocol_tests/comm_requests.rs");
include!("protocol_tests/comm_responses.rs");
include!("protocol_tests/comm_format_awaited.rs");
include!("protocol_tests/misc_events.rs");
include!("protocol_tests/randomized.rs");
include!("protocol_tests/model_usage.rs");
#[test]
fn swarm_runtime_wait_metadata_is_optional_and_nonempty() {
    let empty: super::SwarmMemberRuntime = serde_json::from_str("{}").unwrap();
    assert!(empty.is_empty());
    assert_eq!(serde_json::to_value(&empty).unwrap(), serde_json::json!({}));
    let waiting = super::SwarmMemberRuntime {
        wait_reason: Some("Waiting for background task".into()),
        wait_age_secs: Some(0),
        ..Default::default()
    };
    assert!(!waiting.is_empty());
    let encoded = serde_json::to_value(&waiting).unwrap();
    assert_eq!(encoded["wait_age_secs"], 0);
    assert_eq!(
        serde_json::from_value::<super::SwarmMemberRuntime>(encoded).unwrap(),
        waiting
    );
    assert!(
        !super::SwarmMemberRuntime {
            wait_age_secs: Some(1),
            ..Default::default()
        }
        .is_empty()
    );
}
