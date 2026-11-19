use super::{
    TOOL_INPUT_ENV_LIMIT, json_prefix, tool_input_json_prefix, truncate_bytes, truncate_tool_input,
};
use serde_json::json;

#[test]
fn json_prefix_exact_bytes_at_every_boundary() {
    let fixtures = [
        json!(null),
        json!(true),
        json!(-123.25),
        json!(""),
        json!("a\"\\\n\r\t\u{0}\u{1f}é中😀z"),
        json!({"é": [null, false, 123, {"escaped": "\"\\\n😀"}]}),
        json!("😀é中\\\"\n".repeat(4000)),
        json!({"a": "x".repeat(TOOL_INPUT_ENV_LIMIT * 4), "z": [1, 2, 3]}),
    ];
    for value in fixtures {
        let full = value.to_string();
        for limit in 0..=full.len().min(200) + 1 {
            assert_eq!(
                json_prefix(&value, limit),
                truncate_bytes(&full, limit),
                "limit {limit}"
            );
        }
        for limit in [
            TOOL_INPUT_ENV_LIMIT - 1,
            TOOL_INPUT_ENV_LIMIT,
            TOOL_INPUT_ENV_LIMIT + 1,
            full.len(),
            full.len() + 1,
            usize::MAX,
        ] {
            assert_eq!(
                json_prefix(&value, limit),
                truncate_bytes(&full, limit),
                "limit {limit}"
            );
        }
        assert_eq!(tool_input_json_prefix(&value), truncate_tool_input(&full));
    }
}

#[test]
fn json_prefix_unicode_crosses_environment_limit() {
    for width in 1..=4 {
        let value = json!(format!(
            "{}😀tail",
            "x".repeat(TOOL_INPUT_ENV_LIMIT - width)
        ));
        assert_eq!(
            tool_input_json_prefix(&value),
            truncate_tool_input(&value.to_string())
        );
    }
}

#[test]
fn json_prefix_intentionally_incomplete_json() {
    let value = json!({"input": "a".repeat(TOOL_INPUT_ENV_LIMIT * 2)});
    let prefix = tool_input_json_prefix(&value);
    assert_eq!(prefix.len(), TOOL_INPUT_ENV_LIMIT);
    assert!(serde_json::from_str::<serde_json::Value>(&prefix).is_err());
}
