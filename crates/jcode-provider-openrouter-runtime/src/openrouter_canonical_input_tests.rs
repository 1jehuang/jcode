use super::*;
use serde_json::json;

#[test]
fn borrowed_canonical_parts_match_owned_fingerprints_and_nulls() {
    for request in [
        json!({}),
        json!({"messages": null, "tools": null}),
        json!({"messages": {}, "tools": {"unexpected": true}}),
        json!({"messages": [], "tools": []}),
        json!({"input": [{"role": "user", "content": "responses"}], "tools": []}),
        json!({"messages": [{"role": "system", "content": "雪"}, {"role": "user", "content": "hi"}], "tools": [{"type": "function"}]}),
        json!({"messages": [{"role": "user"}, {"role": "system"}]}),
    ] {
        let before = serde_json::to_vec(&request).unwrap();
        let owned_items = request
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let owned_system = owned_items
            .first()
            .filter(|item| item.get("role").and_then(Value::as_str) == Some("system"))
            .cloned();
        let owned_tools = request.get("tools").cloned();
        let (items, system, tools, count) = canonical_input_parts(&request);
        assert_eq!(items, owned_items);
        assert_eq!(system, owned_system.as_ref());
        assert_eq!(tools, owned_tools.as_ref());
        assert_eq!(
            count,
            owned_tools
                .as_ref()
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0)
        );
        assert_eq!(
            jcode_provider_core::fingerprint::stable_hash_json(items),
            jcode_provider_core::fingerprint::stable_hash_json(&owned_items)
        );
        assert_eq!(before, serde_json::to_vec(&request).unwrap());
        if let Some(tool_value) = tools {
            assert!(std::ptr::eq(tool_value, &request["tools"]));
        }
        if let Some(first) = items.first() {
            assert!(std::ptr::eq(first, &request["messages"][0]));
        }
    }
}
