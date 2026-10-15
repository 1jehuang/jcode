use super::*;

#[test]
fn user_response_capability_is_explicit_and_independent_of_pdf_support() {
    for enabled in [false, true] {
        for req in ["create_session", "attach_session"] {
            let mut state = BridgeState {
                supports_user_response: enabled,
                ..Default::default()
            };
            let output = state.api_request_to_legacy(&json!({
                "req": req, "id": 1, "session_id": "response-session"
            }));
            let subscribe = output
                .iter()
                .find_map(|out| match out {
                    Outbound::Legacy(request) if request["type"] == "subscribe" => Some(request),
                    _ => None,
                })
                .expect("attachment sends Subscribe");
            assert_eq!(subscribe["supports_pdf_panels"], true);
            assert_eq!(subscribe["supports_user_response"], enabled);
        }
    }
    assert!(!BridgeState::default().supports_user_response);
}

#[test]
fn user_response_hello_defaults_off_for_older_clients() {
    for enabled in [false, true] {
        let mut value = json!({"req":"hello", "min_version":1, "max_version":1, "client":"test"});
        if enabled {
            value["supports_user_response"] = json!(true);
        }
        let request: jcode_harness_api::ApiRequest = serde_json::from_value(value).unwrap();
        let jcode_harness_api::ApiRequest::Hello {
            supports_user_response,
            ..
        } = request
        else {
            panic!("hello")
        };
        assert_eq!(supports_user_response, enabled);
    }
}
