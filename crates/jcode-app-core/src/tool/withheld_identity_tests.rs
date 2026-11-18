use super::*;
use serde_json::json;

fn legacy(tool: &str, input: &Value) -> u64 {
    let mut input = input.clone();
    if let Some(object) = input.as_object_mut() {
        object.remove("intent");
        object.remove(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY);
        object.retain(|_, value| !value.is_null());
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool.hash(&mut hasher);
    input.to_string().hash(&mut hasher);
    hasher.finish()
}

#[test]
fn legacy_oracle() {
    let cases = [
        json!(null),
        json!(true),
        json!(false),
        json!(42),
        json!(-12.5),
        json!("雪 🦕 \" \\ \n \u{0000}"),
        json!([]),
        json!({}),
        json!([null, {"intent":"nested", "x":null}, [null]]),
        json!({"intent":"display", "accept_large_output":true, "x":null}),
        json!({"z":1,"a":2,"intent":null,"nested":{"intent":"keep","x":null}}),
        json!({"雪":"🦕", "quoted\"key":"line\nbreak", "a":[null,1]}),
    ];
    for value in cases {
        let before = value.clone();
        for tool in ["", "bash", "雪🦕"] {
            assert_eq!(
                withheld_call_key(tool, &value),
                legacy(tool, &value),
                "{value}"
            );
        }
        assert_eq!(value, before);
    }
    for size in [1, 8, 64, 512] {
        let mut object = serde_json::Map::new();
        for index in 0..size {
            object.insert(
                format!("key-{index}"),
                if index % 3 == 0 {
                    Value::Null
                } else {
                    json!({"intent":"nested", "null":null,"array":[index,"雪"]})
                },
            );
        }
        object.insert("intent".into(), json!("ignored"));
        object.insert(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY.into(), json!(true));
        let value = Value::Object(object);
        assert_eq!(withheld_call_key("read", &value), legacy("read", &value));
        assert_eq!(serde_json::to_string(&IdentityInput(&value)).unwrap(), {
            let mut old = value.clone();
            let map = old.as_object_mut().unwrap();
            map.remove("intent");
            map.remove(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY);
            map.retain(|_, v| !v.is_null());
            old.to_string()
        });
    }
}

#[test]
fn only_top_level_metadata_is_ignored() {
    let base = json!({"x":{"n":null},"values":[null]});
    let mut decorated = base.clone();
    let object = decorated.as_object_mut().unwrap();
    object.insert("intent".into(), json!(["arbitrary"]));
    object.insert(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY.into(), json!(true));
    object.insert("omitted".into(), Value::Null);
    assert_eq!(
        withheld_call_key("read", &base),
        withheld_call_key("read", &decorated)
    );
    for changed in [
        json!({"x":{},"values":[null]}),
        json!({"x":{"n":null},"values":[]}),
    ] {
        assert_ne!(
            withheld_call_key("read", &base),
            withheld_call_key("read", &changed)
        );
    }
    assert_ne!(
        withheld_call_key("read", &base),
        withheld_call_key("write", &base)
    );
}
