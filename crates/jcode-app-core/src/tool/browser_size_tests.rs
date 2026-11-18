//! Exact legacy-wire and retention parity checks for allocation-free size guards.
use super::*;

fn legacy_retain(result: Value, trace: &mut [Value]) -> Value {
    let bytes = result.to_string().len();
    if bytes > 16_000 {
        return json!({"omitted":"Result exceeds the per-action or rolling 32000-byte retention budget. Inspect current state with a read-only direct browser action. Do not repeat side effects."});
    }
    let mut retained: usize = trace
        .iter()
        .filter_map(|v| v.get("result"))
        .map(|v| v.to_string().len())
        .sum();
    for entry in trace {
        if retained + bytes <= 32_000 {
            break;
        }
        if let Some(old) = entry.get_mut("result") {
            let old_size = old.to_string().len();
            let marker = json!({"omitted":"Older result evicted. Inspect read-only state. Do not repeat side effects."});
            let marker_size = marker.to_string().len();
            if old_size > marker_size {
                retained -= old_size - marker_size;
                *old = marker;
            }
        }
    }
    result
}

fn legacy_history(trace: &[Value]) -> Vec<Value> {
    let mut remaining = 20_000;
    let mut history = Vec::new();
    for entry in trace.iter().rev() {
        let mut item = json!({"step":entry["step"],"action":entry["action"],"label":entry["label"],"status":entry["status"]});
        for key in ["result", "after", "before"] {
            if let Some(value) = entry.get(key) {
                let size = value.to_string().len();
                if size <= remaining {
                    item[key] = value.clone();
                    remaining -= size;
                } else {
                    item[key] = json!({"omitted":"Older or oversized task evidence omitted from model context. Do not repeat side effects to recover it."});
                }
            }
        }
        history.push(item);
    }
    history.reverse();
    history
}

fn corpus() -> Vec<Value> {
    let mut values = vec![
        Value::Null,
        json!(true),
        json!(false),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(-0.0),
        json!(1e100),
        json!([]),
        json!({}),
    ];
    for text in [
        "",
        "plain",
        "\"\\\n\r\t\u{0008}\u{000c}",
        "é中😀",
        "\u{2028}\u{2029}",
    ] {
        for count in [1, 31, 2048] {
            values.push(json!(text.repeat(count)));
            values.push(json!({text: [text.repeat(count), null, 3.14159]}));
        }
    }
    values.push(Value::Object(
        (0..2000)
            .map(|i| {
                (
                    format!("key{i}\n\""),
                    json!({"text":"\\😀\n".repeat(i % 64),"n":i}),
                )
            })
            .collect(),
    ));
    for boundary in [16_000, 20_000, 32_000, MAX_OBSERVATION, MAX_REQUEST] {
        for delta in [-1isize, 0, 1] {
            values.push(json!("x".repeat((boundary as isize + delta - 2) as usize)));
        }
    }
    values
}

#[test]
fn browser_size_exact_wire_and_trim_parity() {
    for value in corpus() {
        assert_eq!(estimate_json_bytes(&value), value.to_string().len());
        assert_eq!(
            estimate_json_bytes(&value),
            serde_json::to_vec(&value).unwrap().len()
        );
        // Include nested JSON escaping used by the Jev wire state.
        let wire = json!({"state":value.to_string()});
        assert_eq!(
            estimate_json_bytes(&wire),
            serde_json::to_vec(&wire).unwrap().len()
        );
        for old_size in [0, 79, 15_999, 16_000, 16_001, 19_999, 20_000, 20_001] {
            let trace = vec![
                json!({"step":0,"result":"a".repeat(old_size),"before":value,"after":{"ok":true}}),
                json!({"step":1,"result":value}),
                json!({"step":2}),
            ];
            let mut expected = trace.clone();
            let mut actual = trace.clone();
            let expected_result = legacy_retain(value.clone(), &mut expected);
            let actual_result = retain_result(value.clone(), &mut actual);
            assert_eq!(actual_result.to_string(), expected_result.to_string());
            assert_eq!(actual, expected);
            assert_eq!(task_history(&trace), legacy_history(&trace));
        }
    }
}

#[test]
fn browser_size_retention_and_history_exact_boundaries() {
    for incoming in [15_999usize, 16_000, 16_001] {
        for total in [31_999usize, 32_000, 32_001] {
            let trace = vec![json!({"result":"x".repeat(total - incoming - 2)})];
            let result = json!("y".repeat(incoming - 2));
            let mut expected = trace.clone();
            let mut actual = trace;
            assert_eq!(
                retain_result(result.clone(), &mut actual),
                legacy_retain(result, &mut expected)
            );
            assert_eq!(actual, expected);
        }
    }
    for size in [19_999usize, 20_000, 20_001] {
        let trace = vec![json!({"result":"x".repeat(size - 2)})];
        assert_eq!(task_history(&trace), legacy_history(&trace));
        assert_eq!(
            task_history(&trace)[0]["result"].is_string(),
            size <= 20_000
        );
    }
}

#[test]
fn browser_size_exact_candidate_boundary() {
    for size in [15_999usize, 16_000, 16_001] {
        let mut raw = json!({"action":"eval","script":"return 1; //"});
        let overhead = raw.to_string().len();
        raw["script"] = json!(format!("return 1; //{}", "x".repeat(size - overhead)));
        assert_eq!(raw.to_string().len(), size);
        let input = BrowserInput {
            action: "handoff".into(),
            tab_id: Some(7),
            candidates: vec![ExactCandidate {
                label: "read state".into(),
                input: raw,
            }],
            ..Default::default()
        };
        let actual = candidates(&input, &json!({"elements":[]}));
        assert_eq!(
            actual.is_ok(),
            size <= 16_000,
            "{:?}",
            actual.as_ref().err()
        );
    }
}

#[test]
fn browser_size_observation_boundary() {
    for size in [MAX_OBSERVATION - 1, MAX_OBSERVATION, MAX_OBSERVATION + 1] {
        let mut page = json!({"elements":[],"text":""});
        let overhead = page.to_string().len();
        page["text"] = json!("x".repeat(size - overhead));
        assert_eq!(page.to_string().len(), size);
        let output = ToolOutput::new("").with_metadata(json!({"result":page}));
        assert_eq!(parse_observation(output).is_ok(), size <= MAX_OBSERVATION);
    }
}
