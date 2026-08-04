//! Is the dialect engine actually the code path each provider uses?
//!
//! The registry sweep proves every dialect *would* produce a sendable schema.
//! It says nothing about whether the provider's request builder calls it. Three
//! providers (OpenAI, OpenRouter, Anthropic) have dialects in the registry and
//! still ship their own older sanitizers, so the sweep was passing for code
//! nothing executes. This makes that gap explicit and bounded.
//!
//! The check is behavioral, not structural: for each provider it runs a hostile
//! schema through the *real* request builder and asserts on what would go on the
//! wire. That holds whether the provider reaches the engine or its own
//! sanitizer, so it keeps working through the migration.

use jcode_message_types::ToolDefinition;
use serde_json::Value;

/// A schema combining the trigger from every issue in this class.
fn hostile_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            // #543: unsupported string format.
            "url": { "type": "string", "format": "uri" },
            // #687: uniqueItems.
            "ids": { "type": "array", "uniqueItems": true, "items": { "type": "string" } },
            // #754: propertyNames (+ additionalProperties).
            "data": {
                "type": "object",
                "propertyNames": { "type": "string" },
                "additionalProperties": { "type": "string" },
                "description": "map of MIME type to value"
            },
            // #713: a property with no type at all.
            "value": { "description": "type depends on the sibling key" },
            // A property named like a keyword, which must never be mistaken
            // for one and deleted.
            "uniqueItems": { "type": "boolean", "description": "a real field" }
        },
        "required": ["url"]
    })
}

fn hostile_tool() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "mcp__hostile__probe".to_string(),
        description: "probe".to_string(),
        input_schema: hostile_schema(),
    }]
}

fn contains_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| k == key || contains_key(v, key)),
        Value::Array(items) => items.iter().any(|i| contains_key(i, key)),
        _ => false,
    }
}

/// Whatever normalization a provider uses, the prompt-visible description of a
/// surviving property must survive with it. Losing these is silent: requests
/// still succeed, the model is just told less.
fn assert_descriptions_survive(wire: &Value, provider: &str) {
    assert!(
        contains_key(wire, "description"),
        "{provider} dropped every description: {wire}"
    );
    let serialized = wire.to_string();
    assert!(
        serialized.contains("map of MIME type to value"),
        "{provider} dropped a nested property description: {wire}"
    );
}

#[test]
fn gemini_sends_a_clean_schema_for_the_hostile_tool() {
    let built = jcode_provider_gemini::build_tools(&hostile_tool()).expect("tools");
    let wire = serde_json::to_value(&built).expect("serialize");

    for rejected in ["propertyNames", "additionalProperties", "uniqueItems"] {
        // `uniqueItems` appears as a property NAME, so check the schema
        // position rather than the whole document.
        if rejected == "uniqueItems" {
            let parameters = &built[0].function_declarations[0].parameters;
            assert!(
                parameters["properties"]["ids"].get("uniqueItems").is_none(),
                "gemini kept the uniqueItems keyword: {parameters}"
            );
            assert_eq!(
                parameters["properties"]["uniqueItems"]["type"], "boolean",
                "gemini deleted a property named like a keyword: {parameters}"
            );
            continue;
        }
        assert!(!contains_key(&wire, rejected), "gemini kept {rejected}");
    }
    assert_descriptions_survive(&wire, "gemini");
}

#[test]
fn every_antigravity_route_sends_a_clean_schema_for_the_hostile_tool() {
    let schema = hostile_schema();
    for model in ["gemini-3-flash", "claude-sonnet-4-5", "gpt-oss-120b"] {
        let normalized = jcode_provider_antigravity::antigravity_compatible_schema(&schema, model);
        for rejected in ["propertyNames", "additionalProperties"] {
            assert!(
                !contains_key(&normalized, rejected),
                "antigravity model `{model}` kept {rejected}: {normalized}"
            );
        }
        assert_descriptions_survive(&normalized, &format!("antigravity/{model}"));
    }
}

/// OpenAI still uses its own sanitizer rather than the engine, so this asserts
/// the *outcome* the class requires: nothing OpenAI rejects goes out, and the
/// typeless property does not get a `strict` claim jcode cannot honor.
#[test]
fn openai_sends_a_clean_schema_and_does_not_overclaim_strict() {
    let built = jcode_provider_openai::request::build_tools(&hostile_tool());
    let wire = serde_json::to_value(&built).expect("serialize");

    assert!(!contains_key(&wire, "propertyNames"), "openai kept propertyNames: {wire}");
    let parameters = &wire[0]["parameters"];
    assert!(
        parameters["properties"]["ids"].get("uniqueItems").is_none(),
        "openai kept the uniqueItems keyword: {parameters}"
    );
    assert!(
        parameters["properties"]["url"].get("format").is_none(),
        "openai kept an unsupported format: {parameters}"
    );
    // #713: a typeless property must force strict off, not be rewritten away.
    assert_eq!(
        wire[0]["strict"], false,
        "openai claimed strict for a schema it rejects: {wire}"
    );
    assert!(
        parameters["properties"].get("value").is_some(),
        "openai dropped the typeless property instead of keeping it non-strict"
    );
    assert_descriptions_survive(&wire, "openai");
}

/// OpenRouter forwards to whichever upstream serves the model, so it must
/// satisfy the strictest: no top-level combiner and `properties` present on
/// object schemas (#446, #495).
#[test]
fn openrouter_sends_a_schema_its_strictest_upstream_accepts() {
    let combiner_schema = serde_json::json!({
        "type": "object",
        "properties": { "action": { "type": "string", "description": "what" } },
        "anyOf": [
            { "properties": { "label": { "type": "string" } }, "required": ["label"] },
            { "properties": { "target": { "type": "string" } } }
        ]
    });
    let normalized =
        jcode_provider_openrouter::request::sanitize_tool_parameters_schema(&combiner_schema);

    assert!(
        normalized.get("anyOf").is_none(),
        "openrouter kept a top-level combiner: {normalized}"
    );
    for name in ["action", "label", "target"] {
        assert!(
            normalized["properties"].get(name).is_some(),
            "openrouter lost property `{name}`: {normalized}"
        );
    }
    // #446: a bare no-argument object schema must gain `properties`.
    let bare =
        jcode_provider_openrouter::request::sanitize_tool_parameters_schema(&serde_json::json!({
            "type": "object"
        }));
    assert_eq!(bare["properties"], serde_json::json!({}));
}
