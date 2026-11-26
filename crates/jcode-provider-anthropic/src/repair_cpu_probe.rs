//! Linux-only allocation and actual calling-thread CPU probe.
use super::*;
use crate::native_display_probe::{measure_allocations, thread_cpu_ns};
use std::hint::black_box;

fn measure(
    history: &[Message],
    repair: fn(&[Message]) -> Vec<Message>,
    iterations: usize,
) -> (u64, u64, u64) {
    let start = thread_cpu_ns();
    let (_, (calls, bytes)) = measure_allocations(|| {
        for _ in 0..iterations {
            drop(black_box(repair(black_box(history))));
        }
    });
    (thread_cpu_ns() - start, calls, bytes)
}

#[test]
#[ignore = "optimized alternating CPU/allocation probe: run --release --ignored --nocapture"]
fn alternating_repair_cpu_and_allocations() {
    assert!(
        !cfg!(debug_assertions),
        "CPU probe requires an optimized build"
    );
    let normal: Vec<_> = (0..256)
        .map(|i| {
            message(
                if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                vec![text()],
            )
        })
        .collect();
    let mut valid = normal.clone();
    for (i, msg) in valid.iter_mut().enumerate().step_by(16) {
        msg.content = vec![text(), native(serde_json::json!({"type":"server_tool_use","id":format!("id-{i}")})),
            native(serde_json::json!({"type":"web_search_tool_result","tool_use_id":format!("id-{i}")}))].into();
    }
    let mut repair = normal.clone();
    for msg in repair.iter_mut().step_by(16) {
        msg.content = vec![
            text(),
            native(serde_json::json!({"type":"server_tool_use","id":"dangling"})),
            text(),
        ]
        .into();
    }
    let malformed: Vec<_> = (0..256)
        .map(|_| {
            message(
                Role::User,
                vec![
                    native(serde_json::json!({"type":"server_tool_use","id":42})),
                    native(serde_json::json!({"type":"web_search_tool_result"})),
                ],
            )
        })
        .collect();
    let empty: Vec<_> = (0..256).map(|_| message(Role::User, vec![])).collect();
    for (name, history) in [
        ("normal-large", normal),
        ("valid-native", valid),
        ("repair-mixed", repair),
        ("malformed-dangling", malformed),
        ("empty", empty),
    ] {
        parity(&history);
        for _ in 0..8 {
            drop(black_box(legacy_repair(&history)));
            drop(black_box(repair_dangling_anthropic_server_tools(&history)));
        }
        let mut legacy = (0, 0, 0);
        let mut optimized = (0, 0, 0);
        let iterations = 64;
        for round in 0..16 {
            let order: [bool; 2] = if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            };
            for new in order {
                let m = measure(
                    &history,
                    if new {
                        repair_dangling_anthropic_server_tools
                    } else {
                        legacy_repair
                    },
                    iterations,
                );
                let total = if new { &mut optimized } else { &mut legacy };
                total.0 += m.0;
                total.1 += m.1;
                total.2 += m.2;
            }
        }
        eprintln!(
            "repair {name}: operations={} legacy_cpu_ns={} optimized_cpu_ns={} legacy_allocs={} optimized_allocs={} legacy_bytes={} optimized_bytes={} speedup={:.3}",
            iterations * 16,
            legacy.0,
            optimized.0,
            legacy.1,
            optimized.1,
            legacy.2,
            optimized.2,
            legacy.0 as f64 / optimized.0 as f64
        );
        assert!(optimized.1 <= legacy.1);
        assert!(optimized.2 <= legacy.2);
        assert!(optimized.0 <= legacy.0, "thread CPU regression on {name}");
    }
}
