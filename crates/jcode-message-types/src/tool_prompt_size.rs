//! Count prompt-visible tool JSON without cloning schemas.
use crate::ToolDefinition;
use serde_json::Value;

pub(super) fn prompt_chars(tool: &ToolDefinition) -> usize {
    b"{\"name\":,\"description\":,\"input_schema\":}".len()
        + string_chars(&tool.name)
        + string_chars(&tool.description)
        + value_chars(&tool.input_schema)
}

// Count string escaping directly. Schema counting below reuses Value's Display,
// preserving the compact serializer's escaping and number formatting.
fn string_chars(value: &str) -> usize {
    2 + value
        .chars()
        .map(|ch| match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
            '\u{00}'..='\u{1f}' => 6,
            _ => ch.len_utf8(),
        })
        .sum::<usize>()
}

fn value_chars(value: &Value) -> usize {
    struct ByteCount(usize);

    impl std::fmt::Write for ByteCount {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0 += text.len();
            Ok(())
        }
    }

    // Keep serialization inside serde_json's existing Display implementation.
    // A generic io::Write serializer or a local recursive walk is significantly
    // slower in the selfdev profile, where serde_json itself is optimized.
    let mut count = ByteCount(0);
    match std::fmt::write(&mut count, format_args!("{value}")) {
        Ok(()) => count.0,
        // Preserve the previous Display failure behavior without introducing
        // a new panic site. Value serialization normally cannot fail.
        Err(_) => value.to_string().len(),
    }
}

#[cfg(test)]
#[path = "tool_prompt_size_tests.rs"]
mod candidate_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn legacy_chars(tool: &ToolDefinition) -> usize {
        json!({"name": tool.name, "description": tool.description,
            "input_schema": tool.input_schema})
        .to_string()
        .len()
    }

    #[test]
    fn exact_legacy_prompt_bytes_and_tokens_without_schema_clones() {
        for schema in [
            json!(null),
            json!(true),
            json!(-123.25),
            json!("\n\"界🦀"),
            json!([1, "\\", false]),
            json!({"type":"object", "properties":{"escaped\"key":{"enum":["界", "\n", "\u{0000}"]}}}),
        ] {
            let mut tool = ToolDefinition::new("a\"\\\n界", "description\t🦀", schema);
            for deferred in [false, true] {
                tool.defer_loading = deferred;
                let expected = legacy_chars(&tool);
                assert_eq!(tool.prompt_chars(), expected);
                assert_eq!(tool.prompt_token_estimate(), expected / 4);
            }
            tool.input_schema = json!({"changed": "after construction"});
            assert_eq!(tool.prompt_chars(), legacy_chars(&tool));
        }
    }

    #[test]
    fn exact_count_matches_every_control_byte_and_numeric_boundaries() {
        let controls: String = (0..=127).map(char::from).collect();
        let strings = [controls, "界🦀/\u{2028}\u{2029}".repeat(50), String::new()];
        for text in strings {
            assert_eq!(
                string_chars(&text),
                serde_json::to_string(&text).unwrap().len()
            );
            let schema =
                json!({text.clone(): [text, i64::MIN, u64::MAX, -0.0, 1e-300, 1e300, {}, []]});
            assert_eq!(
                value_chars(&schema),
                serde_json::to_vec(&schema).unwrap().len()
            );
            let tool = ToolDefinition::new("", "", schema);
            assert_eq!(tool.prompt_chars(), legacy_chars(&tool));
        }
        for scalar in [json!(null), json!(false), json!(true), json!([]), json!({})] {
            assert_eq!(
                value_chars(&scalar),
                serde_json::to_vec(&scalar).unwrap().len()
            );
        }
    }

    #[test]
    #[ignore = "comparative tool accounting benchmark"]
    fn benchmark_tool_prompt_accounting() {
        use std::{hint::black_box, time::Instant};
        let schema = json!({"type":"object", "properties": (0..256)
            .map(|n| (format!("field{n}"), json!({"type":"string", "description":"long schema description \"界\n".repeat(16)})))
            .collect::<serde_json::Map<String, serde_json::Value>>()});
        let tool = ToolDefinition::new("benchmark", "description", schema);
        let expected = legacy_chars(&tool);
        assert_eq!(tool.prompt_chars(), expected);
        let start = Instant::now();
        for _ in 0..100 {
            black_box(legacy_chars(black_box(&tool)));
        }
        let legacy = start.elapsed();
        let start = Instant::now();
        for _ in 0..100 {
            black_box(tool.prompt_chars());
        }
        let counted = start.elapsed();
        eprintln!(
            "tool prompt accounting no-schema-clone: legacy={legacy:?} counted={counted:?} ratio={:.2}x",
            legacy.as_secs_f64() / counted.as_secs_f64()
        );
    }
}
