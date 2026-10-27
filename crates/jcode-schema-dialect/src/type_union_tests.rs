use crate::{dialect, must_not_contain_unsupported_constructs, must_preserve_meaning, registry};
use serde_json::json;

#[test]
fn gemini_lowers_nullable_types_at_every_schema_depth() {
    let schema = json!({"type":"object", "properties": {
        "tasks": {"type":["array","null"], "items":{"type":"object", "properties":{
            "label":{"type":["null","string"], "description":"Keep me"},
            "score":{"type":["number","null"], "minimum":0}
        }}},
        "literal":{"type":"object","default":{"type":["string","null"]}}
    }});
    let out = dialect::apply(&schema, &registry::GEMINI);
    let tasks = &out["properties"]["tasks"];
    assert_eq!(tasks["type"], "array");
    assert_eq!(tasks["nullable"], true);
    assert_eq!(tasks["items"]["properties"]["label"]["type"], "string");
    assert_eq!(tasks["items"]["properties"]["label"]["nullable"], true);
    assert_eq!(
        tasks["items"]["properties"]["label"]["description"],
        "Keep me"
    );
    assert_eq!(tasks["items"]["properties"]["score"]["minimum"], 0);
    assert_eq!(
        out["properties"]["literal"]["default"],
        schema["properties"]["literal"]["default"]
    );
    assert!(must_not_contain_unsupported_constructs(&out, &registry::GEMINI).is_empty());
    assert!(must_preserve_meaning(&schema, &out).is_empty());
    assert_eq!(dialect::apply(&out, &registry::GEMINI), out);
    let anthropic = dialect::apply(&schema, &registry::ANTHROPIC);
    assert_eq!(
        anthropic["properties"]["tasks"]["type"],
        schema["properties"]["tasks"]["type"]
    );
    assert_eq!(
        anthropic["properties"]["literal"]["default"],
        schema["properties"]["literal"]["default"]
    );
}

#[test]
fn gemini_preserves_multi_type_union_constraints_in_typed_branches() {
    let schema = json!({"type":"object","properties":{
        "value":{"type":["integer","string","null","string"],"description":"integer or string", "enum":[1,"one",null]}
    }});
    let out = dialect::apply(&schema, &registry::GEMINI);
    let branches = out["properties"]["value"]["anyOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0]["type"], "integer");
    assert_eq!(branches[1]["type"], "string");
    for branch in branches {
        assert_eq!(branch["nullable"], true);
        assert_eq!(branch["enum"], json!([1, "one", null]));
        assert_eq!(branch["description"], "integer or string");
    }
    assert!(must_not_contain_unsupported_constructs(&out, &registry::GEMINI).is_empty());
    assert!(must_preserve_meaning(&schema, &out).is_empty());
    assert_eq!(dialect::apply(&out, &registry::GEMINI), out);
}

#[test]
fn gemini_conformance_rejects_unlowered_type_arrays() {
    let schema = json!({"type":"object","properties":{"value":{"type":["string","null"]}}});
    let errors = must_not_contain_unsupported_constructs(&schema, &registry::GEMINI);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("type array"))
    );
    assert!(must_not_contain_unsupported_constructs(&schema, &registry::ANTHROPIC).is_empty());
}
