use jcode_schema_dialect::{
    RecoveryAction, dialect::apply, learn_from_error, must_not_contain_unsupported_constructs,
    must_preserve_meaning, recover_from_error, registry,
};
use serde_json::json;

#[test]
fn gemini_transport_normalizes_nested_nullable_type_arrays_without_touching_data() {
    let schema = json!({"type":"object","properties":{"tasks":{"type":"array","items":{
        "type":"object","properties":{"value":{"type":["string","null"],
        "default":{"type":["string","null"]}}}
    }}}});
    for dialect in [
        &registry::GEMINI,
        &registry::ANTIGRAVITY_CLAUDE,
        &registry::ANTIGRAVITY_BRIDGE,
    ] {
        let out = apply(&schema, dialect);
        let value = &out["properties"]["tasks"]["items"]["properties"]["value"];
        assert_eq!(value["type"], "string", "{}", dialect.id);
        assert_eq!(value["nullable"], true);
        assert_eq!(
            value["default"],
            schema["properties"]["tasks"]["items"]["properties"]["value"]["default"]
        );
        assert!(must_not_contain_unsupported_constructs(&out, dialect).is_empty());
        assert_eq!(apply(&out, dialect), out);
    }
    assert_eq!(apply(&schema, &registry::ANTHROPIC), schema);
}

#[test]
fn gemini_preserves_non_null_union_branches_and_bridges_choose_primary_type() {
    let schema = json!({"type":"object","properties":{"value":{
        "type":["string","integer","null"],"description":"A value"
    }}});
    let out = apply(&schema, &registry::GEMINI);
    let branches = &out["properties"]["value"]["anyOf"];
    assert_eq!(branches[0]["type"], "string");
    assert_eq!(branches[1]["type"], "integer");
    assert_eq!(branches[0]["nullable"], true);
    assert!(must_not_contain_unsupported_constructs(&out, &registry::GEMINI).is_empty());
    assert!(must_preserve_meaning(&schema, &out).is_empty());
    for dialect in [&registry::ANTIGRAVITY_CLAUDE, &registry::ANTIGRAVITY_BRIDGE] {
        let out = apply(&schema, dialect);
        assert_eq!(out["properties"]["value"]["type"], "string");
        assert_eq!(out["properties"]["value"]["description"], "A value");
        assert!(must_not_contain_unsupported_constructs(&out, dialect).is_empty());
        assert!(must_preserve_meaning(&schema, &out).is_empty());
        assert_eq!(apply(&out, dialect), out);
    }
}

#[test]
fn conformance_rejects_unnormalized_array_type_on_scalar_type_dialects() {
    let raw = json!({
        "type": "object",
        "properties": {
            "count": { "type": ["integer", "null"] }
        }
    });
    for dialect in [
        &registry::GEMINI,
        &registry::ANTIGRAVITY_CLAUDE,
        &registry::ANTIGRAVITY_BRIDGE,
    ] {
        let errors = must_not_contain_unsupported_constructs(&raw, dialect);
        assert!(
            errors
                .iter()
                .any(|e| e.path == "$.properties.count" && e.message.contains("scalar `type`")),
            "dialect `{}` failed to flag array `type`: {errors:?}",
            dialect.id
        );
    }
    assert!(must_not_contain_unsupported_constructs(&raw, &registry::OPENAI).is_empty());
}

#[test]
fn all_of_branches_preserve_types_nullable_enums_properties_and_cross_branch_required() {
    let schema = json!({
        "type": "object",
        "properties": {
            "config": {
                "description": "Endpoint configuration",
                "allOf": [
                    {
                        "type": ["object", "null"],
                        "properties": {
                            "host": { "type": "string", "description": "Hostname" }
                        }
                    },
                    {
                        "properties": {
                            "port": { "type": ["integer", "null"] },
                            "mode": {
                                "description": "Connection mode",
                                "allOf": [
                                    { "type": "string", "enum": ["tls", "plain"] }
                                ]
                            }
                        },
                        "required": ["host", "port", "ghost"]
                    }
                ]
            }
        },
        "required": ["config"]
    });

    for dialect in [
        &registry::GEMINI,
        &registry::ANTIGRAVITY_CLAUDE,
        &registry::ANTIGRAVITY_BRIDGE,
    ] {
        let out = apply(&schema, dialect);
        assert!(
            must_not_contain_unsupported_constructs(&out, dialect).is_empty(),
            "{}: {out}",
            dialect.id
        );
        assert!(
            must_preserve_meaning(&schema, &out).is_empty(),
            "{}: {out}",
            dialect.id
        );
        let config = &out["properties"]["config"];
        assert!(config.get("allOf").is_none(), "{}: {config}", dialect.id);
        assert_eq!(config["type"], "object", "{}", dialect.id);
        assert_eq!(config["nullable"], true, "{}", dialect.id);
        assert_eq!(config["description"], "Endpoint configuration");
        assert_eq!(config["properties"]["host"]["type"], "string");
        assert_eq!(config["properties"]["host"]["description"], "Hostname");
        assert_eq!(config["properties"]["port"]["type"], "integer");
        assert_eq!(config["properties"]["port"]["nullable"], true);
        assert_eq!(config["properties"]["mode"]["type"], "string");
        assert_eq!(
            config["properties"]["mode"]["enum"],
            json!(["tls", "plain"])
        );
        assert_eq!(
            config["properties"]["mode"]["description"],
            "Connection mode"
        );
        // Cross-branch `host` (declared in branch 0, required in branch 1) must
        // survive, while undeclared `ghost` is pruned.
        assert_eq!(
            config["required"],
            json!(["host", "port"]),
            "{}",
            dialect.id
        );
        assert_eq!(apply(&out, dialect), out, "{}", dialect.id);
    }
}

