//! LLM 完成性能基准测试
//!
//! 运行: cargo bench --bench llm_completion
//! 要求: cargo bench 功能

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_estimate_tokens(c: &mut Criterion) {
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(1000);

    c.bench_function("estimate_tokens_10k_chars", |b| {
        b.iter(|| {
            black_box(jcode::util::estimate_tokens(black_box(&text)));
        })
    });
}

fn bench_serialize_messages(c: &mut Criterion) {
    use jcode::message::{ContentBlock, Message, Role};

    let messages: Vec<Message> = (0..100)
        .map(|i| Message {
            role: if i % 2 == 0 { Role::User } else { Role::Assistant },
            content: vec![ContentBlock::Text {
                text: format!("This is test message number {} with some padding content.", i),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        })
        .collect();

    c.bench_function("serialize_100_messages", |b| {
        b.iter(|| {
            black_box(serde_json::to_string(black_box(&messages)).unwrap());
        })
    });
}

criterion_group!(benches, bench_estimate_tokens, bench_serialize_messages);
criterion_main!(benches);
