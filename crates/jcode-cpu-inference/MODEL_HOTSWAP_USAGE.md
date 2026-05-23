# Model Hot-Swapping & Graceful Shutdown Usage Guide (P1-5)

## Overview

This document describes the P1-5 optimization features for zero-downtime model management in CarpAI's inference engine.

### Key Features

1. **Graceful Shutdown**
   - Drain active requests before stopping
   - Configurable timeout (default 30s)
   - Automatic KV Cache snapshot saving

2. **Blue-Green Deployment**
   - Start new model in standby mode
   - Atomic switchover when new model is ready
   - Zero downtime during model updates

3. **State Snapshot/Restore**
   - Save KV Cache snapshots for fast recovery
   - Metadata tracking for version management
   - Configurable snapshot directory

4. **Client Retry Hints**
   - Inform clients about model state changes
   - Provide alternative model suggestions
   - Reduce failed requests during transitions

## Quick Start

### Basic Graceful Shutdown

```rust
use std::sync::Arc;
use jcode_cpu_inference::{CpuEngine, model_lifecycle_manager::{ModelLifecycleManager, GracefulShutdownConfig}};

// Create engine and lifecycle manager
let engine = Arc::new(CpuEngine::new());
let config = GracefulShutdownConfig::default();
let manager = ModelLifecycleManager::new(engine.clone(), config);

// Start a model
manager.start_model(
    "llama-7b",
    &PathBuf::from("/models/llama-7b.gguf"),
    4096,  // context size
    8      // threads
).await?;

// ... serve requests ...

// Gracefully stop (waits for active requests to complete)
manager.graceful_stop("llama-7b").await?;
```

### Custom Configuration

```rust
use std::path::PathBuf;

let config = GracefulShutdownConfig {
    drain_timeout_secs: 60,           // Wait up to 60s
    check_interval_ms: 200,           // Check every 200ms
    save_cache_snapshot: true,        // Save KV Cache
    snapshot_dir: PathBuf::from("/data/snapshots"),
};

let manager = ModelLifecycleManagerBuilder::new(engine)
    .drain_timeout(60)
    .check_interval(200)
    .save_cache_snapshot(true)
    .snapshot_dir(PathBuf::from("/data/snapshots"))
    .build();
```

### Blue-Green Hot-Swap (Zero Downtime)

```rust
// Old model is serving traffic
manager.start_model(
    "llama-7b-v1",
    &PathBuf::from("/models/llama-7b-v1.gguf"),
    4096, 8
).await?;

// ... later, deploy v2 without downtime ...

manager.hot_swap(
    "llama-7b-v1",                    // old model
    "llama-7b-v2",                    // new model
    &PathBuf::from("/models/llama-7b-v2.gguf"),
    4096, 8
).await?;

// Traffic automatically routes to v2 after swap completes
```

### Request Tracking

```rust
// In your request handler:
async fn handle_request(&self, model_name: &str, prompt: &str) -> Result<String> {
    // Check if model can accept requests
    if !self.manager.can_accept_requests(model_name).await {
        // Get retry hint for client
        if let Some(hint) = self.manager.get_retry_hint(model_name).await {
            return Err(anyhow::anyhow!(
                "Model unavailable: {}. {}",
                hint.reason,
                if let Some(alt) = hint.alternative_model {
                    format!("Try using '{}' instead.", alt)
                } else {
                    format!("Retry after {}ms.", hint.retry_after_ms)
                }
            ));
        }
    }

    // Increment active request counter
    self.manager.increment_active_requests(model_name).await;

    // Process request
    let result = self.process_inference(model_name, prompt).await;

    // Decrement counter when done
    self.manager.decrement_active_requests(model_name).await;

    result
}
```

### Snapshot Management

```rust
// List available snapshots for a model
let snapshots = manager.list_snapshots("llama-7b").await;
for snap in &snapshots {
    println!(
        "Snapshot: {} at {} (cache size: {} bytes)",
        snap.model_name,
        snap.timestamp,
        snap.kv_cache_size_bytes
    );
}

// Restore from snapshot (fast warmup)
if let Some(path) = manager.restore_from_snapshot(
    "llama-7b",
    snapshots[0].timestamp.timestamp()
).await? {
    println!("Restored from: {:?}", path);
}
```

### Monitoring Model States

```rust
// List all models and their states
let models = manager.list_models().await;
for (name, state) in &models {
    println!("{}: {}", name, state);
}
// Output:
// llama-7b-v1: draining (3 active)
// llama-7b-v2: active
// llama-13b: standby
// mistral-7b: stopped

// Get specific model state
if let Some(state) = manager.get_model_state("llama-7b").await {
    match state {
        ModelState::Active => println!("Serving traffic"),
        ModelState::Draining { active_requests, .. } => {
            println!("Draining: {} requests remaining", active_requests)
        }
        ModelState::Standby => println!("Warmed up, not serving"),
        ModelState::Stopped => println!("Not running"),
    }
}
```

## State Machine

