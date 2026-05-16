use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_something(c: &mut Criterion) {
    c.bench_function("scheduler_placeholder", |b| {
        b.iter(|| black_box(1 + 1))
    });
}

criterion_group!(benches, benchmark_something);
criterion_main!(benches);
