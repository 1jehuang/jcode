use super::*;

// Frozen pre-borrow selection. Deliberately independent of the production key list.
fn original(request: &Value) -> Value {
    serde_json::json!({
        "model": request.get("model"),
        "instructions": request.get("instructions"),
        "input": request.get("input"),
        "tools": request.get("tools"),
        "tool_choice": request.get("tool_choice"),
        "parallel_tool_calls": request.get("parallel_tool_calls"),
        "reasoning": request.get("reasoning"),
        "context_management": request.get("context_management"),
        "include": request.get("include"),
        "service_tier": request.get("service_tier"),
        "prompt_cache_key": request.get("prompt_cache_key"),
        "prompt_cache_retention": request.get("prompt_cache_retention"),
    })
}

#[test]
fn canonical_selection_matches_frozen_bytes_hash_and_borrowing() {
    for request in [
        serde_json::json!({}),
        serde_json::json!({"input": [], "model": "future-model", "tools": null}),
        serde_json::json!({
            "context_management": [{"type":"compaction","threshold":100}],
            "include":["reasoning.encrypted_content"],
            "input":[{"role":"user","content":"日本語\n🦀"}],
            "instructions":"system", "model":"future-model",
            "parallel_tool_calls":false, "prompt_cache_key":"cache",
            "prompt_cache_retention":"24h", "reasoning":{"effort":"high"},
            "service_tier":"auto", "tool_choice":{"type":"function","name":"read"},
            "tools":[{"name":"read","parameters":{"type":"object"}}],
            "stream":true,"previous_response_id":"excluded"
        }),
    ] {
        let frozen = original(&request);
        let fields = canonical_request_fields(&request);
        assert!(fields.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let mut bytes = vec![b'{'];
        for (index, (key, value)) in fields.iter().enumerate() {
            if index != 0 {
                bytes.push(b',');
            }
            bytes.extend(serde_json::to_vec(key).unwrap());
            bytes.push(b':');
            bytes.extend(serde_json::to_vec(value).unwrap());
        }
        bytes.push(b'}');
        assert_eq!(bytes, serde_json::to_vec(&frozen).unwrap());
        let reconstructed: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reconstructed, frozen);
        assert_eq!(
            jcode_provider_core::fingerprint::stable_hash_json(&reconstructed),
            jcode_provider_core::fingerprint::stable_hash_json(&frozen)
        );
        for (key, value) in fields {
            if let Some(source) = request.get(key) {
                assert!(std::ptr::eq(source, value), "cloned {key}");
            } else {
                assert_eq!(value, &Value::Null);
            }
        }
    }
}
