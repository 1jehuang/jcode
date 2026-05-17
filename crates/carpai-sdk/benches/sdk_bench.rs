//! Performance benchmarks for CarpAI SDK
//!
//! Run with: cargo bench -p carpai-sdk

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use carpai_sdk::{CacheConfig, CacheManager, CompletionRequest, CompletionResponse, RequestId, TokenUsage};
use std::time::Duration;

fn create_test_request(prompt: &str) -> CompletionRequest {
    CompletionRequest {
        prompt: prompt.to_string(),
        session_id: None,
        model: Some("test-model".to_string()),
        max_tokens: Some(100),
        temperature: Some(0.7),
        stop_sequences: vec![],
        top_p: None,
        context: Default::default(),
    }
}

fn create_test_response() -> CompletionResponse {
    CompletionResponse {
        text: "Test response content".to_string(),
        request_id: RequestId::new(),
        session_id: None,
        model: "test-model".to_string(),
        usage: TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        },
        latency_ms: 50.0,
        cached: false,
        finish_reason: Some("stop".to_string()),
    }
}

/// Benchmark cache put operations
fn bench_cache_put(c: &mut Criterion) {
    let config = CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };
    let cache = CacheManager::new(config).unwrap();

    c.bench_function("cache_put", |b| {
        b.iter(|| {
            let request = create_test_request(&format!("Prompt {}", black_box(1)));
            let response = create_test_response();
            cache.put(&request, response).unwrap();
        });
    });
}

/// Benchmark cache get operations (hit scenario)
fn bench_cache_get_hit(c: &mut Criterion) {
    let config = CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };
    let cache = CacheManager::new(config).unwrap();

    // Pre-populate cache
    let request = create_test_request("Benchmark test");
    let response = create_test_response();
    cache.put(&request, response).unwrap();

    c.bench_function("cache_get_hit", |b| {
        b.iter(|| {
            cache.get(&request);
        });
    });
}

/// Benchmark cache get operations (miss scenario)
fn bench_cache_get_miss(c: &mut Criterion) {
    let config = CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };
    let cache = CacheManager::new(config).unwrap();

    c.bench_function("cache_get_miss", |b| {
        b.iter(|| {
            let request = create_test_request(&format!("Miss {}", black_box(1)));
            cache.get(&request);
        });
    });
}

/// Benchmark cache stats collection
fn bench_cache_stats(c: &mut Criterion) {
    let config = CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };
    let cache = CacheManager::new(config).unwrap();

    // Pre-populate with 100 entries
    for i in 0..100 {
        let request = create_test_request(&format!("Entry {}", i));
        let response = create_test_response();
        cache.put(&request, response).unwrap();
    }

    c.bench_function("cache_stats", |b| {
        b.iter(|| {
            cache.stats();
        });
    });
}

/// Benchmark concurrent cache access
fn bench_cache_concurrent(c: &mut Criterion) {
    let config = CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };
    let cache = std::sync::Arc::new(CacheManager::new(config).unwrap());

    c.bench_function("cache_concurrent_10_threads", |b| {
        b.iter(|| {
            let mut handles = vec![];
            for i in 0..10 {
                let cache_clone = cache.clone();
                let handle = std::thread::spawn(move || {
                    let request = create_test_request(&format!("Thread {} Item {}", i, black_box(1)));
                    let response = create_test_response();
                    cache_clone.put(&request, response).unwrap();
                    cache_clone.get(&request);
                });
                handles.push(handle);
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });
    });
}

/// Benchmark cache eviction under load
fn bench_cache_eviction(c: &mut Criterion) {
    let config = CacheConfig {
        enabled: true,
        max_size: 100, // Small to trigger eviction
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };
    let cache = CacheManager::new(config).unwrap();

    c.bench_function("cache_eviction_200_inserts", |b| {
        b.iter(|| {
            for i in 0..200 {
                let request = create_test_request(&format!("Evict {}", black_box(i)));
                let response = create_test_response();
                cache.put(&request, response).unwrap();
            }
        });
    });
}

/// Benchmark request validation
fn bench_request_validation(c: &mut Criterion) {
    c.bench_function("request_validation_valid", |b| {
        b.iter(|| {
            let request = CompletionRequest {
                prompt: "Valid prompt".to_string(),
                session_id: None,
                model: Some("gpt-4".to_string()),
                max_tokens: Some(100),
                temperature: Some(0.7),
                stop_sequences: vec![],
                top_p: Some(0.9),
                context: Default::default(),
            };
            request.validate().unwrap();
        });
    });
}

/// Benchmark serialization/deserialization
fn bench_serialization(c: &mut Criterion) {
    let request = create_test_request("Serialization test");
    let response = create_test_response();

    c.bench_function("serialize_request", |b| {
        b.iter(|| {
            serde_json::to_string(&request).unwrap();
        });
    });

    c.bench_function("deserialize_request", |b| {
        let json = serde_json::to_string(&request).unwrap();
        b.iter(|| {
            let _: CompletionRequest = serde_json::from_str(&json).unwrap();
        });
    });

    c.bench_function("serialize_response", |b| {
        b.iter(|| {
            serde_json::to_string(&response).unwrap();
        });
    });
}

/// Benchmark key generation (hashing)
fn bench_key_generation(c: &mut Criterion) {
    let config = CacheConfig::default();
    let cache = CacheManager::new(config).unwrap();

    c.bench_function("key_generation", |b| {
        b.iter(|| {
            let request = create_test_request(&format!("Key gen {}", black_box(1)));
            // Access private method via public API
            cache.get(&request);
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .sample_size(1000)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3));
    targets = bench_cache_put, bench_cache_get_hit, bench_cache_get_miss,
              bench_cache_stats, bench_cache_concurrent, bench_cache_eviction,
              bench_request_validation, bench_serialization, bench_key_generation
);

criterion_main!(benches);
