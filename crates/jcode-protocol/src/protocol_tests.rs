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
include!("protocol_tests/session_accounts.rs");
#[test]
fn split_without_feedback_role_preserves_legacy_wire_shape() {
    let request = crate::decode_request(r#"{"type":"split","id":17}"#).unwrap();
    assert!(matches!(
        &request,
        crate::Request::Split {
            id: 17,
            feedback_role: None
        }
    ));
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        serde_json::json!({"type": "split", "id": 17})
    );
}

#[test]
fn split_feedback_role_round_trips() {
    let request =
        crate::decode_request(r#"{"type":"split","id":18,"feedback_role":"judge"}"#).unwrap();
    assert!(
        matches!(&request, crate::Request::Split { id: 18, feedback_role: Some(role) } if role == "judge")
    );
    assert_eq!(request.id(), 18);
}
