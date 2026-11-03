use jcode_schema_dialect::{dialect::apply, registry};
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
    for dialect in [&registry::ANTIGRAVITY_CLAUDE, &registry::ANTIGRAVITY_BRIDGE] {
        let out = apply(&schema, dialect);
        assert_eq!(out["properties"]["value"]["type"], "string");
        assert_eq!(out["properties"]["value"]["description"], "A value");
        assert_eq!(apply(&out, dialect), out);
    }
}
