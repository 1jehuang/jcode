use super::*;
use serde_json::json;

fn context() -> ToolContext {
    ToolContext {
        session_id: "telemetry-parity".into(),
        message_id: "message".into(),
        tool_call_id: "call".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn lifecycle_json_length_and_key_labels_match_owned_serialization() {
    for input in [
        Value::Null,
        json!(true),
        json!(123.5),
        json!("text\n\"escaped\"\t🦀"),
        json!([null, false, {"nested": [1, "🦀"]}]),
        json!({}),
        json!({"z": null, "a": "\n\r\t\\\"", "🦀": [1, 2]}),
        json!({"content": "large payload\n🦀".repeat(65_536)}),
    ] {
        let before = input.clone();
        let serialized = serde_json::to_string(&input).unwrap();
        let fields = Registry::tool_lifecycle_fields("start", "write", "write", &input, &context());
        let actual = fields
            .iter()
            .find(|(key, _)| key == "input_json_bytes")
            .unwrap();
        assert_eq!(actual.1, serialized.len().to_string());
        if let Some(object) = input.as_object() {
            let mut legacy_keys = object.keys().cloned().collect::<Vec<_>>();
            legacy_keys.sort();
            assert_eq!(
                fields
                    .iter()
                    .find(|(key, _)| key == "input_keys")
                    .unwrap()
                    .1,
                legacy_keys.join(","),
            );
        } else {
            assert!(!fields.iter().any(|(key, _)| key == "input_keys"));
        }
        assert_eq!(
            input, before,
            "telemetry must not consume or mutate tool input"
        );
    }
}

#[test]
fn lifecycle_hook_input_matches_legacy_utf8_prefix() {
    for input in [
        json!(null),
        json!({"command": "printf 'hello'"}),
        json!({"content": "x".repeat(16_370) + "🦀中é"}),
        json!({"content": "\n\t\"\\🦀".repeat(65_536)}),
    ] {
        let event = Registry::post_tool_event(
            "post_tool",
            "write",
            &input,
            &context(),
            Err(&anyhow::anyhow!("fixture failure")),
            7,
        );
        let actual = &event
            .fields
            .iter()
            .find(|(key, _)| *key == "TOOL_INPUT")
            .unwrap()
            .1;
        assert_eq!(
            actual,
            crate::hooks::truncate_tool_input(&input.to_string())
        );
        assert_eq!(
            event
                .fields
                .iter()
                .find(|(key, _)| *key == "STATUS")
                .unwrap()
                .1,
            "error"
        );
    }
}

// Explicit optimized benchmark, not a timing assertion in the regular suite:
// justrust test -p jcode-app-core --release --lib lifecycle_size_efficiency_probe -- --ignored --nocapture --test-threads=1
#[test]
#[ignore = "explicit alternating CPU/wall-time probe"]
fn lifecycle_size_efficiency_probe() {
    use std::hint::black_box;
    use std::time::Instant;

    #[cfg(unix)]
    fn cpu_ns() -> u128 {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // The process CPU clock excludes time waiting for a contended build host.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) },
            0
        );
        time.tv_sec as u128 * 1_000_000_000 + time.tv_nsec as u128
    }
    #[cfg(not(unix))]
    fn cpu_ns() -> u128 {
        0
    }

    let many_fields: serde_json::Map<String, Value> = (0..512)
        .map(|index| (format!("key-{index:04}"), json!([index, "small"])))
        .collect();
    let fixtures = [
        (
            "small",
            json!({"command": "pwd", "intent": "inspect directory"}),
        ),
        ("many-fields", Value::Object(many_fields)),
        ("large-ascii", json!({"content": "x".repeat(1024 * 1024)})),
        (
            "escaped-unicode",
            json!({"content": "\n\t\"\\🦀".repeat(65_536)}),
        ),
    ];
    for (name, input) in fixtures {
        let legacy = serde_json::to_string(&input).unwrap();
        assert_eq!(
            crate::process_memory::estimate_json_bytes(&input),
            legacy.len()
        );
        let old_capacity = legacy.capacity();
        drop(legacy);
        let repeats = if name == "small" { 8192 } else { 32 };
        let mut wall = [Vec::new(), Vec::new()];
        let mut cpu = [Vec::new(), Vec::new()];
        for round in 0..25 {
            for offset in 0..2 {
                let policy = (round + offset) % 2;
                let started = Instant::now();
                let cpu_start = cpu_ns();
                for _ in 0..repeats {
                    let size = if policy == 0 {
                        serde_json::to_string(black_box(&input)).unwrap().len()
                    } else {
                        crate::process_memory::estimate_json_bytes(black_box(&input))
                    };
                    black_box(size);
                }
                let cpu_elapsed = cpu_ns() - cpu_start;
                let elapsed = started.elapsed().as_nanos();
                if round > 0 {
                    wall[policy].push(elapsed / repeats as u128);
                    cpu[policy].push(cpu_elapsed / repeats as u128);
                }
            }
        }
        for policy in 0..2 {
            wall[policy].sort_unstable();
            cpu[policy].sort_unstable();
        }
        eprintln!(
            "LIFECYCLE_SIZE {name} json_bytes={} legacy_capacity={old_capacity} counter_buffer_capacity=0 rounds=24 wall_ns_before={} wall_ns_after={} cpu_ns_before={} cpu_ns_after={}",
            crate::process_memory::estimate_json_bytes(&input),
            wall[0][12],
            wall[1][12],
            cpu[0][12],
            cpu[1][12],
        );
    }
}
