//! Unit tests for cache module

use crate::cache::{CacheConfig, CacheManager};
use crate::types::{CompletionRequest, CompletionResponse, RequestId, TokenUsage};
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

fn create_test_response(request_id: RequestId) -> CompletionResponse {
    CompletionResponse {
        text: "Test response".to_string(),
        request_id,
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

#[tokio::test]
async fn test_cache_put_and_get() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();
    let request = create_test_request("Hello, world!");
    let response = create_test_response(RequestId::new());

    // Put response in cache
    cache.put(&request, response.clone()).unwrap();

    // Get from cache
    let cached = cache.get(&request);
    assert!(cached.is_some());
    let cached_response = cached.unwrap();
    assert_eq!(cached_response.text, response.text);
    assert_eq!(cached_response.request_id, response.request_id);
}

#[tokio::test]
async fn test_cache_miss() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();
    let request = create_test_request("Non-existent prompt");

    // Should return None for non-cached request
    let cached = cache.get(&request);
    assert!(cached.is_none());
}

#[tokio::test]
async fn test_cache_ttl_expiry() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 1, // 1 second TTL
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();
    let request = create_test_request("Expiring soon");
    let response = create_test_response(RequestId::new());

    // Put in cache
    cache.put(&request, response.clone()).unwrap();

    // Should be available immediately
    assert!(cache.get(&request).is_some());

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Should be expired now
    let cached = cache.get(&request);
    assert!(cached.is_none());
}

#[tokio::test]
async fn test_cache_lru_eviction() {
    let config = CacheConfig {
        enabled: true,
        max_size: 3, // Small cache to trigger eviction
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();

    // Fill cache to capacity
    for i in 0..3 {
        let request = create_test_request(&format!("Request {}", i));
        let response = create_test_response(RequestId::new());
        cache.put(&request, response).unwrap();
    }

    // Add one more to trigger eviction
    let new_request = create_test_request("New request");
    let new_response = create_test_response(RequestId::new());
    cache.put(&new_request, new_response.clone()).unwrap();

    // New request should be in cache
    assert!(cache.get(&new_request).is_some());

    // Check stats - should have evicted at least one entry
    let stats = cache.stats();
    assert!(stats.total_entries <= 3);
}

#[tokio::test]
async fn test_cache_stats_single_pass() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();

    // Add multiple entries
    for i in 0..5 {
        let request = create_test_request(&format!("Request {}", i));
        let response = create_test_response(RequestId::new());
        cache.put(&request, response).unwrap();
    }

    // Get stats
    let stats = cache.stats();
    assert_eq!(stats.total_entries, 5);
    assert_eq!(stats.valid_entries, 5);
    assert_eq!(stats.expired_entries, 0);
    assert!(stats.total_accesses >= 0);
}

#[tokio::test]
async fn test_cache_invalidate() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();
    let request = create_test_request("To be invalidated");
    let response = create_test_response(RequestId::new());

    // Put in cache
    cache.put(&request, response).unwrap();
    assert!(cache.get(&request).is_some());

    // Invalidate
    let removed = cache.invalidate(&request);
    assert!(removed);

    // Should be gone now
    assert!(cache.get(&request).is_none());
}

#[tokio::test]
async fn test_cache_clear() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();

    // Add multiple entries
    for i in 0..10 {
        let request = create_test_request(&format!("Request {}", i));
        let response = create_test_response(RequestId::new());
        cache.put(&request, response).unwrap();
    }

    assert_eq!(cache.stats().total_entries, 10);

    // Clear all
    cache.clear();

    // Should be empty
    assert_eq!(cache.stats().total_entries, 0);
}

#[tokio::test]
async fn test_cache_disabled() {
    let config = CacheConfig {
        enabled: false,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();
    assert!(!cache.is_enabled());

    let request = create_test_request("Test");
    let response = create_test_response(RequestId::new());

    // Put should still work but get might behave differently
    cache.put(&request, response).unwrap();
}

#[tokio::test]
async fn test_cache_batch_eviction() {
    let config = CacheConfig {
        enabled: true,
        max_size: 5,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = CacheManager::new(config).unwrap();

    // Fill cache beyond capacity
    for i in 0..15 {
        let request = create_test_request(&format!("Request {}", i));
        let response = create_test_response(RequestId::new());
        cache.put(&request, response).unwrap();
    }

    // Stats should show batch eviction happened
    let stats = cache.stats();
    assert!(stats.total_entries <= 5);
}

#[tokio::test]
async fn test_cache_concurrent_access() {
    let config = CacheConfig {
        enabled: true,
        max_size: 100,
        ttl_secs: 3600,
        persist_to_disk: false,
        disk_path: None,
    };

    let cache = std::sync::Arc::new(CacheManager::new(config).unwrap());

    // Spawn multiple concurrent tasks
    let mut handles = vec![];
    for i in 0..10 {
        let cache_clone = cache.clone();
        let handle = tokio::spawn(async move {
            let request = create_test_request(&format!("Concurrent {}", i));
            let response = create_test_response(RequestId::new());
            cache_clone.put(&request, response).unwrap();
            cache_clone.get(&request);
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    // All entries should be in cache
    let stats = cache.stats();
    assert_eq!(stats.total_entries, 10);
}
