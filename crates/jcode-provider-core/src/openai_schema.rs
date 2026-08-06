use serde_json::Value;
use std::collections::HashSet;

/// Normalize a tool-parameter schema for the OpenAI function-parameters subset.
///
/// One construct OpenAI rejects fails the entire tool catalog rather than the
/// one tool, which is why this class of bug (#446, #495, #543, #687, #711, #713,
/// #754) has recurred: each fix appended a keyword to a deny-list, so the next
/// unlisted keyword from the next MCP server was the next outage.
///
/// The subset is now an allow-list in `jcode-schema-dialect`, shared with every
/// other provider, so a construct nobody has seen yet is dropped rather than
/// forwarded. Strict-mode eligibility and normalization stay here: they are
/// OpenAI-specific and have no dialect equivalent.
pub fn openai_compatible_schema(schema: &Value) -> Value {
    jcode_schema_dialect::normalize(schema, &jcode_schema_dialect::registry::OPENAI)
}

/// Whether a schema declares enough type information for OpenAI strict mode.
/// An "empty" schema like `{"description": "..."}` accepts any instance in JSON
/// Schema, but OpenAI's strict subset requires a concrete type keyword.
fn schema_has_type_info(schema: &Value) -> bool {
    match schema {
        Value::Bool(_) => false,
        Value::Object(map) => [
            "type",
            "enum",
            "const",
            "$ref",
            "anyOf",
            "oneOf",
            "allOf",
            "properties",
            "items",
        ]
        .iter()
        .any(|key| map.contains_key(*key)),
        _ => true,
    }
}

pub fn schema_supports_strict(schema: &Value) -> bool {
    fn check_map(map: &serde_json::Map<String, Value>) -> bool {
        let is_object_typed = match map.get("type") {
            Some(Value::String(t)) => t == "object",
            Some(Value::Array(types)) => types.iter().any(|v| v.as_str() == Some("object")),
            _ => false,
        };
        let has_properties = map
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|props| !props.is_empty())
            .unwrap_or(false);

        if is_object_typed && !has_properties {
            return false;
        }
        if is_object_typed {
            if matches!(map.get("additionalProperties"), Some(Value::Bool(true))) {
                return false;
            }
            if matches!(map.get("additionalProperties"), Some(Value::Object(_))) {
                return false;
            }
        }
        // A declared property that says nothing about its type is legal JSON
        // Schema (an empty schema accepts anything) and Anthropic takes it, but
        // OpenAI's strict validator rejects the whole catalog over it (#713:
        // cua-driver's `set_config.value`, whose type genuinely depends on
        // `key`). Strict eligibility must fail closed here: the schema is still
        // sent, just without `strict: true`, so the tool stays usable.
        if let Some(Value::Object(properties)) = map.get("properties")
            && properties
                .values()
                .any(|property| !declares_a_type(property))
        {
            return false;
        }

        // The same rule inside a combiner. A branch that only adds a
        // constraint (`{"required": ["memory_id"]}`) or only an enum, with no
        // type of its own, cannot satisfy OpenAI's strict object requirements
        // and fails the whole catalog (#711, observed on a real MCP catalog).
        //
        // Branches are held to a stricter bar than properties: a bare `enum`
        // is enough to describe a property, but a combiner branch must name a
        // `type` (OpenAI reports "schema must have a 'type' key"), which is the
        // same rule the registry-wide sweep already enforces on jcode's own
        // tools.
        for combiner in ["anyOf", "oneOf", "allOf"] {
            if let Some(Value::Array(branches)) = map.get(combiner)
                && branches.iter().any(|branch| !branch_names_a_type(branch))
            {
                return false;
            }
        }

        // An array whose `items` are unconstrained: strict mode requires the
        // element shape to be known (#711).
        let is_array_typed = match map.get("type") {
            Some(Value::String(t)) => t == "array",
            Some(Value::Array(types)) => types.iter().any(|v| v.as_str() == Some("array")),
            _ => false,
        };
        if is_array_typed && !map.get("items").is_some_and(declares_a_type) {
            return false;
        }

        // A `$ref` that survived normalization points at a definition the
        // request does not carry (`$defs` is stripped for some paths), so the
        // strict validator cannot resolve it (#711).
        if map.contains_key("$ref") {
            return false;
        }

        // A property carrying no type information at all (e.g. only a
        // `description`) is valid JSON Schema, but strict normalization turns it
        // into an untyped `anyOf` branch that makes OpenAI reject the entire tool
        // catalog. Fall back to non-strict instead. See issue #713.
        if let Some(Value::Object(props)) = map.get("properties")
            && props.values().any(|prop| !schema_has_type_info(prop))
        {
            return false;
        }

        map.values().all(schema_supports_strict)
    }

    match schema {
        Value::Object(map) => check_map(map),
        Value::Array(items) => items.iter().all(schema_supports_strict),
        _ => true,
    }
}

