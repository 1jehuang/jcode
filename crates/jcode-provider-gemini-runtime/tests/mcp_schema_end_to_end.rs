//! End-to-end: a hostile MCP tool schema must reach every provider's wire
//! format in a form that provider accepts.
//!
//! The unit tests in `jcode-schema-dialect` work on schema values, and the
//! sweep in `jcode-app-core` works on jcode's own tools. Neither covers the
//! path that actually broke in #754: a third-party MCP server's `inputSchema`
//! is deserialized verbatim, becomes a `ToolDefinition`, and is serialized into
//! a provider request. This starts from the raw JSON an MCP server puts on the
//! wire and asserts on the bytes jcode sends to the provider.

use jcode_base::mcp::McpToolDef;
use jcode_message_types::ToolDefinition;

/// Verbatim `tools/list` payload from `@playwright/mcp`, the server that took
/// Antigravity + Gemini down in #754. `browser_drop` declares `propertyNames`,
/// which `generateContent` rejects with HTTP 400 for the whole request.
const PLAYWRIGHT_TOOLS_LIST: &str = r#"{
  "tools": [
    {
      "name": "browser_drop",
      "description": "Drop data onto an element",
      "inputSchema": {
        "type": "object",
        "properties": {
          "element": { "type": "string", "description": "Human-readable element description" },
          "data": {
            "type": "object",
            "additionalProperties": { "type": "string" },
            "propertyNames": { "type": "string" },
            "description": "Data to drop, as a map of MIME type to string value"
          }
        },
        "required": ["element", "data"]
      }
    }
  ]
}"#;

fn playwright_tool_definitions() -> Vec<ToolDefinition> {
    #[derive(serde::Deserialize)]
    struct ToolsList {
        tools: Vec<McpToolDef>,
    }

    let listed: ToolsList = serde_json::from_str(PLAYWRIGHT_TOOLS_LIST).expect("MCP tools/list");
    listed
        .tools
        .into_iter()
        .map(|tool| ToolDefinition {
            name: format!("mcp__playwright__{}", tool.name),
            description: tool.description.unwrap_or_default(),
            input_schema: tool.input_schema,
        })
        .collect()
}

/// Recursively search serialized request JSON for a key, the way a provider's
/// validator does when it reports "Unknown name X at ...".
fn contains_key(value: &serde_json::Value, key: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(k, v)| k == key || contains_key(v, key)),
        serde_json::Value::Array(items) => items.iter().any(|i| contains_key(i, key)),
        _ => false,
    }
}

#[test]
fn playwright_schema_reaches_gemini_without_the_keyword_that_caused_issue_754() {
    let defs = playwright_tool_definitions();
    let built = jcode_provider_gemini::build_tools(&defs).expect("gemini tools");
    let wire = serde_json::to_value(&built).expect("serialize gemini tools");

    assert!(
        !contains_key(&wire, "propertyNames"),
        "the keyword that 400s generateContent is still on the wire: {wire}"
    );
    // `additionalProperties` is rejected by the same endpoint and was already
    // stripped before #754; assert it too so a dialect edit cannot regress it.
    assert!(!contains_key(&wire, "additionalProperties"), "{wire}");

    // The tool is still usable: both parameters, their types, and the
    // prompt-visible descriptions survive.
    let parameters = &built[0].function_declarations[0].parameters;
    assert_eq!(parameters["properties"]["element"]["type"], "string");
    assert_eq!(parameters["properties"]["data"]["type"], "object");
    assert_eq!(
        parameters["properties"]["data"]["description"],
        "Data to drop, as a map of MIME type to string value"
    );
    assert_eq!(
        parameters["required"],
        serde_json::json!(["element", "data"])
    );
}

#[test]
fn playwright_schema_reaches_every_antigravity_route_cleanly() {
    let defs = playwright_tool_definitions();
    let schema = &defs[0].input_schema;

    // The route that actually reported #754, plus the sibling upstreams that
    // the same request path can be dispatched to.
    for model in ["gemini-3-flash", "claude-sonnet-4-5", "gpt-oss-120b"] {
        let normalized = jcode_provider_antigravity::antigravity_compatible_schema(schema, model);
        assert!(
            !contains_key(&normalized, "propertyNames"),
            "model `{model}` still sends propertyNames: {normalized}"
        );
        assert_eq!(
            normalized["properties"]["element"]["description"],
            "Human-readable element description",
            "model `{model}` lost a description"
        );
    }
}

#[test]
fn playwright_schema_reaches_openai_without_an_unsupported_construct() {
    let defs = playwright_tool_definitions();
    let built = jcode_provider_openai::request::build_tools(&defs);
    let wire = serde_json::to_value(&built).expect("serialize openai tools");

    // OpenAI's strict subset rejects `propertyNames` too (it is on the
    // deny-list that #687 extended).
    assert!(!contains_key(&wire, "propertyNames"), "{wire}");
    assert_eq!(wire[0]["name"], "mcp__playwright__browser_drop");
    assert_eq!(
        wire[0]["parameters"]["properties"]["element"]["description"],
        "Human-readable element description"
    );
}

/// The conformance checker agrees with the concrete assertions above. If a
/// future dialect edit makes a schema unsendable, this reports which keyword
/// and where, rather than only that some assertion failed.
#[test]
fn the_normalized_playwright_schema_conforms_to_every_dialect() {
    let defs = playwright_tool_definitions();
    let schema = &defs[0].input_schema;

    for spec in jcode_schema_dialect::registry::ALL {
        let normalized = jcode_schema_dialect::dialect::apply(schema, spec);
        let errors =
            jcode_schema_dialect::must_not_contain_unsupported_constructs(&normalized, spec);
        assert!(
            errors.is_empty(),
            "dialect `{}` would send an unacceptable schema:\n{}",
            spec.id,
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
        let lost = jcode_schema_dialect::must_preserve_meaning(schema, &normalized);
        assert!(
            lost.is_empty(),
            "dialect `{}` lost tool meaning:\n{}",
            spec.id,
            lost.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// The recovery layer must trigger on the error string the runtime actually
/// builds, not on the idealized provider sentence.
///
/// `generate_content` wraps the HTTP body as "Gemini request {method} failed
/// (HTTP 400): {body}", where the body is the raw JSON error envelope with its
/// quotes backslash-escaped, and anyhow adds a "Caused by" layer on top. A
/// classifier matching only the clean provider text would compile, pass its own
/// unit tests, and never once fire in production.
#[test]
fn recovery_triggers_on_the_error_string_the_runtime_really_builds() {
    let runtime_error = format!(
        "Gemini request {} failed (HTTP {}): {}",
        "generateContent", 400, GEMINI_400_BODY
    );
    let with_context = format!(
        "{runtime_error}\n\nCaused by:\n    Gemini request to \
         https://cloudcode-pa.googleapis.com/v1internal:generateContent failed"
    );

    let rejection = jcode_schema_dialect::classify(&with_context)
        .expect("the runtime's own error string must be recognized as a schema rejection");
    assert_eq!(
        rejection.keyword(),
        Some("propertyNames"),
        "recovery must extract the construct from the wrapped, escaped error"
    );
}

/// Verbatim HTTP 400 envelope from the Antigravity/Gemini `generateContent`
/// endpoint as quoted in issue #754, including the backslash-escaped quotes
/// that survive into the error string.
const GEMINI_400_BODY: &str = r#"{"error":{"code":400,"message":"Invalid JSON payload received. Unknown name \"propertyNames\" at 'request.tools[0].function_declarations[32].parameters.properties[0].value': Cannot find field.","status":"INVALID_ARGUMENT"}}"#;
