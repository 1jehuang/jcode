//! 工具执行性能基准测试
//!
//! 运行: cargo bench --bench tool_execution

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::sync::Arc;

fn bench_tool_registry_lookup(c: &mut Criterion) {
    c.bench_function("tool_registry_lookup_known", |b| {
        b.iter(|| {
            black_box(jcode::tool::Registry::resolve_tool_name_for_bench(
                black_box("shell_exec"),
            ));
        })
    });
}

fn bench_tool_registry_lookup_unknown(c: &mut Criterion) {
    c.bench_function("tool_registry_lookup_unknown", |b| {
        b.iter(|| {
            black_box(jcode::tool::Registry::resolve_tool_name_for_bench(
                black_box("nonexistent_tool_12345"),
            ));
        })
    });
}

criterion_group!(benches, bench_tool_registry_lookup, bench_tool_registry_lookup_unknown);
criterion_main!(benches);