/// Whether a subschema says anything about what it accepts.
///
/// `true` for a boolean schema: `true`/`false` are complete JSON Schemas whose
/// meaning is unambiguous, unlike an object that simply omits `type`.
fn declares_a_type(schema: &Value) -> bool {
    let Some(map) = schema.as_object() else {
        return schema.is_boolean();
    };
    const TYPE_BEARING_KEYWORDS: &[&str] = &[
        "type",
        "enum",
        "const",
        "anyOf",
        "oneOf",
        "allOf",
        "$ref",
        "properties",
        "items",
    ];
    TYPE_BEARING_KEYWORDS
        .iter()
        .any(|keyword| map.contains_key(*keyword))
}

/// Whether a combiner branch names a concrete `type`.
///
/// Stricter than [`declares_a_type`]: OpenAI accepts a property described only
/// by an `enum`, but rejects a *branch* that does not name a `type`, which is
/// what #711's `mcp__cirqul__create_object` hit with an enum-only `anyOf`
/// branch. A nested combiner is accepted here and validated on recursion.
fn branch_names_a_type(branch: &Value) -> bool {
    let Some(map) = branch.as_object() else {
        return branch.is_boolean();
    };
    map.contains_key("type")
        || map.contains_key("$ref")
        || ["anyOf", "oneOf", "allOf"]
            .iter()
            .any(|combiner| map.contains_key(*combiner))
}

fn schema_is_object_typed(map: &serde_json::Map<String, Value>) -> bool {
    match map.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|v| v.as_str() == Some("object")),
        _ => false,
    }
}

fn schema_contains_null_type(schema: &Value) -> bool {
    schema
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "null")
        .unwrap_or(false)
}

pub fn make_schema_nullable(schema: Value) -> Value {
    match schema {
        Value::Object(mut map) => {
            if let Some(Value::String(t)) = map.get("type").cloned() {
                if t != "null" {
                    map.insert(
                        "type".to_string(),
                        Value::Array(vec![Value::String(t), Value::String("null".to_string())]),
                    );
                }
                return Value::Object(map);
            }

            if let Some(Value::Array(mut types)) = map.get("type").cloned() {
                if !types.iter().any(|v| v.as_str() == Some("null")) {
                    types.push(Value::String("null".to_string()));
                }
                map.insert("type".to_string(), Value::Array(types));
                return Value::Object(map);
            }

            if let Some(Value::Array(mut any_of)) = map.get("anyOf").cloned() {
                if !any_of.iter().any(schema_contains_null_type) {
                    any_of.push(serde_json::json!({ "type": "null" }));
                }
                map.insert("anyOf".to_string(), Value::Array(any_of));
                return Value::Object(map);
            }

            serde_json::json!({
                "anyOf": [
                    Value::Object(map),
                    { "type": "null" }
                ]
            })
        }
        other => serde_json::json!({
            "anyOf": [
                other,
                { "type": "null" }
            ]
        }),
    }
}

fn normalize_strict_schema_keyword(key: &str, value: &Value) -> Value {
    match key {
        "properties" | "$defs" | "definitions" | "patternProperties" => match value {
            Value::Object(children) => Value::Object(
                children
                    .iter()
                    .map(|(child_key, child_value)| {
                        (child_key.clone(), strict_normalize_schema(child_value))
                    })
                    .collect(),
            ),
            _ => strict_normalize_schema(value),
        },
        "allOf" | "anyOf" | "oneOf" | "prefixItems" => match value {
            Value::Array(items) => {
                Value::Array(items.iter().map(strict_normalize_schema).collect())
            }
            _ => strict_normalize_schema(value),
        },
        _ => strict_normalize_schema(value),
    }
}