#[test]
fn deep_merge_property_preserves_shared_nested_object_properties_required_and_all_of() {
    let schema = json!({
        "type": "object",
        "allOf": [
            {
                "properties": {
                    "endpoint": {
                        "type": "object",
                        "description": "Primary endpoint",
                        "properties": {
                            "host": { "type": "string" }
                        },
                        "required": ["host"],
                        "allOf": [
                            {
                                "properties": {
                                    "tls": { "type": "boolean" }
                                },
                                "required": ["tls"]
                            }
                        ]
                    }
                },
                "required": ["endpoint"]
            },
            {
                "properties": {
                    "endpoint": {
                        "type": "object",
                        "properties": {
                            "port": { "type": ["integer", "null"] }
                        },
                        "required": ["host", "port"],
                        "allOf": [
                            {
                                "properties": {
                                    "timeout_ms": { "type": "integer" }
                                },
                                "required": ["timeout_ms"]
                            }
                        ]
                    }
                }
            }
        ]
    });

    for dialect in [
        &registry::OPENAI,
        &registry::GEMINI,
        &registry::ANTIGRAVITY_CLAUDE,
        &registry::ANTIGRAVITY_BRIDGE,
    ] {
        let out = apply(&schema, dialect);
        assert!(
            must_not_contain_unsupported_constructs(&out, dialect).is_empty(),
            "{}: {out}",
            dialect.id
        );
        let endpoint = &out["properties"]["endpoint"];
        assert_eq!(
            endpoint["description"], "Primary endpoint",
            "{}",
            dialect.id
        );
        assert_eq!(endpoint["properties"]["host"]["type"], "string");
        assert!(
            endpoint["properties"].get("port").is_some(),
            "{}",
            dialect.id
        );
        assert_eq!(endpoint["properties"]["tls"]["type"], "boolean");
        assert_eq!(endpoint["properties"]["timeout_ms"]["type"], "integer");
        assert_eq!(
            endpoint["required"],
            json!(["host", "port", "tls", "timeout_ms"]),
            "{}",
            dialect.id
        );
        assert!(
            endpoint.get("allOf").is_none(),
            "{}: {endpoint}",
            dialect.id
        );
    }
}

#[test]
fn fold_all_of_does_not_promote_constraint_only_or_conflicting_combiners_onto_objects() {
    let schema = json!({
        "type": "object",
        "properties": {
            "selector": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" }
                },
                "allOf": [
                    {
                        "oneOf": [
                            { "required": ["id"] },
                            { "required": ["name"] }
                        ]
                    },
                    {
                        "anyOf": [
                            { "required": ["id"] },
                            { "required": ["name"] }
                        ]
                    },
                    {
                        "not": { "required": ["id", "name"] }
                    }
                ]
            },
            "conflicting_union": {
                "description": "Two separate unions in allOf must not OR-merge",
                "allOf": [
                    { "anyOf": [{ "type": "string" }, { "type": "integer" }] },
                    { "oneOf": [{ "type": "string" }, { "type": "boolean" }] }
                ]
            }
        }
    });

    for dialect in [
        &registry::OPENAI,
        &registry::GEMINI,
        &registry::ANTIGRAVITY_CLAUDE,
        &registry::ANTIGRAVITY_BRIDGE,
    ] {
        let out = apply(&schema, dialect);
        assert!(
            must_not_contain_unsupported_constructs(&out, dialect).is_empty(),
            "{}: {out}",
            dialect.id
        );
        let selector = &out["properties"]["selector"];
        assert_eq!(selector["type"], "object", "{}", dialect.id);
        assert_eq!(selector["properties"]["id"]["type"], "string");
        assert_eq!(selector["properties"]["name"]["type"], "string");
        assert!(
            selector.get("anyOf").is_none(),
            "{}: constraint-only combiner leaked as anyOf: {selector}",
            dialect.id
        );
        assert!(
            selector.get("oneOf").is_none(),
            "{}: constraint-only combiner leaked as oneOf: {selector}",
            dialect.id
        );
        assert!(selector.get("not").is_none(), "{}", dialect.id);

        let conflicting = &out["properties"]["conflicting_union"];
        assert!(
            conflicting.get("anyOf").is_none() && conflicting.get("oneOf").is_none(),
            "{}: conflicting allOf combiners must not OR-merge: {conflicting}",
            dialect.id
        );
    }
}

