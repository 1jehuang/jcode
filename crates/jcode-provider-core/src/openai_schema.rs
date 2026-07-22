use serde_json::Value;
use std::collections::HashSet;

fn merge_string_sets(existing: &Value, incoming: &Value) -> Option<Value> {
    fn collect_strings(value: &Value) -> Option<Vec<String>> {
        match value {
            Value::String(s) => Some(vec![s.clone()]),
            Value::Array(items) => items
                .iter()
                .map(|item| item.as_str().map(ToString::to_string))
                .collect(),
            _ => None,
        }
    }

    let mut combined = collect_strings(existing)?;
    for item in collect_strings(incoming)? {
        if !combined.contains(&item) {
            combined.push(item);
        }
    }

    if combined.len() == 1 {
        Some(Value::String(combined.remove(0)))
    } else {
        Some(Value::Array(
            combined.into_iter().map(Value::String).collect(),
        ))
    }
}

fn merge_schema_objects(
    target: &mut serde_json::Map<String, Value>,
    incoming: &serde_json::Map<String, Value>,
) {
    for (key, incoming_value) in incoming {
        match key.as_str() {
            "properties" | "$defs" | "definitions" | "patternProperties" => {
                let Some(incoming_children) = incoming_value.as_object() else {
                    target.insert(key.clone(), incoming_value.clone());
                    continue;
                };

                match target.get_mut(key) {
                    Some(Value::Object(existing_children)) => {
                        for (child_key, child_value) in incoming_children {
                            if let Some(existing_child) = existing_children.get_mut(child_key) {
                                merge_schema_values(existing_child, child_value.clone());
                            } else {
                                existing_children.insert(child_key.clone(), child_value.clone());
                            }
                        }
                    }
                    _ => {
                        target.insert(key.clone(), Value::Object(incoming_children.clone()));
                    }
                }
            }
            "required" | "enum" | "type" => match target.get_mut(key) {
                Some(existing_value) => {
                    if let Some(merged) = merge_string_sets(existing_value, incoming_value) {
                        *existing_value = merged;
                    }
                }
                None => {
                    target.insert(key.clone(), incoming_value.clone());
                }
            },
            "description" | "title" => {
                target
                    .entry(key.clone())
                    .or_insert_with(|| incoming_value.clone());
            }
            "additionalProperties" => match target.get_mut(key) {
                Some(Value::Bool(existing_bool)) => {
                    if incoming_value == &Value::Bool(false) {
                        *existing_bool = false;
                    }
                }
                Some(Value::Object(existing_obj)) => {
                    if let Value::Object(incoming_obj) = incoming_value {
                        merge_schema_objects(existing_obj, incoming_obj);
                    } else if incoming_value == &Value::Bool(false) {
                        target.insert(key.clone(), Value::Bool(false));
                    }
                }
                Some(_) => {
                    if incoming_value == &Value::Bool(false) {
                        target.insert(key.clone(), Value::Bool(false));
                    }
                }
                None => {
                    target.insert(key.clone(), incoming_value.clone());
                }
            },
            _ => match target.get_mut(key) {
                Some(existing_value) => merge_schema_values(existing_value, incoming_value.clone()),
                None => {
                    target.insert(key.clone(), incoming_value.clone());
                }
            },
        }
    }
}

fn merge_schema_values(existing: &mut Value, incoming: Value) {
    if *existing == incoming {
        return;
    }

    match incoming {
        Value::Object(incoming_map) => {
            if let Value::Object(existing_map) = existing {
                merge_schema_objects(existing_map, &incoming_map);
            } else {
                *existing = Value::Object(incoming_map);
            }
        }
        Value::Array(incoming_items) => {
            if let Value::Array(existing_items) = existing {
                if existing_items != &incoming_items {
                    for item in incoming_items {
                        if !existing_items.contains(&item) {
                            existing_items.push(item);
                        }
                    }
                }
            } else {
                *existing = Value::Array(incoming_items);
            }
        }
        incoming_value => {
            *existing = incoming_value;
        }
    }
}

fn flatten_all_of_schema(mut map: serde_json::Map<String, Value>) -> Value {
    let Some(Value::Array(all_of_items)) = map.remove("allOf") else {
        return Value::Object(map);
    };

    let mut merged = map;
    let mut fallback_any_of = Vec::new();

    for item in all_of_items {
        match item {
            Value::Object(item_map) => merge_schema_objects(&mut merged, &item_map),
            other => fallback_any_of.push(other),
        }
    }

    if !fallback_any_of.is_empty() {
        match merged.get_mut("anyOf") {
            Some(Value::Array(existing_any_of)) => existing_any_of.extend(fallback_any_of),
            _ => {
                merged.insert("anyOf".to_string(), Value::Array(fallback_any_of));
            }
        }
    }

    Value::Object(merged)
}

pub fn openai_compatible_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                let normalized_key = if key == "oneOf" { "anyOf" } else { key };
                out.insert(normalized_key.to_string(), openai_compatible_schema(value));
            }
            flatten_all_of_schema(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(openai_compatible_schema).collect()),
        _ => schema.clone(),
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

        map.values().all(schema_supports_strict)
    }

    match schema {
        Value::Object(map) => check_map(map),
        Value::Array(items) => items.iter().all(schema_supports_strict),
        _ => true,
    }
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
}
