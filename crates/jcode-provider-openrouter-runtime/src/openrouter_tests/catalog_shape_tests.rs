use super::*;

#[test]
fn openai_compatible_models_endpoint_allows_minimal_model_objects() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [
                {"id": "glm-51-nvfp4", "object": "model", "created": null, "owned_by": null},
                {"id": "gte-qwen2-7b", "object": "model"}
            ]
        }"#,
    )
    .expect("minimal OpenAI-compatible /models response should parse");

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "glm-51-nvfp4");
    assert_eq!(parsed[0].name, "");
}

#[test]
fn openai_compatible_models_endpoint_allows_chutes_numeric_pricing() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [{
                "id": "Qwen/Qwen3-32B-TEE",
                "root": "Qwen/Qwen3-32B-FP8",
                "price": {
                    "input": {"tao": 0.0002439746644509701, "usd": 0.08},
                    "output": {"tao": 0.0007319239933529102, "usd": 0.24}
                },
                "object": "model",
                "parent": null,
                "created": 1778439139,
                "pricing": {
                    "prompt": 0.08,
                    "completion": 0.24,
                    "input_cache_read": 0.04
                },
                "owned_by": "sglang",
                "context_length": 40960,
                "supported_features": ["json_mode", "tools"]
            }]
        }"#,
    )
    .expect("Chutes /models response with numeric pricing should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "Qwen/Qwen3-32B-TEE");
    assert_eq!(parsed[0].pricing.prompt.as_deref(), Some("0.08"));
    assert_eq!(parsed[0].pricing.completion.as_deref(), Some("0.24"));
    assert_eq!(parsed[0].pricing.input_cache_read.as_deref(), Some("0.04"));
}

#[test]
fn openai_compatible_models_endpoint_allows_together_top_level_array() {
    let parsed = parse_openai_compatible_models_response(
        r#"[
            {
                "id": "Austism/chronos-hermes-13b",
                "object": "model",
                "created": 1692896905,
                "type": "chat",
                "display_name": "Chronos Hermes (13B)",
                "context_length": 2048,
                "pricing": {
                    "input": 0.3,
                    "output": 0.3,
                    "cached_input": 0.2
                }
            }
        ]"#,
    )
    .expect("Together /models top-level array should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "Austism/chronos-hermes-13b");
    assert_eq!(parsed[0].name, "Chronos Hermes (13B)");
    assert_eq!(parsed[0].context_length, Some(2048));
    assert_eq!(parsed[0].pricing.prompt.as_deref(), Some("0.3"));
    assert_eq!(parsed[0].pricing.completion.as_deref(), Some("0.3"));
    assert_eq!(parsed[0].pricing.input_cache_read.as_deref(), Some("0.2"));
}

#[test]
fn openai_compatible_models_endpoint_allows_models_array_with_name_ids() {
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "models": [{
                "name": "accounts/fireworks/models/example",
                "displayName": "Example Fireworks Model",
                "contextLength": 8192
            }]
        }"#,
    )
    .expect("models array with name-based identifiers should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "accounts/fireworks/models/example");
    assert_eq!(parsed[0].name, "accounts/fireworks/models/example");
    assert_eq!(parsed[0].context_length, Some(8192));
}

#[test]
fn openai_compatible_models_endpoint_reads_llamacpp_meta_n_ctx() {
    // llama.cpp's /v1/models only exposes the context window inside `meta`
    // (issue #447). The `data` entry mirrors llama.cpp's response shape.
    let parsed = parse_openai_compatible_models_response(
        r#"{
            "object": "list",
            "data": [{
                "id": "unsloth/gemma-4-31B-it-UD-Q8_K_XL",
                "object": "model",
                "created": 1783253170,
                "owned_by": "llamacpp",
                "meta": {
                    "vocab_type": 2,
                    "n_vocab": 262144,
                    "n_ctx": 262144,
                    "n_ctx_train": 262144,
                    "n_embd": 5376
                }
            }]
        }"#,
    )
    .expect("llama.cpp /v1/models response should parse");

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "unsloth/gemma-4-31B-it-UD-Q8_K_XL");
    assert_eq!(parsed[0].context_length, Some(262144));
}
