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
fn proto_field_not_repeating_400_is_classified_as_unrecoverable_schema_error() {
    let message = r#"Antigravity generateContent failed (HTTP 400 Bad Request): "Invalid JSON payload received. Unknown name \"type\" at 'request.tools[0].function_declarations[21].parameters.properties[7].value': Proto field is not repeating, cannot start list.""#;
    assert!(matches!(
        recover_from_error(message, &registry::GEMINI),
        RecoveryAction::Unrecoverable { .. }
    ));
    let hint = learn_from_error(message, &registry::GEMINI).expect("labelled as schema error");
    assert!(hint.contains("schema"), "{hint}");
}
