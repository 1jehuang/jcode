//! Recognizing a schema rejection in a provider's error text.
//!
//! The allow-list in [`crate::registry`] shrinks the blast radius of an unknown
//! construct, but it cannot be complete: a provider may reject something jcode
//! believed was fine. When that happens the failure is currently a hard 400 the
//! user reports as a GitHub issue days later. Parsing the error instead lets the
//! same turn recover, and lets jcode report exactly which keyword to add.
//!
//! The three shapes below are verbatim from the filed issues, so the parser is
//! tested against real provider output rather than invented strings.

use serde_json::Value;

/// What a provider objected to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaRejection {
    /// The offending JSON Schema keyword, when the provider names one.
    pub keyword: Option<String>,
    /// The offending `format` value, when the provider names one.
    pub format: Option<String>,
    /// The tool whose schema was rejected, when the provider names one.
    pub tool: Option<String>,
}

impl SchemaRejection {
    /// Whether this rejection identifies something actionable to strip. A
    /// rejection we cannot attribute is not worth retrying: the retry would
    /// send a byte-identical request.
    pub fn is_actionable(&self) -> bool {
        self.keyword.is_some() || self.format.is_some()
    }
}

/// Extract a schema rejection from a provider error message.
///
/// Returns `None` for errors that are not about tool schemas, so callers can
/// use this as the retry predicate directly.
pub fn classify(message: &str) -> Option<SchemaRejection> {
    let tool = extract_tool(message);

    // Gemini / Antigravity (#754):
    //   Invalid JSON payload received. Unknown name "propertyNames" at
    //   'request.tools[0].function_declarations[32].parameters.properties[0].value':
    //   Cannot find field.
    if message.contains("Cannot find field")
        && let Some(name) = extract_quoted_after(message, "Unknown name")
    {
        return Some(SchemaRejection {
            keyword: Some(name),
            format: None,
            tool,
        });
    }

    // OpenAI (#543): 'uri' is not a valid format.
    if let Some(format) = extract_quoted_before(message, "is not a valid format") {
        return Some(SchemaRejection {
            keyword: None,
            format: Some(format),
            tool,
        });
    }

    // OpenAI (#687): 'uniqueItems' is not permitted.
    if let Some(keyword) = extract_quoted_before(message, "is not permitted") {
        return Some(SchemaRejection {
            keyword: Some(keyword),
            format: None,
            tool,
        });
    }

    // Gemini (#655): required fields ['label'] are not defined in the schema
    // properties. Structural, not a keyword, but still a schema rejection the
    // caller can act on by re-normalizing with pruning enabled.
    if message.contains("are not defined in the schema properties") {
        return Some(SchemaRejection {
            keyword: Some("required".to_string()),
            format: None,
            tool,
        });
    }

    // Anthropic / OpenRouter (#495) and the Antigravity Claude bridge.
    if message.contains("does not support oneOf, allOf, or anyOf")
        || (message.contains("input_schema") && message.contains("JSON Schema draft 2020-12"))
    {
        return Some(SchemaRejection {
            keyword: Some("anyOf".to_string()),
            format: None,
            tool,
        });
    }

    None
}

/// Whether an error is *about* tool schemas at all, even if unattributable.
/// Used to label a hard failure with a useful hint instead of a raw 400.
pub fn is_schema_error(message: &str) -> bool {
    if classify(message).is_some() {
        return true;
    }
    let lowered = message.to_ascii_lowercase();
    lowered.contains("invalid_function_parameters")
        || lowered.contains("invalid schema for function")
        || (lowered.contains("function_declarations") && lowered.contains("invalid"))
}

fn extract_tool(message: &str) -> Option<String> {
    // OpenAI: Invalid schema for function 'mcp__firecrawl__firecrawl_map': ...
    extract_quoted_after(message, "for function")
}

