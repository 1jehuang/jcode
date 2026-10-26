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

#[test]
fn swarm_live_status_defaults_and_roundtrips() {
    let legacy: super::SwarmMemberRuntime =
        serde_json::from_str(r#"{"context_tokens":4200,"output_tokens":99}"#).unwrap();
    assert!(legacy.live_status.is_none());
    let partial: super::SwarmLiveStatus =
        serde_json::from_str(r#"{"phase":"streaming","future_field":true}"#).unwrap();
    assert_eq!(partial.input_tokens, None);
    assert_eq!(partial.connection_type, None);
    let runtime = super::SwarmMemberRuntime {
        live_status: Some(super::SwarmLiveStatus {
            phase: "streaming".into(),
            input_tokens: Some(0),
            output_tokens: Some(17),
            output_tps_milli: Some(12500),
            connection_type: Some("websocket".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!runtime.is_empty());
    assert_eq!(
        serde_json::from_value::<super::SwarmMemberRuntime>(
            serde_json::to_value(&runtime).unwrap()
        )
        .unwrap(),
        runtime
    );
}
