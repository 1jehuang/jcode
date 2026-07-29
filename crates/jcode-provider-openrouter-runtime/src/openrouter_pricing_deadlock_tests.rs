//! Regression test for issue #649: `model_pricing` must not hold a read guard
//! on `models_cache` across `fetch_models()`, which awaits a write guard on the
//! same lock. With `tokio::sync::RwLock` a pending writer blocks, so the task
//! ends up waiting on a lock it is itself holding a reader for.
//!
//! Constructing the hang takes some care, and the obvious test does *not*
//! reproduce it. Two conditions must hold at once:
//!
//! 1. `models_cache.fetched` is false, so `model_pricing`'s first early return
//!    (which drops the guard) does not fire.
//! 2. A *usable disk cache exists but does not contain the requested model id*.
//!    That makes `model_pricing`'s disk-cache branch fall through without
//!    returning, while making `fetch_models()` take its disk-cache path, which
//!    is the only branch in `fetch_models` that acquires a **write** guard.
//!
//! With no disk cache at all, `fetch_models` goes straight to the HTTP call and
//! never contends for the lock, so the test passes whether or not the bug is
//! present. My first attempt made exactly that mistake.

use crate::tests::{ENV_LOCK, EnvVarGuard};

use crate::*;

#[test]
fn model_pricing_on_cold_catalog_does_not_deadlock_on_models_cache() {
    let _lock = ENV_LOCK.lock();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().to_str().expect("utf8 temp path"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_ISSUE_649_KEY", "test-key");

    let api_base = "https://issue649.models.test/v1";
    let profile = jcode_base::config::NamedProviderConfig {
        base_url: api_base.to_string(),
        api_key_env: Some("TEST_ISSUE_649_KEY".to_string()),
        model_catalog: true,
        ..Default::default()
    };

    let provider = OpenRouterProvider::new_named_openai_compatible("issue649", &profile)
        .expect("named profile should initialize");

    // A usable disk cache that does NOT contain the model we will ask about.
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let cache = serde_json::json!({
        "cached_at": now,
        "source_api_base": api_base,
        "models": [{ "id": "some-other-model" }],
    });
    std::fs::write(
        cache_dir.join("issue649_models.json"),
        serde_json::to_string(&cache).expect("serialize cache"),
    )
    .expect("write cache");

    assert!(
        provider.load_usable_model_disk_cache_entry().is_some(),
        "test setup: the disk cache must be loadable, or fetch_models() never \
         reaches the write-guard branch that produces the deadlock"
    );

    // Cold in-memory catalog, so the first early return does not fire.
    {
        let mut memory = provider.models_cache.blocking_write();
        memory.models.clear();
        memory.fetched = false;
        memory.cached_at = None;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build test runtime");

    // The bug is a hang rather than a wrong value, so the assertion is simply
    // that this returns. Before the fix it waits forever on a lock it holds a
    // reader for, and the timeout elapses.
    let completed = runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            provider.model_pricing("model-not-in-any-cache"),
        )
        .await
    });

    assert!(
        completed.is_ok(),
        "model_pricing deadlocked on a cold catalog: it held a models_cache read \
         guard across fetch_models(), which awaits a write guard on the same lock"
    );
}