/// Trim the quoting artifacts left by a provider that embedded a JSON document
/// inside its own error string, which is how the real Antigravity 400 arrives
/// (`Unknown name \"propertyNames\"`).
fn clean_token(token: &str) -> Option<String> {
    let token = token.trim().trim_matches('\\').trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// The first single- or double-quoted token appearing after `marker`.
fn extract_quoted_after(message: &str, marker: &str) -> Option<String> {
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let quote = rest.find(['\'', '"'])?;
    let quote_char = rest[quote..].chars().next()?;
    let after = &rest[quote + quote_char.len_utf8()..];
    let end = after.find(quote_char)?;
    clean_token(&after[..end])
}

/// The last single- or double-quoted token appearing before `marker`.
fn extract_quoted_before(message: &str, marker: &str) -> Option<String> {
    let end = message.find(marker)?;
    let head = message[..end].trim_end();
    let close = head.rfind(['\'', '"'])?;
    let quote_char = head[close..].chars().next()?;
    let open = head[..close].rfind(quote_char)?;
    clean_token(&head[open + quote_char.len_utf8()..close])
}

/// Whether a schema still contains the construct a provider rejected. Used to
/// avoid a pointless retry when the rejection names something jcode did not
/// send (which would otherwise loop).
pub fn schema_contains_keyword(schema: &Value, keyword: &str) -> bool {
    match schema {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key == keyword || schema_contains_keyword(value, keyword)),
        Value::Array(items) => items.iter().any(|i| schema_contains_keyword(i, keyword)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_real_gemini_unknown_name_400() {
        let message = r#"Antigravity generateContent failed (HTTP 400 Bad Request): "Invalid JSON payload received. Unknown name \"propertyNames\" at 'request.tools[0].function_declarations[32].parameters.properties[0].value': Cannot find field.""#;
        let rejection = classify(message).expect("recognized");
        assert_eq!(rejection.keyword.as_deref(), Some("propertyNames"));
        assert!(rejection.is_actionable());
    }

    #[test]
    fn parses_the_real_openai_unsupported_format_400() {
        let message = "invalid_request_error (invalid_function_parameters): Invalid schema for function 'mcp__firecrawl__firecrawl_map': In context=('properties', 'url'), 'uri' is not a valid format.";
        let rejection = classify(message).expect("recognized");
        assert_eq!(rejection.format.as_deref(), Some("uri"));
        assert_eq!(
            rejection.tool.as_deref(),
            Some("mcp__firecrawl__firecrawl_map")
        );
    }

    #[test]
    fn parses_the_real_openai_unpermitted_keyword_400() {
        let message = "invalid_request_error (invalid_function_parameters): Invalid schema for function 'mcp__tubealfred__youtube_channels_batch': In context=('properties', 'ids'), 'uniqueItems' is not permitted.";
        let rejection = classify(message).expect("recognized");
        assert_eq!(rejection.keyword.as_deref(), Some("uniqueItems"));
    }

    #[test]
    fn parses_the_real_gemini_dangling_required_400() {
        let message = "GenerateContentRequest.tools[0].function_declarations[3].parameters: required fields ['label'] are not defined in the schema properties";
        assert_eq!(
            classify(message).unwrap().keyword.as_deref(),
            Some("required")
        );
    }

    #[test]
    fn parses_the_real_anthropic_top_level_combiner_400() {
        let message = "input_schema does not support oneOf, allOf, or anyOf at the top level";
        assert_eq!(classify(message).unwrap().keyword.as_deref(), Some("anyOf"));
    }

    #[test]
    fn ignores_errors_that_are_not_about_schemas() {
        assert!(classify("HTTP 429 Too Many Requests").is_none());
        assert!(classify("Function call is missing a thought_signature").is_none());
        assert!(!is_schema_error("connection reset by peer"));
    }

    #[test]
    fn detects_a_keyword_nested_anywhere() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "data": { "propertyNames": { "type": "string" } } }
        });
        assert!(schema_contains_keyword(&schema, "propertyNames"));
        assert!(!schema_contains_keyword(&schema, "uniqueItems"));
    }
}
