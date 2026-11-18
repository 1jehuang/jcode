//! Alternating optimized calling-thread CPU and allocation counts for COW repairs.
use super::*;
use crate::native_display_probe::{measure_allocations, thread_cpu_ns};
use std::hint::black_box;

fn measure(history: &[Message], repair: Repair, iterations: usize) -> (u64, u64, u64) {
    let start = thread_cpu_ns();
    let (_, (calls, bytes)) = measure_allocations(|| {
        for _ in 0..iterations {
            drop(black_box(repair(black_box(history))));
        }
    });
    (thread_cpu_ns() - start, calls, bytes)
}
#[test]
#[ignore = "optimized alternating actual Linux thread CPU/allocation probe"]
fn alternating_cow_repair_cpu_allocations() {
    assert!(!cfg!(debug_assertions));
    for (name, old, new) in repairs() {
        for shape in [
            "sparse-large",
            "dense-tiny",
            "empty-mixed",
            "retained-prefix-tail",
        ] {
            let mut history: Vec<_> = (0..256)
                .map(|i| {
                    msg(
                        if i % 2 == 0 {
                            Role::Assistant
                        } else {
                            Role::User
                        },
                        if shape == "sparse-large" {
                            vec![text()]
                        } else {
                            vec![]
                        },
                    )
                })
                .collect();
            for i in (0..240).step_by(if shape == "dense-tiny" { 4 } else { 120 }) {
                let id = format!("call-{i}");
                match name {
                    "hoist" => {
                        history[i].content = vec![call(&id)].into();
                        history[i + 3].content = vec![
                            result(&id, false),
                            ContentBlock::Image {
                                media_type: "image/png".into(),
                                data: "data".into(),
                            },
                            ContentBlock::ToolReference {
                                tool_use_id: id,
                                tool_name: "ref".into(),
                            },
                        ]
                        .into();
                    }
                    "uses" => {
                        history[i].content = vec![call(&id)].into();
                        history[i + 2].content = vec![call(&id)].into();
                    }
                    _ => {
                        history[i].content = vec![result(&id, true)].into();
                        history[i + 1].content = vec![result(&id, false)].into();
                        history[i + 3].content = vec![result(&id, false)].into();
                    }
                }
            }
            if shape == "retained-prefix-tail" {
                for message in &mut history {
                    let small = ContentBlock::Text {
                        text: "retained".repeat(16),
                        cache_control: None,
                    };
                    let mut content = vec![small.clone()];
                    content.extend(message.content.iter().cloned());
                    content.extend(std::iter::repeat_n(small, 8));
                    message.content = content.into();
                }
            }
            assert_eq!(
                serde_json::to_value(old(&history)).unwrap(),
                serde_json::to_value(new(&history)).unwrap()
            );
            for _ in 0..16 {
                drop(black_box(old(&history)));
                drop(black_box(new(&history)));
            }
            let mut totals = [(0u64, 0u64, 0u64); 2];
            let iterations = 64;
            for round in 0..20 {
                for index in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let m = measure(&history, if index == 0 { old } else { new }, iterations);
                    totals[index].0 += m.0;
                    totals[index].1 += m.1;
                    totals[index].2 += m.2;
                }
            }
            eprintln!(
                "cow-repair {name}/{shape} n={} legacy(cpu_ns,calls,bytes)={:?} optimized={:?} speedup={:.3}",
                iterations * 20,
                totals[0],
                totals[1],
                totals[0].0 as f64 / totals[1].0 as f64
            );
            assert!(totals[1].1 <= totals[0].1, "{name}/{shape} allocations");
            assert!(totals[1].2 <= totals[0].2, "{name}/{shape} bytes");
            assert!(
                totals[1].0 <= totals[0].0,
                "{name}/{shape} thread CPU regression"
            );
        }
    }
}
