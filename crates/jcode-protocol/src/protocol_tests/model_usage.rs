#[test]
fn model_usage_delta_requires_explicit_catalog_opt_in() {
    let old = serde_json::json!({"type":"get_model_catalog","id":7});
    let request: Request = serde_json::from_value(old.clone()).unwrap();
    assert!(matches!(
        request,
        Request::GetModelCatalog {
            subscribe_usage_updates: false,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(request).unwrap(), old);
    let new = serde_json::json!({"type":"get_model_catalog","id":8,"subscribe_usage_updates":true});
    let request: Request = serde_json::from_value(new.clone()).unwrap();
    assert!(matches!(
        request,
        Request::GetModelCatalog {
            subscribe_usage_updates: true,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(request).unwrap(), new);
}

#[test]
fn model_usage_route_metadata_defaults_absent_and_round_trips() {
    let old = serde_json::json!({"model":"m","provider":"OpenAI","api_method":"openai-oauth",
        "available":true,"detail":"ready"});
    let route: jcode_provider_core::ModelRoute = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(route.usage, None);
    // Serialized routes also carry their derived effort ladder.
    let efforts = jcode_provider_core::route_reasoning_efforts("openai-oauth", "m");
    let mut expected = old.clone();
    expected["reasoning_efforts"] = serde_json::json!(efforts);
    assert_eq!(serde_json::to_value(route).unwrap(), expected);
    let mut event = serde_json::json!({"type":"model_usage_updated","route":{
        "model":"m","provider":"OpenAI","api_method":"openai-oauth","available":true,"detail":"ready",
        "usage":{"count":3,"last_used_unix_secs":100,"tracking_started_unix_secs":10,
            "selection_count":5,"last_selected_unix_secs":8}}});
    let parsed: ServerEvent = serde_json::from_value(event.clone()).unwrap();
    event["route"]["reasoning_efforts"] = serde_json::json!(efforts);
    assert_eq!(serde_json::to_value(parsed).unwrap(), event);
}
