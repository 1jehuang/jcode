use super::*;
use serde_json::json;
use std::{hint::black_box, io, time::Instant};

#[derive(Default)]
struct Counter(usize);
impl io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0 += bytes.len();
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn writer(value: &Value) -> usize {
    let mut counter = Counter::default();
    serde_json::to_writer(&mut counter, value).unwrap();
    counter.0
}
fn recursive(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(_) => writer(value),
        Value::String(text) => string_chars(text),
        Value::Array(items) => {
            2 + items.len().saturating_sub(1) + items.iter().map(recursive).sum::<usize>()
        }
        Value::Object(items) => {
            2 + items.len().saturating_sub(1)
                + items
                    .iter()
                    .map(|(key, value)| string_chars(key) + 1 + recursive(value))
                    .sum::<usize>()
        }
    }
}
fn cases() -> Vec<(&'static str, Value)> {
    vec![
        ("null", Value::Null),
        ("tiny", json!({"type":"string"})),
        (
            "ascii",
            json!({"description":"ordinary ASCII description ".repeat(100)}),
        ),
        (
            "escaping",
            json!({"description":"\"\\\n\r\t\u{0000}\u{001f}".repeat(300)}),
        ),
        (
            "unicode",
            json!({"界🦀":"界🦀é\u{2028}\u{2029}".repeat(300)}),
        ),
        (
            "numbers",
            json!((0..512).map(|n| n as f64 * 1e-120).collect::<Vec<_>>()),
        ),
        (
            "nested",
            (0..64).fold(json!(null), |v, _| json!({"a":[v,true,{},[]]})),
        ),
        (
            "large",
            json!({"type":"object", "properties": (0..256).map(|n| (format!("field{n}"), json!({"type":"string", "description":"long schema description \"界\n".repeat(16)}))).collect::<serde_json::Map<String, Value>>()}),
        ),
    ]
}

#[test]
fn candidates_match_frozen_serialization() {
    for (_, schema) in cases() {
        assert_eq!(value_chars(&schema), schema.to_string().len());
        assert_eq!(writer(&schema), schema.to_string().len());
        assert_eq!(recursive(&schema), schema.to_string().len());
    }
}

#[test]
fn exact_bytes_after_schema_and_tool_mutation() {
    fn frozen(tool: &ToolDefinition) -> usize {
        b"{\"name\":,\"description\":,\"input_schema\":}".len()
            + string_chars(&tool.name)
            + string_chars(&tool.description)
            + tool.input_schema.to_string().len()
    }
    let controls: String = (0..=127).map(char::from).collect();
    let mut tool = ToolDefinition::new("initial", "initial", Value::Null);
    for (_, schema) in cases().into_iter().chain([
        (
            "controls",
            json!({controls.clone(): [controls, "界🦀/\u{2028}\u{2029}"]}),
        ),
        (
            "boundaries",
            json!([
                i64::MIN,
                i64::MAX,
                u64::MAX,
                -0.0,
                1e-300,
                1e300,
                {},
                [],
                "",
                true,
                false
            ]),
        ),
    ]) {
        tool.input_schema = schema;
        tool.name.push_str("\"\\\n界");
        tool.description.push_str("\t🦀");
        for deferred in [false, true] {
            tool.defer_loading = deferred;
            assert_eq!(tool.prompt_chars(), frozen(&tool));
            assert_eq!(tool.prompt_token_estimate(), frozen(&tool) / 4);
        }
        tool.input_schema = json!({"mutated": tool.input_schema, "new": "\u{0000}"});
        assert_eq!(tool.prompt_chars(), frozen(&tool));
    }
}

#[test]
fn generated_string_and_numeric_byte_parity() {
    // Exercise each ASCII escaping branch mixed with multibyte strings and
    // finite f64 bit patterns, without a property-test dependency.
    let mut state = 0x1234_5678_9abc_def0_u64;
    for i in 0..2048 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let number = serde_json::Number::from_f64(f64::from_bits(state));
        let text = format!(
            "{}界🦀{}",
            char::from((i % 128) as u8),
            char::from((state % 128) as u8)
        );
        let schema = json!({text.clone(): [text, number, state, i as i64 - 1024]});
        assert_eq!(value_chars(&schema), schema.to_string().len());
    }
}

#[test]
#[ignore = "paired unchanged-selfdev candidate benchmark"]
fn paired_schema_count_benchmark() {
    fn frozen(value: &Value) -> usize {
        value.to_string().len()
    }
    fn measure(f: fn(&Value) -> usize, value: &Value, iterations: usize) -> f64 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(f(black_box(value)));
        }
        start.elapsed().as_secs_f64() * 1e9 / iterations as f64
    }
    for (name, schema) in cases() {
        let iterations = (20_000_000 / schema.to_string().len().max(1)).clamp(200, 100_000);
        let mut ratios = [Vec::new(), Vec::new(), Vec::new()];
        for round in 0..9 {
            let fs: [fn(&Value) -> usize; 4] = [frozen, writer, recursive, value_chars];
            let mut ns = [0.0; 4];
            for index in 0..4 {
                let i = (index + round) % 4;
                ns[i] = measure(fs[i], &schema, iterations);
            }
            for i in 0..3 {
                ratios[i].push(ns[i + 1] / ns[0]);
            }
            eprintln!(
                "schema={name} round={round} baseline_ns={:.1} writer_ns={:.1} recursive_ns={:.1} counted_ns={:.1}",
                ns[0], ns[1], ns[2], ns[3]
            );
        }
        for (label, mut samples) in ["writer", "recursive", "counted"].into_iter().zip(ratios) {
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "schema={name} {label}/baseline median={:.3} range={:.3}..{:.3}",
                samples[4], samples[0], samples[8]
            );
        }
    }
}
