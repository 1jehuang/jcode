//! 遥测系统性能基准测试
//!
//! 运行: cargo bench --bench telemetry_bench
//! 条件: jcode-telemetry crate 的 criterion bench

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_metric_recording(c: &mut Criterion) {
    let collector = jcode_telemetry::MetricsCollector::new();

    c.bench_function("record_1000_metrics", |b| {
        b.iter(|| {
            for i in 0..1000 {
                collector.record(
                    jcode_telemetry::MetricKey::LlmRequestTotal {
                        model: format!("model-{}", i % 10),
                    },
                    jcode_telemetry::MetricValue::Counter(1),
                );
            }
            black_box(collector.snapshot());
        })
    });
}

fn bench_span_create_and_finish(c: &mut Criterion) {
    c.bench_function("span_create_finish_100", |b| {
        b.iter(|| {
            for i in 0..100 {
                let span = jcode_telemetry::Span::root(&format!("operation-{}", i));
                let _ = span.child("sub-op");
                drop(span);
            }
            black_box(());
        })
    });
}

criterion_group!(benches, bench_metric_recording, bench_span_create_and_finish);
criterion_main!(benches);