fn existing_required_keys(map: &serde_json::Map<String, Value>) -> HashSet<String> {
    map.get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_required_properties(map: &mut serde_json::Map<String, Value>) {
    let Some(property_names) = map
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            let mut names: Vec<String> = properties.keys().cloned().collect();
            names.sort();
            names
        })
    else {
        return;
    };

    let existing_required = existing_required_keys(map);

    if let Some(Value::Object(properties)) = map.get_mut("properties") {
        for (prop_name, prop_schema) in properties.iter_mut() {
            if !existing_required.contains(prop_name) {
                *prop_schema = make_schema_nullable(prop_schema.clone());
            }
        }
    }

    map.insert(
        "required".to_string(),
        Value::Array(property_names.into_iter().map(Value::String).collect()),
    );
}

/// String `format` values accepted by OpenAI strict structured-output schemas.
/// Unknown formats (e.g. "uri") cause the API to reject the request, so they
/// are stripped during normalization.
const OPENAI_SUPPORTED_STRING_FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "ipv4",
    "ipv6",
    "uuid",
];

fn is_supported_string_format(value: &Value) -> bool {
    value
        .as_str()
        .map(|s| OPENAI_SUPPORTED_STRING_FORMATS.contains(&s))
        .unwrap_or(false)
}

pub fn strict_normalize_schema(schema: &Value) -> Value {
    fn normalize_map(map: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
        let mut out = serde_json::Map::new();
        for (key, value) in map {
            if key == "format" && !is_supported_string_format(value) {
                continue;
            }
            let normalized = normalize_strict_schema_keyword(key, value);
            out.insert(key.clone(), normalized);
        }

        let is_object_typed = schema_is_object_typed(&out);
        normalize_required_properties(&mut out);

        if is_object_typed || out.contains_key("properties") {
            out.insert("additionalProperties".to_string(), Value::Bool(false));
        }

        out
    }

    match schema {
        Value::Object(map) => Value::Object(normalize_map(map)),
        Value::Array(items) => Value::Array(items.iter().map(strict_normalize_schema).collect()),
        _ => schema.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        make_schema_nullable, openai_compatible_schema, schema_supports_strict,
        strict_normalize_schema,
    };
    use serde_json::json;

    #[test]
    fn strict_normalize_schema_marks_optional_properties_nullable_and_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "required_name": { "type": "string" },
                "optional_age": { "type": "integer" }
            },
            "required": ["required_name"]
        });

        let normalized = strict_normalize_schema(&schema);

        assert_eq!(
            normalized,
            json!({
                "type": "object",
                "properties": {
                    "required_name": { "type": "string" },
                    "optional_age": { "type": ["integer", "null"] }
                },
                "required": ["optional_age", "required_name"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn strict_normalize_schema_strips_unsupported_formats() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "format": "uri" },
                "when": { "type": "string", "format": "date-time" },
                "format": { "type": "string" }
            },
            "required": ["url", "when", "format"]
        });

        let normalized = strict_normalize_schema(&schema);

        // Unsupported format "uri" is dropped.
        assert_eq!(normalized["properties"]["url"], json!({ "type": "string" }));
        // Supported format is preserved.
        assert_eq!(
            normalized["properties"]["when"],
            json!({ "type": "string", "format": "date-time" })
        );
        // A property literally named "format" is untouched.
        assert_eq!(
            normalized["properties"]["format"],
            json!({ "type": "string" })
        );
    }

    #[test]
    fn strict_normalize_schema_strips_nested_unsupported_formats() {
        let schema = json!({
            "type": "object",
            "properties": {
                "links": {
                    "type": "array",
                    "items": { "type": "string", "format": "uri" }
                }
            },
            "required": ["links"]
        });

        let normalized = strict_normalize_schema(&schema);
        assert_eq!(
            normalized["properties"]["links"]["items"],
            json!({ "type": "string" })
        );
    }

    #[test]
    fn strict_normalize_schema_preserves_existing_nullability() {
        let schema = json!({
            "anyOf": [
                { "type": "string" },
                { "type": "null" }
            ]
        });

        assert_eq!(
            make_schema_nullable(schema.clone()),
            json!({
                "anyOf": [
                    { "type": "string" },
                    { "type": "null" }
                ]
            })
        );
    }

    #[test]
    fn strict_normalize_schema_recurses_through_nested_object_keywords() {
        let schema = json!({
            "type": "object",
            "properties": {
                "child": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                }
            }
        });

        let normalized = strict_normalize_schema(&schema);

        assert_eq!(
            normalized,
            json!({
                "type": "object",
                "properties": {
                    "child": {
                        "type": ["object", "null"],
                        "properties": {
                            "name": { "type": ["string", "null"] }
                        },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                },
                "required": ["child"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn schema_supports_strict_rejects_open_or_empty_objects() {
        assert!(!schema_supports_strict(&json!({ "type": "object" })));
        assert!(!schema_supports_strict(&json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "additionalProperties": true
        })));
        assert!(schema_supports_strict(&json!({
            "type": "object",
            "properties": { "x": { "type": "string" } },
            "additionalProperties": false
        })));
    }

    /// Regression test for issue #713: an MCP tool property with only a
    /// `description` (no type keyword) made OpenAI reject the whole tool catalog.
    #[test]
    fn schema_supports_strict_rejects_untyped_properties() {
        assert!(!schema_supports_strict(&json!({
            "type": "object",
            "properties": {
                "key": { "type": "string" },
                "value": { "description": "JSON type depends on the key." }
            },
            "additionalProperties": false
        })));
        assert!(!schema_supports_strict(&json!({
            "type": "object",
            "properties": { "value": true },
            "additionalProperties": false
        })));
        assert!(schema_supports_strict(&json!({
            "type": "object",
            "properties": {
                "key": { "type": "string" },
                "value": { "enum": ["a", "b"] }
            },
            "additionalProperties": false
        })));
    }

    #[test]
    fn openai_compatible_schema_flattens_allof_object_branches() {
        let schema = json!({
            "description": "Read params",
            "allOf": [
                {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" }
                    },
                    "required": ["file_path"]
                },
                {
                    "type": "object",
                    "properties": {
                        "start_line": { "type": "integer" }
                    }
                }
            ]
        });

        let normalized = openai_compatible_schema(&schema);
        assert!(normalized.get("allOf").is_none());
        assert_eq!(normalized["type"], json!("object"));
        assert_eq!(normalized["description"], json!("Read params"));
        assert_eq!(
            normalized["properties"]["file_path"]["type"],
            json!("string")
        );
        assert_eq!(
            normalized["properties"]["start_line"]["type"],
            json!("integer")
        );
        assert_eq!(normalized["required"], json!(["file_path"]));
    }

    /// Regression test for issue #687: a valid MCP schema using `uniqueItems`
    /// made OpenAI reject the whole tool catalog, blocking every turn.
    #[test]
    fn openai_compatible_schema_strips_unsupported_keywords() {
        let schema = json!({
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": 50,
                    "uniqueItems": true
                },
                "fields": { "type": ["string", "null"] }
            },
            "required": ["ids"],
            "minProperties": 1
        });

        let normalized = openai_compatible_schema(&schema);

        assert!(normalized["properties"]["ids"].get("uniqueItems").is_none());
        assert!(normalized.get("minProperties").is_none());
        // Supported array constraints survive.
        assert_eq!(normalized["properties"]["ids"]["minItems"], json!(1));
        assert_eq!(normalized["properties"]["ids"]["maxItems"], json!(50));
        assert_eq!(
            normalized["properties"]["ids"]["items"]["type"],
            json!("string")
        );
        assert_eq!(normalized["required"], json!(["ids"]));

        // Strict normalization keeps it clean too.
        let strict = strict_normalize_schema(&normalized);
        assert!(strict["properties"]["ids"].get("uniqueItems").is_none());
    }

    /// Stripping is keyword-aware: a *property* named like an unsupported
    /// keyword must be preserved.
    #[test]
    fn openai_compatible_schema_keeps_properties_named_like_keywords() {
        let schema = json!({
            "type": "object",
            "properties": {
                "uniqueItems": { "type": "boolean" },
                "not": { "type": "string" }
            },
            "required": ["uniqueItems"]
        });

        let normalized = openai_compatible_schema(&schema);

        assert_eq!(
            normalized["properties"]["uniqueItems"]["type"],
            json!("boolean")
        );
        assert_eq!(normalized["properties"]["not"]["type"], json!("string"));
    }

    /// Issue #713: `cua-driver`'s `set_config.value` declares a description and
    /// no type, because its type genuinely depends on the sibling `key`. That
    /// is legal JSON Schema and Anthropic accepts it, but OpenAI's strict
    /// validator rejects the entire tool catalog over it, so every
    /// OpenAI-route agent died on its first turn.
    ///
    /// The fix is to fail strict eligibility closed rather than to rewrite the
    /// schema: the tool is still advertised with its real shape, just without
    /// `strict: true`.
    #[test]
    fn issue_713_a_property_without_a_type_disqualifies_strict_mode() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "key": { "type": "string" },
                "value": { "description": "JSON type depends on the key." }
            }
        });
        assert!(
            !schema_supports_strict(&openai_compatible_schema(&schema)),
            "a typeless property must not be sent as a strict schema"
        );
    }

    /// The counterpart: failing closed must not become failing always, or every
    /// well-formed tool silently loses strict mode and the structured-output
    /// guarantees that come with it.
    #[test]
    fn a_fully_typed_schema_still_qualifies_for_strict_mode() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "where" },
                "count": { "type": "integer" },
                "mode": { "type": "string", "enum": ["fast", "slow"] },
                "nested": {
                    "type": "object",
                    "properties": { "inner": { "type": "boolean" } }
                },
                "either": { "anyOf": [{ "type": "string" }, { "type": "integer" }] }
            },
            "required": ["path"]
        });
        assert!(
            schema_supports_strict(&openai_compatible_schema(&schema)),
            "every property declares its shape, so strict must stay available"
        );
    }

    /// Issue #711, reproduced independently against master before fixing: four
    /// constructs from a real MCP catalog that jcode marked `strict: true` and
    /// OpenAI then rejected, failing the entire tool catalog.
    ///
    /// Each is legal JSON Schema that Anthropic accepts, so the fix is to fail
    /// strict eligibility closed rather than to rewrite the schema.
    #[test]
    fn issue_711_constructs_openai_rejects_do_not_claim_strict() {
        let cases: &[(&str, serde_json::Value)] = &[
            (
                // mcp__cirqul__correct_memory: a branch that only adds a
                // constraint cannot satisfy strict object requirements.
                "constraint-only anyOf branch",
                serde_json::json!({
                    "type": "object",
                    "properties": { "memory_id": { "type": "string" } },
                    "anyOf": [ { "required": ["memory_id"] } ]
                }),
            ),
            (
                // mcp__cirqul__create_object: OpenAI wants a `type` key on a
                // branch even when an enum already pins the values.
                "enum-only anyOf branch without a type",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "kind": { "anyOf": [ { "enum": ["a", "b"] }, { "type": "string" } ] }
                    }
                }),
            ),
            (
                "array with unconstrained items",
                serde_json::json!({
                    "type": "object",
                    "properties": { "tags": { "type": "array" } }
                }),
            ),
            (
                // `$defs` is stripped on some paths, so a surviving `$ref`
                // cannot be resolved by the validator.
                "unresolvable $ref",
                serde_json::json!({
                    "type": "object",
                    "properties": { "node": { "$ref": "#/$defs/missing" } }
                }),
            ),
        ];

        for (label, schema) in cases {
            assert!(
                !schema_supports_strict(&openai_compatible_schema(schema)),
                "{label} must not be sent as a strict schema"
            );
        }
    }

    /// Failing closed must not become failing always: every construct below is
    /// well formed, and losing strict mode for them would quietly drop the
    /// structured-output guarantees on every OpenAI-route tool call.
    #[test]
    fn well_formed_schemas_keep_strict_mode() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "where" },
                "tags": { "type": "array", "items": { "type": "string" } },
                "mode": { "enum": ["fast", "slow"] },
                "either": { "anyOf": [ { "type": "string" }, { "type": "integer" } ] },
                "nested": { "type": "object", "properties": { "x": { "type": "boolean" } } }
            },
            "required": ["path"]
        });
        assert!(
            schema_supports_strict(&openai_compatible_schema(&schema)),
            "a well-formed schema must keep strict mode"
        );
    }
}
