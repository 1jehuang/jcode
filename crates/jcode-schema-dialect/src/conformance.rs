//! Checking that a normalized schema is actually safe to send.
//!
//! Normalization is only half the guarantee: it is easy to write a dialect that
//! drops the right keyword and still emits something a provider rejects for a
//! second reason. This module states the properties a normalized schema must
//! hold so they can be asserted over the *real* tool registry in CI, where they
//! catch a bad dialect edit before a user does.
//!
//! Both directions matter and both have shipped as bugs:
//! `must_not_contain_unsupported_constructs` catches under-stripping (#754,
//! #687), and `must_preserve_meaning` catches over-stripping, which is the
//! failure mode a keyword allow-list newly makes possible.

use crate::dialect::DialectSpec;
use crate::keyword::{KeywordRole, LOAD_BEARING_KEYWORDS, keyword_role};
use serde_json::Value;

/// A property violation found in a normalized schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceError {
    /// JSON path to the offending node, e.g. `$.properties.data`.
    pub path: String,
    pub message: String,
}

/// Report properties that declare nothing about what they accept.
///
/// Legal JSON Schema (an empty schema accepts any instance) and fine for
/// Anthropic, but OpenAI's strict validator rejects the whole tool catalog over
/// it (#713). Unlike the keyword checks this is not per-dialect: no dialect
/// *rejects* a typeless property, the OpenAI path just must not claim `strict`
/// for it. Reporting it separately keeps jcode's own tools from acquiring one
/// silently, since that would cost every OpenAI-route agent its strict
/// structured-output guarantees.
pub fn untyped_properties(schema: &Value) -> Vec<ConformanceError> {
    fn declares_a_type(schema: &Value) -> bool {
        let Some(map) = schema.as_object() else {
            return schema.is_boolean();
        };
        [
            "type",
            "enum",
            "const",
            "anyOf",
            "oneOf",
            "allOf",
            "$ref",
            "properties",
            "items",
        ]
        .iter()
        .any(|keyword| map.contains_key(*keyword))
    }

    fn walk(schema: &Value, path: &str, errors: &mut Vec<ConformanceError>) {
        let Some(map) = schema.as_object() else {
            if let Some(items) = schema.as_array() {
                for (idx, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{idx}]"), errors);
                }
            }
            return;
        };
        if let Some(Value::Object(properties)) = map.get("properties") {
            for (name, property) in properties {
                let child = format!("{path}.properties.{name}");
                if !declares_a_type(property) {
                    errors.push(ConformanceError {
                        path: child.clone(),
                        message: "property declares no type, enum, or combiner, so the OpenAI \
                                  strict validator would reject the whole catalog"
                            .to_string(),
                    });
                }
                walk(property, &child, errors);
            }
        }
        for (key, value) in map {
            if key == "properties" {
                continue;
            }
            match keyword_role(key) {
                KeywordRole::Data => {}
                _ => walk(value, &format!("{path}.{key}"), errors),
            }
        }
    }

    let mut errors = Vec::new();
    walk(schema, "$", &mut errors);
    errors
}

impl std::fmt::Display for ConformanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Assert a normalized schema contains nothing `spec` is known to reject.
///
/// This is the invariant whose violation is an outage: any surviving keyword
/// outside the dialect's allow-list would 400 the entire tool catalog.
pub fn must_not_contain_unsupported_constructs(
    schema: &Value,
    spec: &DialectSpec,
) -> Vec<ConformanceError> {
    let mut errors = Vec::new();
    walk(schema, spec, "$", &mut errors);
    errors
}

fn walk(schema: &Value, spec: &DialectSpec, path: &str, errors: &mut Vec<ConformanceError>) {
    match schema {
        Value::Object(map) => {
            for (key, value) in map {
                if !spec.supports(key) {
                    errors.push(ConformanceError {
                        path: path.to_string(),
                        message: format!("keyword `{key}` is not in dialect `{}`", spec.id),
                    });
                }
                if key == "format"
                    && let Some(format) = value.as_str()
                    && !spec.supports_format(format)
                {
                    errors.push(ConformanceError {
                        path: path.to_string(),
                        message: format!("format `{format}` is not in dialect `{}`", spec.id),
                    });
                }
                if spec.transforms.flatten_all_combiners
                    && matches!(key.as_str(), "anyOf" | "oneOf" | "allOf")
                {
                    errors.push(ConformanceError {
                        path: path.to_string(),
                        message: format!(
                            "combiner `{key}` survived a dialect that flattens all combiners"
                        ),
                    });
                }
                if spec.transforms.prune_dangling_required && key == "required" {
                    check_required(map, value, path, errors);
                }

                let child_path = format!("{path}.{key}");
                match keyword_role(key) {
                    KeywordRole::SubschemaMap => {
                        if let Value::Object(children) = value {
                            for (name, child) in children {
                                walk(child, spec, &format!("{child_path}.{name}"), errors);
                            }
                        }
                    }
                    KeywordRole::SubschemaArray => {
                        if let Value::Array(items) = value {
                            for (idx, item) in items.iter().enumerate() {
                                walk(item, spec, &format!("{child_path}[{idx}]"), errors);
                            }
                        }
                    }
                    KeywordRole::Subschema => walk(value, spec, &child_path, errors),
                    KeywordRole::Data => {}
                }
            }
            if spec.transforms.require_properties_on_objects
                && matches!(map.get("type"), Some(Value::String(t)) if t == "object")
                && !map.contains_key("properties")
            {
                errors.push(ConformanceError {
                    path: path.to_string(),
                    message: "object schema is missing `properties`".to_string(),
                });
            }
        }
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                walk(item, spec, &format!("{path}[{idx}]"), errors);
            }
        }
        _ => {}
    }
}

