use super::*;

#[test]
fn api_sessions_enable_server_account_routing_on_create_attach_and_reconnect() {
    let mut state = BridgeState::default();
    for request in [
        json!({"req": "create_session", "id": 1}),
        json!({"req": "attach_session", "id": 2, "session_id": "s1"}),
        json!({"req": "attach_session", "id": 3, "session_id": "s1"}),
    ] {
        let outbound = state.api_request_to_legacy(&request);
        let subscription = outbound
            .iter()
            .find_map(|outbound| match outbound {
                Outbound::Legacy(value) if value["type"] == "subscribe" => Some(value),
                _ => None,
            })
            .expect("every attachment must subscribe to the daemon");
        assert_eq!(
            subscription["supports_server_failover"], true,
            "API clients must route exhausted accounts rather than retry the same account: {request}"
        );
    }
}

#[test]
fn create_session_maps_to_subscribe() {
    let mut state = BridgeState::default();
    let out = state.api_request_to_legacy(&json!({"req": "create_session", "id": 1}));
    let Outbound::Legacy(value) = &out[0] else {
        panic!("expected legacy outbound");
    };
    assert_eq!(value["type"], "subscribe");
    assert!(value["working_dir"].is_string());
    assert!(value.get("system_prompt").is_none());
}