```
                    ┌──────────┐
                    │ Stopped  │
                    └────┬─────┘
                         │ start_model()
                         ▼
                    ┌──────────┐
              ┌─────│ Standby  │─────┐
              │     └────┬─────┘     │
              │          │ promote   │
              │          ▼           │
              │     ┌──────────┐     │
              │     │  Active  │     │
              │     └────┬─────┘     │
              │          │           │
              │          │ graceful_stop() or hot_swap()
              │          ▼           │
              │     ┌──────────┐     │
              └────▶│ Draining │─────┘
                    └────┬─────┘
                         │ all requests complete
                         │ or timeout
                         ▼
                    ┌──────────┐
                    │ Stopped  │
                    └──────────┘
```

## Performance Characteristics

| Operation | Typical Duration | Impact on Requests |
|-----------|------------------|-------------------|
| Model startup | 5-30s (depends on size) | None (standby) |
| Graceful stop | <30s (configurable) | New requests rejected |
| Hot-swap total | 10-60s | Zero downtime |
| Snapshot save | 1-5s | None (async) |
| Snapshot restore | 2-10s | Faster warmup |

## Integration with Distributed Inference

The lifecycle manager integrates with the distributed coordinator:

```rust
use jcode_distributed_inference::coordinator_client::DistributedCoordinatorClient;

struct DistributedInferenceService {
    local_manager: ModelLifecycleManager,
    remote_coordinators: HashMap<String, DistributedCoordinatorClient>,
}

impl DistributedInferenceService {
    async fn handle_model_update(&self, model_name: &str, new_version: &str) -> Result<()> {
        // 1. Update local model
        self.local_manager.hot_swap(
            &format!("{}-{}", model_name, "old"),
            &format!("{}-{}", model_name, new_version),
            &new_model_path,
            ctx_size, threads
        ).await?;

        // 2. Notify remote workers
        for (worker_id, coordinator) in &mut self.remote_coordinators {
            info!("Notifying worker {} to update model", worker_id);
            // Send update command via gRPC
            // coordinator.send_model_update(...).await?;
        }

        Ok(())
    }
}
```

## Error Handling

```rust
match manager.graceful_stop("llama-7b").await {
    Ok(_) => println!("Model stopped cleanly"),
    Err(e) if e.to_string().contains("timeout") => {
        warn!("Drain timeout - forcing stop");
        manager.engine.stop("llama-7b").await?;
    }
    Err(e) => return Err(e),
}
```

## Best Practices

### 1. Always Track Active Requests

```rust
// Wrap all inference calls with increment/decrement
struct InferenceHandler {
    manager: ModelLifecycleManager,
}

impl InferenceHandler {
    async fn infer(&self, model: &str, input: &str) -> Result<String> {
        self.manager.increment_active_requests(model).await;
        let _guard = RequestGuard::new(&self.manager, model); // RAII pattern

        // ... perform inference ...
    }
}

struct RequestGuard<'a> {
    manager: &'a ModelLifecycleManager,
    model: String,
}

impl<'a> RequestGuard<'a> {
    fn new(manager: &'a ModelLifecycleManager, model: &str) -> Self {
        Self {
            manager,
            model: model.to_string(),
        }
    }
}

impl<'a> Drop for RequestGuard<'a> {
    fn drop(&mut self) {
        let manager = self.manager.clone();
        let model = self.model.clone();
        tokio::spawn(async move {
            manager.decrement_active_requests(&model).await;
        });
    }
}
```

### 2. Use Blue-Green for Production Updates

```rust
// DON'T: Stop then start (causes downtime)
manager.graceful_stop("llama-v1").await?;
manager.start_model("llama-v2", ...).await?; // Gap here!

// DO: Hot-swap (zero downtime)
manager.hot_swap("llama-v1", "llama-v2", ...).await?;
```

### 3. Configure Appropriate Timeouts

```rust
// For fast-draining services (few concurrent requests)
let config = GracefulShutdownConfig {
    drain_timeout_secs: 10,
    check_interval_ms: 100,
    ..Default::default()
};

// For high-concurrency services
let config = GracefulShutdownConfig {
    drain_timeout_secs: 120,
    check_interval_ms: 1000,
    ..Default::default()
};
```

### 4. Monitor Snapshot Disk Usage

```rust
// Periodically clean old snapshots
async fn cleanup_old_snapshots(manager: &ModelLifecycleManager, max_age_days: u64) {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);

    for model in manager.list_models().await.keys() {
        let snapshots = manager.list_snapshots(model).await;
        for snap in snapshots {
            if snap.timestamp < cutoff {
                if let Err(e) = tokio::fs::remove_file(&snap.snapshot_path).await {
                    warn!("Failed to remove old snapshot: {}", e);
                }
            }
        }
    }
}
```

## Migration from Legacy Code

Old code (abrupt stop):
```rust
// OLD - May drop active requests
engine.stop("llama-7b").await?;
```

New code (graceful stop):
```rust
// NEW - Waits for requests to complete
manager.graceful_stop("llama-7b").await?;
```

## Future Enhancements

Potential improvements:
- Rolling updates across multiple nodes
- A/B testing support (split traffic between versions)
- Automatic rollback on error rate spikes
- Integration with Kubernetes for orchestration