#[test]
fn fold_all_of_distributes_non_object_defaults_into_concrete_combiner_branches() {
    let schema = json!({
        "type": "object",
        "properties": {
            "mode": {
                "type": "string",
                "description": "Pick a mode",
                "allOf": [
                    {
                        "anyOf": [
                            { "enum": ["fast"] },
                            { "enum": ["thorough"] }
                        ]
                    }
                ]
            },
            "values": {
                "type": "array",
                "description": "Homogeneous list",
                "allOf": [
                    {
                        "anyOf": [
                            { "items": { "type": "string" } },
                            { "items": { "type": "integer" } }
                        ]
                    }
                ]
            },
            "channel": {
                "type": "string",
                "description": "Release channel",
                "allOf": [
                    {
                        "oneOf": [
                            { "const": "stable" },
                            { "const": "canary" }
                        ]
                    }
                ]
            }
        }
    });

    for dialect in [&registry::GEMINI, &registry::OPENAI] {
        let out = apply(&schema, dialect);
        assert!(
            must_not_contain_unsupported_constructs(&out, dialect).is_empty(),
            "{}: {out}",
            dialect.id
        );
        assert!(
            must_preserve_meaning(&schema, &out).is_empty(),
            "{}: {out}",
            dialect.id
        );
        assert_eq!(apply(&out, dialect), out, "{} idempotence", dialect.id);

        let mode = &out["properties"]["mode"];
        assert!(
            mode.get("type").is_none() && mode.get("description").is_none(),
            "{}: sibling keywords must not sit alongside anyOf: {mode}",
            dialect.id
        );
        let mode_branches = mode["anyOf"].as_array().expect("mode anyOf");
        assert_eq!(mode_branches.len(), 2, "{}", dialect.id);
        assert_eq!(mode_branches[0]["type"], "string");
        assert_eq!(mode_branches[0]["description"], "Pick a mode");
        assert_eq!(mode_branches[0]["enum"], json!(["fast"]));
        assert_eq!(mode_branches[1]["type"], "string");
        assert_eq!(mode_branches[1]["description"], "Pick a mode");
        assert_eq!(mode_branches[1]["enum"], json!(["thorough"]));

        let values = &out["properties"]["values"];
        assert!(
            values.get("type").is_none() && values.get("description").is_none(),
            "{}: sibling keywords must not sit alongside anyOf: {values}",
            dialect.id
        );
        let value_branches = values["anyOf"].as_array().expect("values anyOf");
        assert_eq!(value_branches.len(), 2, "{}", dialect.id);
        assert_eq!(value_branches[0]["type"], "array");
        assert_eq!(value_branches[0]["description"], "Homogeneous list");
        assert_eq!(value_branches[0]["items"]["type"], "string");
        assert_eq!(value_branches[1]["type"], "array");
        assert_eq!(value_branches[1]["description"], "Homogeneous list");
        assert_eq!(value_branches[1]["items"]["type"], "integer");

        let channel_branches = out["properties"]["channel"]["anyOf"]
            .as_array()
            .expect("channel oneOf->anyOf");
        assert_eq!(channel_branches.len(), 2, "{}", dialect.id);
        assert_eq!(channel_branches[0]["type"], "string");
        assert_eq!(channel_branches[0]["description"], "Release channel");
        assert_eq!(channel_branches[1]["type"], "string");
        assert_eq!(channel_branches[1]["description"], "Release channel");
    }

    for dialect in [&registry::ANTIGRAVITY_CLAUDE, &registry::ANTIGRAVITY_BRIDGE] {
        let out = apply(&schema, dialect);
        assert!(
            must_not_contain_unsupported_constructs(&out, dialect).is_empty(),
            "{}: {out}",
            dialect.id
        );
        assert!(
            must_preserve_meaning(&schema, &out).is_empty(),
            "{}: {out}",
            dialect.id
        );
        assert_eq!(apply(&out, dialect), out, "{} idempotence", dialect.id);
        assert_eq!(out["properties"]["mode"]["type"], "string");
        assert_eq!(out["properties"]["mode"]["enum"], json!(["fast"]));
        assert_eq!(out["properties"]["values"]["type"], "array");
        assert_eq!(out["properties"]["values"]["items"]["type"], "string");
        assert_eq!(out["properties"]["channel"]["type"], "string");
        assert_eq!(out["properties"]["channel"]["enum"], json!(["stable"]));
    }
}

#[test]
fn proto_field_not_repeating_400_is_classified_as_unrecoverable_schema_error() {
    let message = r#"Antigravity generateContent failed (HTTP 400 Bad Request): "Invalid JSON payload received. Unknown name \"type\" at 'request.tools[0].function_declarations[21].parameters.properties[7].value': Proto field is not repeating, cannot start list.""#;
    assert!(matches!(
        recover_from_error(message, &registry::GEMINI),
        RecoveryAction::Unrecoverable { .. }
    ));
    let hint = learn_from_error(message, &registry::GEMINI).expect("labelled as schema error");
    assert!(hint.contains("schema"), "{hint}");
}