fn check_required(
    map: &serde_json::Map<String, Value>,
    required: &Value,
    path: &str,
    errors: &mut Vec<ConformanceError>,
) {
    let Some(Value::Object(properties)) = map.get("properties") else {
        return;
    };
    let Some(names) = required.as_array() else {
        return;
    };
    for name in names.iter().filter_map(Value::as_str) {
        if !properties.contains_key(name) {
            errors.push(ConformanceError {
                path: path.to_string(),
                message: format!("`required` names `{name}`, which this object does not declare"),
            });
        }
    }
}

/// Assert normalization did not destroy what the tool means.
///
/// An allow-list makes over-stripping the new hazard: a dialect that forgot to
/// list `description` would silently delete every tool's prompt text and the
/// requests would still succeed, so nothing else would catch it. Compares the
/// normalized schema against its source.
pub fn must_preserve_meaning(original: &Value, normalized: &Value) -> Vec<ConformanceError> {
    let mut errors = Vec::new();
    compare(original, normalized, "$", &mut errors);
    errors
}

fn compare(original: &Value, normalized: &Value, path: &str, errors: &mut Vec<ConformanceError>) {
    let (Some(original_map), Some(normalized_map)) = (original.as_object(), normalized.as_object())
    else {
        return;
    };

    for key in LOAD_BEARING_KEYWORDS {
        // A combiner flatten legitimately moves properties around, so only
        // absence is checked, not equality.
        if original_map.contains_key(*key) && !normalized_map.contains_key(*key) {
            // `required` is the one load-bearing keyword a dialect may legally
            // shrink, because Gemini rejects names it cannot resolve (#655).
            if *key == "required" {
                continue;
            }
            errors.push(ConformanceError {
                path: path.to_string(),
                message: format!("load-bearing keyword `{key}` was dropped"),
            });
        }
    }

    if let (Some(Value::Object(original_props)), Some(Value::Object(normalized_props))) = (
        original_map.get("properties"),
        normalized_map.get("properties"),
    ) {
        for (name, original_child) in original_props {
            match normalized_props.get(name) {
                Some(normalized_child) => compare(
                    original_child,
                    normalized_child,
                    &format!("{path}.properties.{name}"),
                    errors,
                ),
                None => errors.push(ConformanceError {
                    path: path.to_string(),
                    message: format!("property `{name}` disappeared"),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dialect, registry};
    use serde_json::json;

    /// The property that matters: whatever goes in, what comes out is clean.
    #[test]
    fn normalizing_a_hostile_schema_satisfies_the_dialect() {
        let hostile = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "format": "uri" },
                "ids": { "type": "array", "uniqueItems": true, "items": { "type": "string" } },
                "data": {
                    "type": "object",
                    "propertyNames": { "type": "string" },
                    "additionalProperties": { "type": "string" }
                },
                "weird": { "type": "string", "x-vendor": { "nested": true } }
            },
            "required": ["url", "ghost"],
            "$schema": "https://json-schema.org/draft/2020-12/schema"
        });

        for spec in registry::ALL {
            let normalized = dialect::apply(&hostile, spec);
            let errors = must_not_contain_unsupported_constructs(&normalized, spec);
            assert!(
                errors.is_empty(),
                "dialect `{}` emitted an unsendable schema:\n{}",
                spec.id,
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }

    #[test]
    fn normalization_never_destroys_tool_meaning() {
        let schema = json!({
            "type": "object",
            "description": "a tool",
            "properties": {
                "path": { "type": "string", "description": "where" },
                "count": { "type": "integer", "minimum": 1 }
            },
            "required": ["path"]
        });
        for spec in registry::ALL {
            let normalized = dialect::apply(&schema, spec);
            let errors = must_preserve_meaning(&schema, &normalized);
            assert!(
                errors.is_empty(),
                "dialect `{}` lost meaning:\n{}",
                spec.id,
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            assert_eq!(normalized["properties"]["path"]["description"], "where");
        }
    }

    /// The checker must actually fail on a bad schema, or the sweep above is
    /// passing vacuously.
    #[test]
    fn the_checker_rejects_an_unnormalized_schema() {
        let raw = json!({
            "type": "object",
            "properties": { "data": { "type": "object", "propertyNames": { "type": "string" } } }
        });
        let errors = must_not_contain_unsupported_constructs(&raw, &registry::GEMINI);
        assert!(
            errors.iter().any(|e| e.message.contains("propertyNames")),
            "expected propertyNames to be flagged, got {errors:?}"
        );

        let stripped = json!({ "type": "object", "properties": {} });
        let lost = must_preserve_meaning(&raw, &stripped);
        assert!(
            lost.iter().any(|e| e.message.contains("disappeared")),
            "expected the lost property to be flagged, got {lost:?}"
        );
    }
}
