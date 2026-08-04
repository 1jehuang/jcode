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

    assert!(
        !contains_key(&wire, "propertyNames"),
        "openai kept propertyNames: {wire}"
    );
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

/// Which provider request builders reach the dialect engine.
///
/// A dialect in the registry that no provider executes is a sweep passing over
/// dead code, which is how OpenAI, OpenRouter and Anthropic ended up with
/// registry entries while still shipping their own older sanitizers. Pinning the
/// set turns the remaining migration into bounded, visible work: a provider
/// moving onto the engine fails this until the list is updated, and a provider
/// silently reverting off it fails too.
#[test]
fn provider_request_builders_that_reach_the_dialect_engine_are_pinned() {
    // (source file, reaches the engine)
    const BUILDERS: &[(&str, bool)] = &[
        ("../jcode-provider-gemini/src/lib.rs", true),
        ("../jcode-provider-antigravity/src/lib.rs", true),
        ("../jcode-provider-openrouter/src/request.rs", true),
        ("../jcode-provider-anthropic/src/lib.rs", true),
        // OpenAI's keyword subset now comes from the engine too. Strict
        // eligibility and strict normalization stay in jcode-provider-core
        // because they are OpenAI-specific and have no dialect equivalent, so
        // the engine call lives there rather than in this request builder.
        ("../jcode-provider-core/src/openai_schema.rs", true),
        ("../jcode-provider-openai/src/request.rs", false),
    ];

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut mismatches = Vec::new();

    for (relative, should_use_engine) in BUILDERS {
        let path = manifest.join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        // Ignore doc comments, so a file that only *mentions* the engine in
        // prose is not counted as using it.
        let uses_engine = source.lines().any(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//") && trimmed.contains("jcode_schema_dialect::")
        });
        if uses_engine != *should_use_engine {
            mismatches.push(format!(
                "{relative}: pinned as engine={should_use_engine}, found engine={uses_engine}"
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "provider/engine wiring changed without updating this list:\n{}",
        mismatches.join("\n")
    );
}

/// Anthropic rejects a top-level combiner and requires an object schema with a
/// `properties` map (#495's sibling constraint). Now that it runs the shared
/// engine, its wire output needs the same behavioral pin as the others.
#[test]
fn anthropic_sends_a_schema_without_a_top_level_combiner() {
    let combiner_tool = vec![ToolDefinition {
        name: "multi_action".to_string(),
        description: "probe".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "action": { "type": "string", "description": "what" } },
            "anyOf": [
                { "properties": { "label": { "type": "string" } }, "required": ["label"] },
                { "properties": { "target": { "type": "string" } } }
            ]
        }),
    }];

    let built = jcode_provider_anthropic::format_tools(&combiner_tool, false, false);
    let wire = serde_json::to_value(&built).expect("serialize");
    let schema = &wire[0]["input_schema"];

    for combiner in ["anyOf", "oneOf", "allOf"] {
        assert!(
            schema.get(combiner).is_none(),
            "anthropic kept a top-level {combiner}: {schema}"
        );
    }
    // Every branch's fields are advertised, so the model can still call any
    // action; runtime deserialization enforces which combination is valid.
    for name in ["action", "label", "target"] {
        assert!(
            schema["properties"].get(name).is_some(),
            "anthropic lost property `{name}`: {schema}"
        );
    }
    assert_eq!(schema["properties"]["action"]["description"], "what");
    // A branch-only requirement must not survive as a demand the merged object
    // cannot express.
    assert!(
        schema
            .get("required")
            .and_then(|r| r.as_array())
            .is_none_or(|r| r.iter().all(|n| n.as_str() != Some("label"))),
        "anthropic promoted an anyOf branch's requirement: {schema}"
    );

    // And a no-argument tool still gets the object shape Anthropic requires.
    let bare = vec![ToolDefinition {
        name: "noargs".to_string(),
        description: "probe".to_string(),
        input_schema: serde_json::json!({}),
    }];
    let bare_wire =
        serde_json::to_value(jcode_provider_anthropic::format_tools(&bare, false, false))
            .expect("serialize");
    assert_eq!(bare_wire[0]["input_schema"]["type"], "object");
    assert_eq!(
        bare_wire[0]["input_schema"]["properties"],
        serde_json::json!({})
    );
}

/// The property the whole system exists for: a keyword nobody has ever seen
/// cannot reach any provider.
///
/// Every issue in this class began this way. Some MCP server emitted a construct
/// that was not on the relevant deny-list, it was forwarded verbatim, and the
/// provider 400d the entire tool catalog. A deny-list can only ever contain what
/// has already broken for somebody, so this test is the difference between the
/// fix and the system: it uses an invented keyword that appears in no list, no
/// issue, and no provider documentation.
#[test]
fn a_keyword_no_deny_list_has_ever_heard_of_reaches_no_provider() {
    let novel = vec![ToolDefinition {
        name: "mcp__future__probe".to_string(),
        description: "probe".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "x": {
                    "type": "string",
                    "description": "keep me",
                    "someKeywordFromADraftThatDoesNotExistYet": { "nested": true }
                }
            },
            "required": ["x"]
        }),
    }];
    const NOVEL: &str = "someKeywordFromADraftThatDoesNotExistYet";

    let gemini =
        serde_json::to_value(jcode_provider_gemini::build_tools(&novel).expect("gemini tools"))
            .expect("serialize");
    assert!(
        !contains_key(&gemini, NOVEL),
        "gemini forwarded it: {gemini}"
    );

    let openai = serde_json::to_value(jcode_provider_openai::request::build_tools(&novel))
        .expect("serialize");
    assert!(
        !contains_key(&openai, NOVEL),
        "openai forwarded it: {openai}"
    );

    let anthropic =
        serde_json::to_value(jcode_provider_anthropic::format_tools(&novel, false, false))
            .expect("serialize");
    assert!(
        !contains_key(&anthropic, NOVEL),
        "anthropic forwarded it: {anthropic}"
    );

    let openrouter =
        jcode_provider_openrouter::request::sanitize_tool_parameters_schema(&novel[0].input_schema);
    assert!(
        !contains_key(&openrouter, NOVEL),
        "openrouter forwarded it: {openrouter}"
    );

    for model in ["gemini-3-flash", "claude-sonnet-4-5", "gpt-oss-120b"] {
        let antigravity = jcode_provider_antigravity::antigravity_compatible_schema(
            &novel[0].input_schema,
            model,
        );
        assert!(
            !contains_key(&antigravity, NOVEL),
            "antigravity/{model} forwarded it: {antigravity}"
        );
    }

    // Dropping the unknown keyword must not cost the tool its meaning.
    assert_eq!(
        gemini[0]["functionDeclarations"][0]["parameters"]["properties"]["x"]["description"],
        "keep me"
    );
    assert_eq!(openai[0]["parameters"]["properties"]["x"]["type"], "string");
}
